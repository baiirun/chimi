//! Which side serves a call to an intrinsic: the families that have moved
//! to Rust are called here, straight from the evaluator, and the rest
//! (the process kernel) go to `runtime.c`'s
//! `chimi_runtime_invoke_native_v1`.
//!
//! The ranges are the `CHIMI_INTRINSIC_*` order of runtime.h, which keeps
//! each family contiguous; heap/layout.rs pins every bound.

use core::ffi::c_void;
use core::ptr;

use super::abi::arguments;
use super::assertions::assertion_operation;
use super::binding_calls::binding_call;
use super::bytes::bytes_operation;
use super::causes::cause_operation;
use super::channels::channel_operation;
use super::clocks::{ClockCall, clock_host, clock_operation};
use super::collections::{Call, map_operation, set_operation};
use super::console_io::{console_host, log_operation};
use super::core::core_operation;
use super::decimals::decimal_operation;
use super::durations::duration_operation;
use super::host::host_operation;
use super::input::process_utf8_lines;
use super::json::json_operation;
use super::metrics_host::metrics_operation;
use super::processes::process_data_operation;
use super::random_host::random_host;
use super::scopes::{ScopeCall, capture_defect, cause_context, resource_scoped};
use super::scalars::scalar_operation;
use super::signals::signal_operation;
use super::standard::defect;
use super::tasks::{TaskCall, task_operation};
use super::test_log::test_log_operation;
use super::waits::wait_operation;
use crate::heap::layout::{Callable, CallableKind, Control, Core, Environment, Intrinsic, Runtime, Value};

unsafe extern "C" {
    /// `invoke` for an intrinsic `runtime.c` serves, a host function or a C
    /// binding.
    fn chimi_runtime_invoke_native_v1(
        runtime: *mut Runtime,
        callable: *mut Callable,
        arguments: *mut *mut Value,
        argument_count: u32,
        span: u64,
        call_node: u32,
        environment: *mut Environment,
    ) -> *mut Control;
}

/// The intrinsics over single values (intrinsics/scalars.rs).
fn is_scalar(intrinsic: u32) -> bool {
    matches!(
        intrinsic,
        Intrinsic::IO_PATH
            | Intrinsic::PATH_FROM
            | Intrinsic::PATH_TO_TEXT
            | Intrinsic::LIST_LENGTH
            | Intrinsic::TEXT_APPEND
            | Intrinsic::TEXT_EQUAL
            | Intrinsic::INT_PARSE
            | Intrinsic::INT_TO_TEXT
            | Intrinsic::INT_ADD_CHECKED
            | Intrinsic::INT_SUBTRACT_CHECKED
            | Intrinsic::INT_MULTIPLY_CHECKED
            | Intrinsic::INT_DIVIDE_CHECKED
            | Intrinsic::INT_REMAINDER_CHECKED
            | Intrinsic::INT_MODULO
            | Intrinsic::RANGE_START
            | Intrinsic::RANGE_END
            | Intrinsic::RANGE_LENGTH
            | Intrinsic::RANGE_CONTAINS
            | Intrinsic::FLOAT_FROM_INT
            | Intrinsic::FLOAT_TRUNCATE
            | Intrinsic::FLOAT_FLOOR
            | Intrinsic::FLOAT_CEILING
            | Intrinsic::FLOAT_ROUND
            | Intrinsic::FLOAT_TO_INT
            | Intrinsic::DEBUG_RENDER
            | Intrinsic::DEBUG_RENDER_INDENTED
            | Intrinsic::FLOAT_TO_TEXT
            | Intrinsic::FLOAT_PARSE
    )
}

/// Calls the intrinsic or host function `callable` with `values`.
///
/// # Safety
/// Runs on a fiber; `callable` is an INTRINSIC or HOST callable, `values`
/// holds `count` live values, and the rest are the call's.
#[inline(never)]
pub unsafe fn invoke_native(
    runtime: *mut Runtime,
    callable: *mut Callable,
    values: *mut *mut Value,
    count: u32,
    span: u64,
    call_node: u32,
    environment: *mut Environment,
) -> *mut Control {
    // SAFETY: the caller's contract.
    unsafe {
        if (*callable).kind != CallableKind::INTRINSIC {
            if (*callable).kind == CallableKind::HOST && (*callable).host >= Core::HOST_CLOCK_NOW {
                let call = ClockCall { runtime, span, call_node, environment };
                return clock_host(call, callable, arguments(values, count));
            }
            if (*callable).kind == CallableKind::HOST {
                return match (*callable).host {
                    Core::HOST_WRITE_LINE | Core::HOST_WRITE_ERROR => {
                        console_host(runtime, (*callable).host, arguments(values, count), call_node)
                    }
                    Core::HOST_PROCESS_UTF8_LINES => {
                        process_utf8_lines(ClockCall { runtime, span, call_node, environment }, arguments(values, count))
                    }
                    Core::HOST_RANDOM_NEXT_INT => random_host(runtime, arguments(values, count), call_node),
                    _ => defect(c"unknown host function", i64::from(call_node)),
                };
            }
            return chimi_runtime_invoke_native_v1(runtime, callable, values, count, span, call_node, environment);
        }
        let intrinsic = (*callable).intrinsic;
        let call = || Call { runtime: runtime.cast::<c_void>(), span, call_node, environment };
        let scope = || ScopeCall { runtime, span, call_node, environment };
        match intrinsic {
            Intrinsic::MAP_REMOVE..=Intrinsic::MAP_REDUCE_ENTRIES | Intrinsic::MAP_EMPTY..=Intrinsic::MAP_PUT => {
                map_operation(&call(), intrinsic, arguments(values, count))
            }
            Intrinsic::SET_EMPTY..=Intrinsic::SET_REDUCE_ITEMS => {
                set_operation(&call(), intrinsic, arguments(values, count))
            }
            Intrinsic::JSON_DECODE..=Intrinsic::JSON_NUMBER_TO_DECIMAL => {
                json_operation(intrinsic, arguments(values, count), call_node)
            }
            Intrinsic::DECIMAL_SCALED..=Intrinsic::DECIMAL_TO_FLOAT => {
                decimal_operation(intrinsic, arguments(values, count), call_node)
            }
            Intrinsic::BYTES_FROM..=Intrinsic::EMBED_BYTES | Intrinsic::HTTP_COUNT_ENDS => {
                bytes_operation(intrinsic, arguments(values, count), call_node)
            }
            Intrinsic::CAUSE_CONTEXT => cause_context(scope(), arguments(values, count)),
            Intrinsic::CAUSE_OF..=Intrinsic::CAUSE_TO_TEXT => {
                cause_operation(intrinsic, arguments(values, count), call_node)
            }
            Intrinsic::TASK_RESULT..=Intrinsic::CHANNEL_BOUNDED
            | Intrinsic::CHANNEL_ADD_RECEIVER..=Intrinsic::WATCH_RECEIVE => {
                channel_operation(intrinsic, arguments(values, count), call_node)
            }
            Intrinsic::CLOCK_SLEEP..=Intrinsic::TASK_DEADLINE => {
                clock_operation(ClockCall { runtime, span, call_node, environment }, intrinsic, arguments(values, count))
            }
            Intrinsic::SELECT_RECEIVE..=Intrinsic::SELECT_FIRST | Intrinsic::SELECT_WATCH => {
                wait_operation(ClockCall { runtime, span, call_node, environment }, intrinsic, arguments(values, count))
            }
            Intrinsic::TASK_START | Intrinsic::TASK_ALL..=Intrinsic::TASK_RACE => {
                task_operation(TaskCall { runtime, span, call_node, environment }, intrinsic, arguments(values, count))
            }
            Intrinsic::PROCESS_SPEC
            | Intrinsic::PROCESS_REGISTER
            | Intrinsic::PROCESS_HANDOFF_SPEC..=Intrinsic::SUPERVISOR_RESTARTS => {
                process_data_operation(intrinsic, arguments(values, count), call_node)
            }
            Intrinsic::RESOURCE_SCOPED => resource_scoped(scope(), arguments(values, count)),
            Intrinsic::TEST_CAPTURE_DEFECT => capture_defect(scope(), arguments(values, count)),
            Intrinsic::TEST_ASSERT_EQUAL..=Intrinsic::TEST_FAIL => {
                assertion_operation(intrinsic, arguments(values, count), call_node)
            }
            Intrinsic::TEST_RECORDER..=Intrinsic::TEST_RECEIVED => {
                test_log_operation(intrinsic, arguments(values, count), call_node)
            }
            Intrinsic::ENV_HOST_VARIABLES..=Intrinsic::FS_TEST_ROOT => {
                let cancel = ptr::read_volatile(&raw const (*runtime).cancel) != 0;
                host_operation(runtime.cast::<c_void>(), cancel, intrinsic, arguments(values, count), call_node)
            }
            Intrinsic::BINDING_CALL => binding_call(runtime, (*callable).symbol, values, count, call_node),
            Intrinsic::SIGNAL_HOST_INSTALL | Intrinsic::SIGNAL_HOST_WAIT => {
                signal_operation(intrinsic, arguments(values, count), call_node)
            }
            Intrinsic::LOG_HOST_MINIMUM_LEVEL | Intrinsic::LOG_HOST_WRITE => {
                log_operation(runtime, intrinsic, arguments(values, count), call_node)
            }
            Intrinsic::METRICS_HOST_ENABLED..=Intrinsic::METRICS_HOST_DOCUMENT => {
                metrics_operation(runtime, intrinsic, arguments(values, count), call_node)
            }
            Intrinsic::DURATION_NANOSECONDS..=Intrinsic::DURATION_PARSE => {
                duration_operation(intrinsic, arguments(values, count), call_node)
            }
            Intrinsic::DEBUG_CRASH..=Intrinsic::LIST_SET => {
                core_operation(intrinsic, arguments(values, count), call_node)
            }
            _ if is_scalar(intrinsic) => scalar_operation(intrinsic, arguments(values, count), call_node),
            _ => chimi_runtime_invoke_native_v1(runtime, callable, values, count, span, call_node, environment),
        }
    }
}
