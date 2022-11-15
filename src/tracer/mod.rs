use std::collections::HashMap;

use zkwasm_types::{
    host_function::HostFunctionDesc,
    imtable::{InitMemoryTable, InitMemoryTableEntry},
    itable::{InstructionTable, InstructionTableEntry},
    jtable::FrameTable,
    mtable::LocationType,
    types::{FunctionType, ValueType},
};

use crate::{
    runner::{from_value_internal_to_u64_with_typ, ValueInternal},
    FuncRef,
    GlobalRef,
    MemoryRef,
    Module,
    ModuleRef,
    Signature,
};

use self::etable::ETable;

pub mod etable;

#[derive(Debug)]
pub struct FuncDesc {
    pub index_within_jtable: u16,
    pub ftype: FunctionType,
    pub signature: Signature,
}

#[derive(Debug)]
pub struct Tracer {
    pub itable: InstructionTable,
    pub imtable: InitMemoryTable,
    pub etable: ETable,
    pub jtable: FrameTable,
    module_instance_lookup: Vec<(ModuleRef, u16)>,
    memory_instance_lookup: Vec<(MemoryRef, u16)>,
    global_instance_lookup: Vec<(GlobalRef, (u16, u16))>,
    function_lookup: Vec<(FuncRef, u16)>,
    last_jump_eid: Vec<u64>,
    function_index_allocator: u32,
    pub(crate) function_index_translation: HashMap<u32, FuncDesc>,
    pub host_function_index_lookup: HashMap<usize, HostFunctionDesc>,
}

impl Tracer {
    /// Create an empty tracer
    pub fn new(host_plugin_lookup: HashMap<usize, HostFunctionDesc>) -> Self {
        Tracer {
            itable: InstructionTable::new(),
            imtable: InitMemoryTable::new(),
            etable: ETable::default(),
            last_jump_eid: vec![0],
            jtable: FrameTable::default(),
            module_instance_lookup: vec![],
            memory_instance_lookup: vec![],
            global_instance_lookup: vec![],
            function_lookup: vec![],
            function_index_allocator: 1,
            function_index_translation: Default::default(),
            host_function_index_lookup: host_plugin_lookup,
        }
    }

    pub fn push_frame(&mut self) {
        self.last_jump_eid.push(self.etable.get_latest_eid());
    }

    pub fn pop_frame(&mut self) {
        self.last_jump_eid.pop().unwrap();
    }

    pub fn next_module_id(&self) -> u16 {
        (self.module_instance_lookup.len() as u16) + 1
    }

    pub fn next_memory_id(&self) -> u16 {
        (self.memory_instance_lookup.len() as u16) + 1
    }

    pub fn last_jump_eid(&self) -> u64 {
        *self.last_jump_eid.last().unwrap()
    }

    pub fn eid(&self) -> u64 {
        self.etable.get_latest_eid()
    }

    fn allocate_func_index(&mut self) -> u32 {
        let r = self.function_index_allocator;
        self.function_index_allocator = r + 1;
        r
    }

    fn lookup_host_plugin(&self, function_index: usize) -> HostFunctionDesc {
        self.host_function_index_lookup
            .get(&function_index)
            .unwrap()
            .clone()
    }
}

impl Tracer {
    pub(crate) fn push_init_memory(&mut self, memref: MemoryRef) {
        let pages = (*memref).limits().initial();
        // one page contains 64KB*1024/8=8192 u64 entries
        for i in 0..(pages * 8192) {
            let mut buf = [0u8; 8];
            (*memref).get_into(i * 8, &mut buf).unwrap();
            self.imtable.push(&InitMemoryTableEntry {
                ltype: LocationType::Heap,
                is_mutable: true,
                mmid: self.next_memory_id().try_into().unwrap(),
                offset: i.try_into().unwrap(),
                vtype: ValueType::I64,
                value: u64::from_le_bytes(buf),
            });
        }

        self.memory_instance_lookup
            .push((memref, self.next_memory_id()));
    }

    pub(crate) fn push_global(&mut self, moid: u16, globalidx: u32, globalref: &GlobalRef) {
        let vtype = globalref.elements_value_type().into();

        if let Some((_origin_moid, _origin_idx)) = self.lookup_global_instance(globalref) {
            // Import global does not support yet.
            todo!()
        } else {
            self.global_instance_lookup
                .push((globalref.clone(), (moid, globalidx as u16)));

            self.imtable.push(&InitMemoryTableEntry {
                ltype: LocationType::Global,
                is_mutable: globalref.is_mutable(),
                mmid: moid.try_into().unwrap(),
                offset: globalidx.try_into().unwrap(),
                vtype,
                value: from_value_internal_to_u64_with_typ(
                    vtype,
                    ValueInternal::from(globalref.get()),
                ),
            });
        }
    }

    #[allow(dead_code)]
    pub(crate) fn statistics_instructions<'a>(&mut self, module_instance: &ModuleRef) {
        let mut func_index = 0;
        let mut insts = vec![];

        loop {
            if let Some(func) = module_instance.func_by_index(func_index) {
                let body = func.body().unwrap();

                let code = &body.code.vec;

                for inst in code {
                    if insts.iter().position(|i| i == inst).is_none() {
                        insts.push(inst.clone())
                    }
                }
            } else {
                break;
            }

            func_index = func_index + 1;
        }

        for inst in insts {
            println!("{:?}", inst);
        }
    }

    pub(crate) fn register_module_instance(
        &mut self,
        module: &Module,
        module_instance: &ModuleRef,
    ) {
        let start_fn_idx = module.module().start_section();

        {
            let mut func_index = 0;

            loop {
                if let Some(func) = module_instance.func_by_index(func_index) {
                    let func_index_in_itable = if Some(func_index) == start_fn_idx {
                        0
                    } else {
                        self.allocate_func_index()
                    };

                    let ftype = match *func.as_internal() {
                        crate::func::FuncInstanceInternal::Internal { .. } => {
                            FunctionType::WasmFunction
                        }
                        crate::func::FuncInstanceInternal::Host {
                            host_func_index, ..
                        } => {
                            let plugin_desc = self.lookup_host_plugin(host_func_index);

                            FunctionType::HostFunction {
                                plugin: plugin_desc.plugin,
                                function_index: host_func_index,
                                function_name: plugin_desc.name,
                                op_index_in_plugin: plugin_desc.op_index_in_plugin,
                            }
                        }
                    };

                    self.function_lookup
                        .push((func.clone(), func_index_in_itable as u16));
                    self.function_index_translation.insert(
                        func_index,
                        FuncDesc {
                            index_within_jtable: func_index_in_itable as u16,
                            ftype,
                            signature: func.signature().clone(),
                        },
                    );
                    func_index = func_index + 1;
                } else {
                    break;
                }
            }
        }

        {
            let mut func_index = 0;

            loop {
                if let Some(func) = module_instance.func_by_index(func_index) {
                    let funcdesc = self.function_index_translation.get(&func_index).unwrap();

                    if let Some(body) = func.body() {
                        let code = &body.code;
                        let mut iter = code.iterate_from(0);
                        loop {
                            let pc = iter.position();
                            if let Some(instruction) = iter.next() {
                                let moid = self.next_module_id();

                                let ientry = InstructionTableEntry {
                                    moid,
                                    mmid: moid,
                                    fid: funcdesc.index_within_jtable,
                                    iid: pc.try_into().unwrap(),
                                    opcode: instruction.into(&self.function_index_translation),
                                };

                                self.itable.push(&ientry);
                            } else {
                                break;
                            }
                        }
                    }

                    func_index = func_index + 1;
                } else {
                    break;
                }
            }
        }

        self.module_instance_lookup
            .push((module_instance.clone(), self.next_module_id()));
    }

    pub fn lookup_module_instance(&self, module_instance: &ModuleRef) -> u16 {
        for m in &self.module_instance_lookup {
            if &m.0 == module_instance {
                return m.1;
            }
        }

        unreachable!()
    }

    pub fn lookup_memory_instance(&self, module_instance: &MemoryRef) -> u16 {
        for m in &self.memory_instance_lookup {
            if &m.0 == module_instance {
                return m.1;
            }
        }

        unreachable!()
    }

    pub fn lookup_global_instance(&self, global_instance: &GlobalRef) -> Option<(u16, u16)> {
        for m in &self.global_instance_lookup {
            if &m.0 == global_instance {
                return Some(m.1);
            }
        }

        None
    }

    pub fn lookup_function(&self, function: &FuncRef) -> u16 {
        let pos = self
            .function_lookup
            .iter()
            .position(|m| m.0 == *function)
            .unwrap();
        self.function_lookup.get(pos).unwrap().1
    }

    pub fn lookup_ientry(&self, function: &FuncRef, pos: u32) -> InstructionTableEntry {
        let function_idx = self.lookup_function(function);

        for ientry in &self.itable.0 {
            if ientry.fid == function_idx && ientry.iid as u32 == pos {
                return ientry.clone();
            }
        }

        unreachable!()
    }

    pub fn lookup_first_inst(&self, function: &FuncRef) -> InstructionTableEntry {
        let function_idx = self.lookup_function(function);

        for ientry in &self.itable.0 {
            if ientry.fid == function_idx {
                return ientry.clone();
            }
        }

        unreachable!();
    }
}
