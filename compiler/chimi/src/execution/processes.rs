//! `std/process`, `std/supervisor`, and `std/clock` on the reference interpreter.
//!
//! The interpreter has no process runtime (coordinator decision 1 on ts-6ccbc9). It
//! runs the data constructors, which build plain records so module constants can hold
//! specs, members, and policies, and checks their ranges with the defect texts the
//! native kernel uses. Every operation that needs a running process or the clock
//! defects with "requires the supervised process runtime".

use super::{Control, Evaluator, RuntimeField, RuntimeValue};
use crate::ids::CoreNodeId;
use crate::standard::{StandardConstructor, StandardFunction};

impl Evaluator<'_> {
    /// Requires: `function` is in `std/process`, `std/supervisor`, or `std/clock`.
    pub(super) fn invoke_process(
        &self,
        function: StandardFunction,
        arguments: Vec<RuntimeValue>,
        call_node: CoreNodeId,
    ) -> Control {
        if function.requires_process_runtime() {
            return defect(
                format!(
                    "{}.{} requires the supervised process runtime",
                    function
                        .module()
                        .expect("a process function has a module")
                        .qualifier(),
                    function.name()
                ),
                call_node,
            );
        }
        let checked = match (function, arguments.as_slice()) {
            (
                StandardFunction::ProcessSpec,
                [
                    body @ RuntimeValue::Callable(_),
                    RuntimeValue::Integer(capacity),
                    within @ RuntimeValue::Duration(nanoseconds),
                ],
            ) => {
                if *capacity < 1 {
                    Err(format!(
                        "Process.spec: mailbox capacity must be at least 1, got {capacity}"
                    ))
                } else if *nanoseconds < 0 {
                    Err(format!(
                        "Process.spec: shutdown_within must be at least zero, got {}",
                        crate::duration::to_text(*nanoseconds)
                    ))
                } else {
                    Ok(record([
                        ("body", body.clone()),
                        ("capacity", RuntimeValue::Integer(*capacity)),
                        (
                            "name",
                            self.standard_value(StandardConstructor::OptionNone, Vec::new()),
                        ),
                        ("shutdown_within", within.clone()),
                    ]))
                }
            }
            (
                StandardFunction::ProcessRegister,
                [
                    RuntimeValue::Record(fields),
                    address @ RuntimeValue::ProcessAddress(_),
                ],
            ) => match field(fields, "name") {
                Some(RuntimeValue::Constructed { fields: named, .. }) if !named.is_empty() => {
                    Err("Process.register: spec already has a name".to_owned())
                }
                Some(_) => Ok(RuntimeValue::Record(
                    fields
                        .iter()
                        .map(|existing| RuntimeField {
                            name: existing.name.clone(),
                            value: if existing.name == "name" {
                                self.standard_value(
                                    StandardConstructor::OptionSome,
                                    vec![address.clone()],
                                )
                            } else {
                                existing.value.clone()
                            },
                        })
                        .collect::<Vec<_>>()
                        .into(),
                )),
                None => return self.intrinsic_shape_defect(function, call_node),
            },
            (StandardFunction::SupervisorProcess, [spec, restart]) => Ok(record([
                ("restart", restart.clone()),
                ("spec", spec.clone()),
            ])),
            (StandardFunction::SupervisorSupervisor, [policy, restart]) => Ok(record([
                ("policy", policy.clone()),
                ("restart", restart.clone()),
            ])),
            (
                StandardFunction::SupervisorPolicy,
                [
                    members @ RuntimeValue::List(list),
                    RuntimeValue::Integer(capacity),
                    restarts,
                ],
            ) => {
                if *capacity < 1 {
                    Err(format!(
                        "Supervisor.policy: capacity must be at least 1, got {capacity}"
                    ))
                } else if list.len() as i64 > *capacity {
                    Err(format!(
                        "Supervisor.policy: members must not exceed capacity {capacity}, got {}",
                        list.len()
                    ))
                } else {
                    Ok(record([
                        ("capacity", RuntimeValue::Integer(*capacity)),
                        ("members", members.clone()),
                        ("restarts", restarts.clone()),
                    ]))
                }
            }
            (
                StandardFunction::SupervisorRestarts,
                [
                    RuntimeValue::Integer(limit),
                    window @ RuntimeValue::Duration(nanoseconds),
                ],
            ) => {
                if *limit < 0 {
                    Err(format!(
                        "Supervisor.restarts: restart limit must be at least 0, got {limit}"
                    ))
                } else if *nanoseconds <= 0 {
                    Err(format!(
                        "Supervisor.restarts: restart window must be positive, got {}",
                        crate::duration::to_text(*nanoseconds)
                    ))
                } else {
                    Ok(record([
                        ("limit", RuntimeValue::Integer(*limit)),
                        ("window", window.clone()),
                    ]))
                }
            }
            _ => return self.intrinsic_shape_defect(function, call_node),
        };
        match checked {
            Ok(value) => Control::Success(value),
            Err(message) => defect(message, call_node),
        }
    }
}

fn defect(message: String, call_node: CoreNodeId) -> Control {
    Control::Defect {
        message,
        node: Some(call_node),
    }
}

/// A record whose fields are given in ascending name order.
fn record<const N: usize>(fields: [(&str, RuntimeValue); N]) -> RuntimeValue {
    RuntimeValue::Record(
        fields
            .into_iter()
            .map(|(name, value)| RuntimeField {
                name: name.to_owned(),
                value,
            })
            .collect::<Vec<_>>()
            .into(),
    )
}

fn field<'value>(fields: &'value [RuntimeField], name: &str) -> Option<&'value RuntimeValue> {
    fields
        .iter()
        .find(|field| field.name == name)
        .map(|field| &field.value)
}
