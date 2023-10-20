use specs::{
    host_function::HostPlugin,
    itable::InstructionTableEntry,
    step::StepInfo,
    types::ValueType,
};

use super::{etable::ETable, Tracer};
use crate::{
    isa::{DropKeep, Instruction, Keep},
    Signature,
};
use std::rc::Rc;
use core::{cell::RefCell};
use lazy_static::lazy_static;
use std::sync::Mutex;

lazy_static! {
    pub static ref ETABLE_TRACE: Mutex<usize> = Mutex::new(0);
}

pub fn reset_trace_count() {
    let mut counter = ETABLE_TRACE.lock().unwrap();
    *counter += 0;
}

pub fn add_trace_count() {
    let mut counter = ETABLE_TRACE.lock().unwrap();
    *counter += 1;
}

pub fn get_trace_count() -> usize {
    let mut counter = ETABLE_TRACE.lock().unwrap();
    let count = *counter;
    *counter = 0;
    return count;
}

pub struct PhantomFunction;

impl PhantomFunction {
    pub fn build_phantom_function_instructions(
        sig: &Signature,
        // Wasm Image Function Id
        wasm_input_function_idx: u32,
    ) -> Vec<Instruction<'static>> {
        let mut instructions = vec![];

        if sig.return_type().is_some() {
            instructions.push(Instruction::I32Const(0));

            instructions.push(Instruction::Call(wasm_input_function_idx));

            if sig.return_type() != Some(wasmi_core::ValueType::I64) {
                instructions.push(Instruction::I32WrapI64);
            }
        }

        instructions.push(Instruction::Return(DropKeep {
            drop: sig.params().len() as u32,
            keep: if let Some(t) = sig.return_type() {
                Keep::Single(t.into_elements())
            } else {
                Keep::None
            },
        }));

        instructions
    }
}

impl Tracer {
    pub fn fill_trace(
        tracer: &Option<Rc<RefCell<Tracer>>>,
        current_sp: u32,
        allocated_memory_pages: u32,
        last_jump_eid: u32,
        fid: u32,
        callee_sig: &Signature,
        // Wasm Image Function Id
        wasm_input_function_idx: u32,
        keep_value: Option<u64>,
    ) {
        let mut inst = PhantomFunction::build_phantom_function_instructions(
            &callee_sig,
            wasm_input_function_idx,
        )
            .into_iter();
        let mut iid = 0;

        let has_return_value = callee_sig.return_type().is_some();
        let mut wasm_input_host_func_index: usize = 0;
        if let Some(tracer) = &tracer {
            let wasm_input_host_function_ref = tracer.borrow().wasm_input_func_ref.clone().unwrap();
            let func_index = match wasm_input_host_function_ref.as_internal() {
                crate::func::FuncInstanceInternal::Internal { .. } => unreachable!(),
                crate::func::FuncInstanceInternal::Host {
                    host_func_index, ..
                } => host_func_index,
            };
            wasm_input_host_func_index = *func_index;
        }


        if has_return_value {
            if let Some(tracer) = &tracer {
                tracer.borrow_mut().etable.push(
                    InstructionTableEntry {
                        fid,
                        iid,
                        opcode: inst.next().unwrap().into(&tracer.borrow().function_index_translation),
                    },
                    current_sp,
                    allocated_memory_pages,
                    last_jump_eid,
                    StepInfo::I32Const { value: 0 },
                );
            }
            add_trace_count();

            iid += 1;

            if let Some(tracer) = &tracer {
                tracer.borrow_mut().etable.push(
                    InstructionTableEntry {
                        fid,
                        iid,
                        opcode: inst.next().unwrap().into(&tracer.borrow().function_index_translation),
                    },
                    current_sp + 1,
                    allocated_memory_pages,
                    last_jump_eid,
                    StepInfo::CallHost {
                        plugin: HostPlugin::HostInput,
                        host_function_idx: wasm_input_host_func_index,
                        function_name: "wasm_input".to_owned(),
                        signature: specs::host_function::Signature {
                            params: vec![ValueType::I32],
                            return_type: Some(ValueType::I64),
                        },
                        args: vec![0],
                        ret_val: Some(keep_value.unwrap()),
                        op_index_in_plugin: 0,
                    },
                );
            }
            add_trace_count();

            iid += 1;

            if callee_sig.return_type() != Some(wasmi_core::ValueType::I64) {
                if let Some(tracer) = &tracer {
                    tracer.borrow_mut().etable.push(
                        InstructionTableEntry {
                            fid,
                            iid,
                            opcode: inst.next().unwrap().into(&tracer.borrow().function_index_translation),
                        },
                        current_sp + 1,
                        allocated_memory_pages,
                        last_jump_eid,
                        StepInfo::I32WrapI64 {
                            value: keep_value.unwrap() as i64,
                            result: keep_value.unwrap() as i32,
                        },
                    );
                }
                add_trace_count();

                iid += 1;
            }
        }

        if let Some(tracer) = &tracer {
            tracer.borrow_mut().etable.push(
                InstructionTableEntry {
                    fid,
                    iid,
                    opcode: inst.next().unwrap().into(&tracer.borrow().function_index_translation),
                },
                current_sp + has_return_value as u32,
                allocated_memory_pages,
                last_jump_eid,
                StepInfo::Return {
                    drop: callee_sig.params().len() as u32,
                    keep: if let Some(t) = callee_sig.return_type() {
                        vec![t.into_elements().into()]
                    } else {
                        vec![]
                    },
                    keep_values: keep_value.map_or(vec![], |v| vec![v]),
                },
            );
        }
        add_trace_count();
    }

    pub fn fill_trace_count(
        callee_sig: &Signature,
    ) {
        let has_return_value = callee_sig.return_type().is_some();

        if has_return_value {
            add_trace_count();
            add_trace_count();
            if callee_sig.return_type() != Some(wasmi_core::ValueType::I64) {
                add_trace_count();
            }
        }
        add_trace_count();
    }
}


