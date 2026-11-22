//! The operation registry: the one table that names every runtime-backed item of
//! the embedded standard tree (`compiler/chimi/std/`).
//!
//! A `native fn` in a standard module binds the operation whose id is its
//! qualified name (`std/list.push`); source never spells the id. A runtime-known
//! declaration is an ordinary source declaration that the runtime constructs or
//! inspects (`std/option.Option.Some`), so it carries a fixed runtime identity.
//!
//! In this phase an operation's implementation is a [`StandardFunction`]
//! intrinsic, which both backends already dispatch (the interpreter's intrinsic
//! match and the native runtime's `CHIMI_INTRINSIC_*` switch), and a
//! runtime-known declaration takes the identity of a [`StandardType`] or
//! [`StandardConstructor`]. The generated `operations.h` table replaces both
//! when the remaining `standard.rs` modules move to source.

use crate::compiler::{ProjectSource, resolve_project_sources};
use crate::diagnostic::Diagnostic;
use crate::standard::{StandardConstructor, StandardFunction, StandardSymbol, StandardType};
use crate::typing::{ValueProperty, check_project};
use std::collections::{HashMap, HashSet};

/// What an operation may touch, which decides where it may run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OperationClass {
    /// Deterministic and creates no runtime resource; constants may call it.
    Pure,
    /// Needs the scheduler or creates runtime resources.
    Runtime,
    /// Talks to the operating system; its `native fn` must be private (`N3106`).
    Host,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Operation {
    /// The qualified name of the `native fn` that binds it.
    pub(crate) id: &'static str,
    /// Checked against the declaration's parameter count (`T4320`).
    pub(crate) arity: usize,
    pub(crate) class: OperationClass,
    pub(crate) implementation: StandardFunction,
    /// Only tests may reach it: `chimi build` and `chimi run` reject a program
    /// whose `main` reaches it (`T4401`, testing.md "Defects").
    pub(crate) test_only: bool,
    /// The C function that implements a binding operation (G20), whose
    /// `implementation` is [`StandardFunction::BindingCall`].
    pub(crate) binding: Option<Binding>,
}

/// A C function of a binding package (`backend/bindings.rs`) that implements
/// one `native fn`. It has the binding API's one signature
/// (`chimi_binding.h`), so the registry needs only its name.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct Binding {
    /// The package whose archive defines `symbol`.
    pub(crate) package: &'static str,
    pub(crate) symbol: &'static str,
}

/// A source declaration whose identity the runtime knows.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RuntimeKnownDeclaration {
    /// `std/<module>.<Type>` or `std/<module>.<Type>.<Variant>`.
    pub(crate) id: &'static str,
    pub(crate) identity: RuntimeIdentity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RuntimeIdentity {
    Type(StandardType),
    /// The declaration's payload arity must equal the constructor's field count
    /// (`N3105`).
    Constructor(StandardConstructor),
    /// A `native type`: opaque in source, represented by the runtime.
    NativeType(StandardType),
    /// An `interface` written in source that the runner provides at the root,
    /// as `std/random.Random`, or that a runner provider builds
    /// (`RUNNER_PROVIDERS`), as `std/env.Environment`.
    Interface(StandardType),
}

const fn pure(id: &'static str, arity: usize, implementation: StandardFunction) -> Operation {
    Operation {
        id,
        arity,
        class: OperationClass::Pure,
        implementation,
        test_only: false,
        binding: None,
    }
}

const fn runtime(id: &'static str, arity: usize, implementation: StandardFunction) -> Operation {
    Operation {
        id,
        arity,
        class: OperationClass::Runtime,
        implementation,
        test_only: false,
        binding: None,
    }
}

const fn host(id: &'static str, arity: usize, implementation: StandardFunction) -> Operation {
    Operation {
        id,
        arity,
        class: OperationClass::Host,
        implementation,
        test_only: false,
        binding: None,
    }
}

const fn test_only(id: &'static str, arity: usize, implementation: StandardFunction) -> Operation {
    Operation {
        id,
        arity,
        class: OperationClass::Runtime,
        implementation,
        test_only: true,
        binding: None,
    }
}

/// A `host` operation implemented by `symbol` in binding package `package`.
/// `test_only` binds it like [`test_only`]: only tests may reach it.
const fn binding(
    id: &'static str,
    arity: usize,
    package: &'static str,
    symbol: &'static str,
    test_only: bool,
) -> Operation {
    Operation {
        id,
        arity,
        class: OperationClass::Host,
        implementation: StandardFunction::BindingCall,
        test_only,
        binding: Some(Binding { package, symbol }),
    }
}

/// The source spelling of a test-only operation (`Test.capture_defect` for
/// `std/test.capture_defect`), for `T4401`; `None` for any other operation.
pub(crate) fn test_only_name(operation: &Operation) -> Option<String> {
    if !operation.test_only {
        return None;
    }
    let (module, name) = operation.id.strip_prefix("std/")?.split_once('.')?;
    let qualifier = module
        .split('_')
        .map(|word| {
            let mut letters = word.chars();
            letters.next().map_or_else(String::new, |first| {
                format!("{}{}", first.to_ascii_uppercase(), letters.as_str())
            })
        })
        .collect::<String>();
    Some(format!("{qualifier}.{name}"))
}

/// The registry operation an intrinsic implements. `None` for
/// [`StandardFunction::BindingCall`], which many operations share.
pub(crate) fn intrinsic_operation(function: StandardFunction) -> Option<&'static Operation> {
    OPERATIONS
        .iter()
        .find(|operation| operation.implementation == function && operation.binding.is_none())
}

pub(crate) const OPERATIONS: &[Operation] = &[
    pure(
        "std/option.unwrap_or_fail",
        2,
        StandardFunction::OptionUnwrapOrFail,
    ),
    pure("std/int.parse", 1, StandardFunction::IntParse),
    pure("std/int.to_text", 1, StandardFunction::IntToText),
    pure("std/int.add_checked", 2, StandardFunction::IntAddChecked),
    pure(
        "std/int.subtract_checked",
        2,
        StandardFunction::IntSubtractChecked,
    ),
    pure(
        "std/int.multiply_checked",
        2,
        StandardFunction::IntMultiplyChecked,
    ),
    pure(
        "std/int.divide_checked",
        2,
        StandardFunction::IntDivideChecked,
    ),
    pure(
        "std/int.remainder_checked",
        2,
        StandardFunction::IntRemainderChecked,
    ),
    pure("std/int.modulo", 2, StandardFunction::IntModulo),
    // Native rather than `concat([text, other])`: appending in a loop is the
    // common way to build text, and the list would be one allocation per step.
    pure("std/text.append", 2, StandardFunction::TextAppend),
    pure("std/text.concat", 1, StandardFunction::TextConcat),
    pure("std/text.byte_length", 1, StandardFunction::TextByteLength),
    pure("std/text.byte_slice", 2, StandardFunction::TextByteSlice),
    pure("std/text.contains", 2, StandardFunction::TextContains),
    pure("std/text.starts_with", 2, StandardFunction::TextStartsWith),
    pure("std/text.ends_with", 2, StandardFunction::TextEndsWith),
    pure("std/text.split", 2, StandardFunction::TextSplit),
    pure("std/text.replace", 3, StandardFunction::TextReplace),
    pure("std/text.trim_start", 1, StandardFunction::TextTrimStart),
    pure("std/text.trim_end", 1, StandardFunction::TextTrimEnd),
    pure("std/text.lowercase", 1, StandardFunction::TextLowercase),
    pure("std/text.uppercase", 1, StandardFunction::TextUppercase),
    pure("std/text.graphemes", 1, StandardFunction::TextGraphemes),
    pure("std/list.length", 1, StandardFunction::ListLength),
    pure("std/list.get", 2, StandardFunction::ListGet),
    pure("std/list.push", 2, StandardFunction::ListPush),
    pure("std/list.slice", 2, StandardFunction::ListSlice),
    pure("std/list.set", 3, StandardFunction::ListSet),
    pure("std/map.empty", 0, StandardFunction::MapEmpty),
    pure("std/map.get", 2, StandardFunction::MapGet),
    pure("std/map.put", 3, StandardFunction::MapPut),
    pure("std/map.remove", 2, StandardFunction::MapRemove),
    pure("std/map.size", 1, StandardFunction::MapSize),
    pure(
        "std/map.reduce_entries",
        3,
        StandardFunction::MapReduceEntries,
    ),
    pure("std/set.empty", 0, StandardFunction::SetEmpty),
    pure("std/set.contains", 2, StandardFunction::SetContains),
    pure("std/set.add", 2, StandardFunction::SetAdd),
    pure("std/set.remove", 2, StandardFunction::SetRemove),
    pure("std/set.size", 1, StandardFunction::SetSize),
    pure("std/set.reduce_items", 3, StandardFunction::SetReduceItems),
    // std/json: `boolean`, `text`, `array`, and `object` are Chimi.
    pure("std/json.decode", 1, StandardFunction::JsonDecode),
    pure("std/json.encode", 1, StandardFunction::JsonEncode),
    pure("std/json.int", 1, StandardFunction::JsonInt),
    pure(
        "std/json.number_to_int",
        1,
        StandardFunction::JsonNumberToInt,
    ),
    pure(
        "std/json.number_to_decimal",
        1,
        StandardFunction::JsonNumberToDecimal,
    ),
    // std/debug
    pure("std/debug.render", 1, StandardFunction::DebugRender),
    pure(
        "std/debug.render_indented",
        1,
        StandardFunction::DebugRenderIndented,
    ),
    pure("std/debug.crash", 1, StandardFunction::DebugCrash),
    // std/decimal: `+`, `-`, `*`, and comparisons are operators, so only the
    // checked forms and the constructors that validate in Chimi are native.
    pure("std/decimal.scaled", 2, StandardFunction::DecimalScaled),
    pure("std/decimal.read", 1, StandardFunction::DecimalRead),
    pure("std/decimal.to_text", 1, StandardFunction::DecimalToText),
    pure(
        "std/decimal.add_checked",
        2,
        StandardFunction::DecimalAddChecked,
    ),
    pure(
        "std/decimal.subtract_checked",
        2,
        StandardFunction::DecimalSubtractChecked,
    ),
    pure(
        "std/decimal.multiply_checked",
        2,
        StandardFunction::DecimalMultiplyChecked,
    ),
    pure(
        "std/decimal.divide_rounded",
        4,
        StandardFunction::DecimalDivideRounded,
    ),
    pure("std/decimal.to_float", 1, StandardFunction::DecimalToFloat),
    // std/bytes: the Chimi wrappers pass an `IntLayout` as its private code.
    pure("std/bytes.from", 1, StandardFunction::BytesFrom),
    pure("std/bytes.length", 1, StandardFunction::BytesLength),
    pure("std/bytes.get", 2, StandardFunction::BytesGet),
    pure("std/bytes.index_of", 2, StandardFunction::BytesIndexOf),
    pure("std/bytes.concat", 1, StandardFunction::BytesConcat),
    pure("std/bytes.slice", 2, StandardFunction::BytesSlice),
    pure("std/bytes.to_list", 1, StandardFunction::BytesToList),
    pure("std/bytes.from_text", 1, StandardFunction::BytesFromText),
    pure(
        "std/bytes.to_text_lossy",
        1,
        StandardFunction::BytesToTextLossy,
    ),
    pure(
        "std/bytes.valid_utf8_length",
        1,
        StandardFunction::BytesValidUtf8Length,
    ),
    pure(
        "std/bytes.read_int_code",
        3,
        StandardFunction::BytesReadIntCode,
    ),
    pure("std/bytes.int_bytes", 2, StandardFunction::BytesIntBytes),
    // std/embed: build-time only; see `CoreLowerer::embed`.
    pure("std/embed.text", 1, StandardFunction::EmbedText),
    pure("std/embed.bytes", 1, StandardFunction::EmbedBytes),
    // std/cause: `kind` and `reason` map a private code to their enums in source.
    pure("std/cause.context", 2, StandardFunction::CauseContext),
    pure("std/cause.of", 1, StandardFunction::CauseOf),
    pure("std/cause.kind_code", 1, StandardFunction::CauseKindCode),
    pure("std/cause.summary", 1, StandardFunction::CauseSummary),
    pure("std/cause.frames", 1, StandardFunction::CauseFrames),
    pure("std/cause.caused_by", 1, StandardFunction::CauseCausedBy),
    pure("std/cause.suppressed", 1, StandardFunction::CauseSuppressed),
    pure(
        "std/cause.reason_code",
        1,
        StandardFunction::CauseReasonCode,
    ),
    pure("std/cause.to_text", 1, StandardFunction::CauseToText),
    // std/test: assertions defect at their own call (testing.md, Assertions).
    pure(
        "std/test.assert_equal",
        2,
        StandardFunction::TestAssertEqual,
    ),
    pure(
        "std/test.assert_not_equal",
        2,
        StandardFunction::TestAssertNotEqual,
    ),
    pure("std/test.assert", 2, StandardFunction::TestAssert),
    pure("std/test.fail", 1, StandardFunction::TestFail),
    // std/resource
    runtime("std/resource.scoped", 3, StandardFunction::ResourceScoped),
    test_only("std/test.capture_defect", 1, StandardFunction::TestCaptureDefect),
    test_only("std/test.recorder", 0, StandardFunction::TestRecorder),
    test_only("std/test.record", 2, StandardFunction::TestRecord),
    test_only("std/test.recorded", 1, StandardFunction::TestRecorded),
    test_only("std/test.script", 1, StandardFunction::TestScript),
    test_only("std/test.next", 1, StandardFunction::TestNext),
    test_only("std/test.mailbox", 1, StandardFunction::TestMailbox),
    test_only("std/test.received", 1, StandardFunction::TestReceived),
    // std/binding_probe: a test-only binding package that exercises the binding
    // API (G20) from the compiler's own tests.
    binding(
        "std/binding_probe.native_echo",
        1,
        "probe",
        "chimi_probe_echo",
        true,
    ),
    binding(
        "std/binding_probe.native_add",
        2,
        "probe",
        "chimi_probe_add",
        true,
    ),
    binding(
        "std/binding_probe.native_try_parse",
        1,
        "probe",
        "chimi_probe_try_parse",
        true,
    ),
    binding(
        "std/binding_probe.native_crash",
        1,
        "probe",
        "chimi_probe_crash",
        true,
    ),
    binding(
        "std/binding_probe.native_open_pipe",
        0,
        "probe",
        "chimi_probe_open_pipe",
        true,
    ),
    binding(
        "std/binding_probe.native_write_pipe",
        2,
        "probe",
        "chimi_probe_write_pipe",
        true,
    ),
    binding(
        "std/binding_probe.native_read_pipe",
        1,
        "probe",
        "chimi_probe_read_pipe",
        true,
    ),
    binding(
        "std/binding_probe.native_close_pipe",
        1,
        "probe",
        "chimi_probe_close_pipe",
        true,
    ),
    // std/net: TCP sockets (CHI-255). `native_allow_real_network` is test-only,
    // so only `Net.host_for_tests` in a test reaches it.
    binding("std/net.native_connect", 2, "net", "chimi_net_connect", false),
    binding("std/net.native_listen", 2, "net", "chimi_net_listen", false),
    binding("std/net.native_accept", 1, "net", "chimi_net_accept", false),
    binding("std/net.native_read", 1, "net", "chimi_net_read", false),
    binding("std/net.native_write", 2, "net", "chimi_net_write", false),
    binding("std/net.native_finish", 1, "net", "chimi_net_finish", false),
    binding("std/net.native_stop_reading", 1, "net", "chimi_net_stop_reading", false),
    binding("std/net.native_close", 1, "net", "chimi_net_close", false),
    binding("std/net.native_close_listener", 1, "net", "chimi_net_close_listener", false),
    binding("std/net.native_local_address", 1, "net", "chimi_net_local_address", false),
    binding("std/net.native_peer_address", 1, "net", "chimi_net_peer_address", false),
    binding("std/net.native_listener_address", 1, "net", "chimi_net_listener_address", false),
    binding("std/net.native_allow_real_network", 0, "net", "chimi_net_allow_real_network", true),
    // std/tls: TLS sessions over a Net.Connection, through vendored mbedTLS.
    binding("std/tls.native_client", 2, "tls", "chimi_tls_client", false),
    binding("std/tls.native_client_system", 1, "tls", "chimi_tls_client_system", false),
    binding("std/tls.native_server", 2, "tls", "chimi_tls_server", false),
    binding("std/tls.native_check_roots", 1, "tls", "chimi_tls_check_roots", false),
    binding("std/tls.native_check_identity", 2, "tls", "chimi_tls_check_identity", false),
    binding("std/tls.native_handshake", 1, "tls", "chimi_tls_handshake", false),
    binding("std/tls.native_feed", 2, "tls", "chimi_tls_feed", false),
    binding("std/tls.native_take", 1, "tls", "chimi_tls_take", false),
    binding("std/tls.native_send", 2, "tls", "chimi_tls_send", false),
    binding("std/tls.native_receive", 1, "tls", "chimi_tls_receive", false),
    binding("std/tls.native_close_notify", 1, "tls", "chimi_tls_close_notify", false),
    binding("std/tls.native_handoff", 1, "tls", "chimi_tls_handoff", false),
    binding("std/tls.native_claim", 1, "tls", "chimi_tls_claim", false),
    binding("std/tls.native_unclaim", 1, "tls", "chimi_tls_unclaim", false),
    binding("std/tls.native_free", 1, "tls", "chimi_tls_free", false),
    binding("std/tls.native_live_sessions", 0, "tls", "chimi_tls_live_sessions", true),
    // std/sqlite: the SQLite driver for std/sql, through vendored SQLite.
    binding("std/postgres.native_scram", 4, "tls", "chimi_tls_postgres_scram", false),
    binding("std/postgres.native_scram_check", 4, "tls", "chimi_tls_postgres_scram", true),
    binding("std/postgres.native_random", 1, "tls", "chimi_tls_postgres_random", false),
    binding("std/postgres.native_gate", 2, "tls", "chimi_tls_postgres_gate", false),
    binding("std/postgres.native_claim", 1, "tls", "chimi_tls_postgres_claim", false),
    binding("std/postgres.native_settle", 4, "tls", "chimi_tls_postgres_settle", false),
    binding("std/postgres.native_abandon", 2, "tls", "chimi_tls_postgres_abandon", false),
    binding("std/postgres.native_status", 1, "tls", "chimi_tls_postgres_status", false),
    binding("std/postgres.native_depth", 1, "tls", "chimi_tls_postgres_depth", false),
    binding("std/postgres.native_set_depth", 2, "tls", "chimi_tls_postgres_set_depth", false),
    binding("std/sqlite.native_version", 0, "sqlite", "chimi_sqlite_version", false),
    binding("std/sqlite.native_open", 1, "sqlite", "chimi_sqlite_open", false),
    binding("std/sqlite.native_close", 1, "sqlite", "chimi_sqlite_close", false),
    binding("std/sqlite.native_autocommit", 1, "sqlite", "chimi_sqlite_autocommit", false),
    binding("std/sqlite.native_depth", 1, "sqlite", "chimi_sqlite_depth", false),
    binding("std/sqlite.native_set_depth", 2, "sqlite", "chimi_sqlite_set_depth", false),
    binding("std/sqlite.native_prepare", 2, "sqlite", "chimi_sqlite_prepare", false),
    binding("std/sqlite.native_finalize", 1, "sqlite", "chimi_sqlite_finalize", false),
    binding("std/sqlite.native_parameter_count", 1, "sqlite", "chimi_sqlite_parameter_count", false),
    binding("std/sqlite.native_bind_null", 2, "sqlite", "chimi_sqlite_bind_null", false),
    binding("std/sqlite.native_bind_integer", 3, "sqlite", "chimi_sqlite_bind_integer", false),
    binding("std/sqlite.native_bind_float", 3, "sqlite", "chimi_sqlite_bind_float", false),
    binding("std/sqlite.native_bind_text", 3, "sqlite", "chimi_sqlite_bind_text", false),
    binding("std/sqlite.native_bind_bytes", 3, "sqlite", "chimi_sqlite_bind_bytes", false),
    binding("std/sqlite.native_step", 1, "sqlite", "chimi_sqlite_step", false),
    binding("std/sqlite.native_columns", 1, "sqlite", "chimi_sqlite_columns", false),
    binding("std/sqlite.native_row", 1, "sqlite", "chimi_sqlite_row", false),
    // std/exec: child processes (CHI-250). `native_allow_real_commands` is
    // test-only, so only `Exec.host_for_tests` in a test reaches it.
    binding("std/exec.native_command", 1, "exec", "chimi_exec_command", false),
    binding("std/exec.native_argument", 2, "exec", "chimi_exec_argument", false),
    binding("std/exec.native_variable", 3, "exec", "chimi_exec_variable", false),
    binding("std/exec.native_clear_environment", 1, "exec", "chimi_exec_clear_environment", false),
    binding("std/exec.native_directory", 2, "exec", "chimi_exec_directory", false),
    binding("std/exec.native_spawn", 4, "exec", "chimi_exec_spawn", false),
    binding("std/exec.native_wait", 1, "exec", "chimi_exec_wait", false),
    binding("std/exec.native_terminate", 1, "exec", "chimi_exec_terminate", false),
    binding("std/exec.native_kill", 1, "exec", "chimi_exec_kill", false),
    binding("std/exec.native_stop", 2, "exec", "chimi_exec_stop", false),
    binding("std/exec.native_release", 1, "exec", "chimi_exec_release", false),
    binding("std/exec.native_read", 1, "exec", "chimi_exec_read", false),
    binding("std/exec.native_write", 2, "exec", "chimi_exec_write", false),
    binding("std/exec.native_close_pipe", 1, "exec", "chimi_exec_close_pipe", false),
    binding("std/exec.native_allow_real_commands", 0, "exec", "chimi_exec_allow_real_commands", true),
    binding("std/terminal.native_is_terminal", 1, "terminal", "chimi_terminal_is_terminal", false),
    binding("std/terminal.native_size", 1, "terminal", "chimi_terminal_size", false),
    pure("std/path.from", 1, StandardFunction::PathFrom),
    pure("std/path.to_text", 1, StandardFunction::PathToText),
    host("std/env.host_variables", 0, StandardFunction::EnvHostVariables),
    host(
        "std/env.host_current_directory",
        0,
        StandardFunction::EnvHostCurrentDirectory,
    ),
    host("std/fs.host_open", 1, StandardFunction::FsHostOpen),
    host("std/fs.host_create", 2, StandardFunction::FsHostCreate),
    host("std/fs.host_read", 1, StandardFunction::FsHostRead),
    host("std/fs.host_write", 2, StandardFunction::FsHostWrite),
    host("std/fs.host_close", 1, StandardFunction::FsHostClose),
    host("std/fs.host_metadata", 1, StandardFunction::FsHostMetadata),
    host("std/fs.host_list", 1, StandardFunction::FsHostList),
    host("std/fs.host_create_directory", 1, StandardFunction::FsHostCreateDirectory),
    host("std/fs.host_remove", 1, StandardFunction::FsHostRemove),
    host("std/fs.host_rename", 2, StandardFunction::FsHostRename),
    test_only("std/fs.host_is_directory", 1, StandardFunction::FsHostIsDirectory),
    test_only("std/fs.host_temporary_directory", 0, StandardFunction::FsHostTemporaryDirectory),
    test_only("std/fs.test_root", 1, StandardFunction::FsTestRoot),
    host("std/signal.host_install", 0, StandardFunction::SignalHostInstall),
    host("std/signal.host_wait", 1, StandardFunction::SignalHostWait),
    host("std/log.host_minimum_level", 0, StandardFunction::LogHostMinimumLevel),
    host("std/log.host_write", 3, StandardFunction::LogHostWrite),
    host("std/metrics.host_enabled", 0, StandardFunction::MetricsHostEnabled),
    host("std/metrics.host_record", 4, StandardFunction::MetricsHostRecord),
];

/// A capability the runner closes on `main` with a provider written in the
/// standard module that declares it (host-capabilities-and-execution-properties.md,
/// "Runner-provided capabilities"). The need name is fixed, as `clock` is, so
/// every function that reads the capability names the same need.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RunnerProvider {
    pub(crate) interface: StandardType,
    pub(crate) need: &'static str,
    /// The qualified name of the private function that builds the provider.
    pub(crate) provider: &'static str,
}

pub(crate) const RUNNER_PROVIDERS: &[RunnerProvider] = &[
    RunnerProvider {
        interface: StandardType::Environment,
        need: "environment",
        provider: "std/env.host",
    },
    RunnerProvider {
        interface: StandardType::Files,
        need: "files",
        provider: "std/fs.host",
    },
    RunnerProvider {
        interface: StandardType::Signals,
        need: "signals",
        provider: "std/signal.host",
    },
    RunnerProvider {
        interface: StandardType::Commands,
        need: "commands",
        provider: "std/exec.system",
    },
    RunnerProvider {
        interface: StandardType::Terminal,
        need: "terminal",
        provider: "std/terminal.host",
    },
    RunnerProvider {
        interface: StandardType::Network,
        need: "network",
        provider: "std/net.system",
    },
    RunnerProvider {
        interface: StandardType::Log,
        need: "log",
        provider: "std/log.host",
    },
    RunnerProvider {
        interface: StandardType::Metrics,
        need: "metrics",
        provider: "std/metrics.host",
    },
];

/// Whether the runtime serves `interface` itself, one host function per method,
/// with no provider value: a method call on such a need with no provider in
/// scope runs the host function, and reading the need as a value builds a
/// record of those methods.
pub(crate) fn host_served_interface(interface: StandardType) -> bool {
    matches!(
        interface,
        StandardType::Console | StandardType::FileSystem | StandardType::Random
    )
}

/// The runner provider for a need on `interface` named `need`, if the runner
/// closes it.
pub(crate) fn runner_provider(interface: StandardType, need: &str) -> Option<&'static RunnerProvider> {
    RUNNER_PROVIDERS
        .iter()
        .find(|provider| provider.interface == interface && provider.need == need)
}

/// The qualified name of a host operation, which a constant must not call;
/// `None` for any other operation.
pub(crate) fn host_operation_name(function: StandardFunction) -> Option<&'static str> {
    OPERATIONS
        .iter()
        .find(|operation| {
            operation.binding.is_none()
                && operation.implementation == function
                && operation.class == OperationClass::Host
        })
        .map(|operation| operation.id)
}

pub(crate) const RUNTIME_KNOWN: &[RuntimeKnownDeclaration] = &[
    known_type("std/option.Option", StandardType::Option),
    known_constructor("std/option.Option.None", StandardConstructor::OptionNone),
    known_constructor("std/option.Option.Some", StandardConstructor::OptionSome),
    known_type("std/order.Ordering", StandardType::Ordering),
    known_interface("std/random.Random", StandardType::Random),
    known_constructor("std/order.Ordering.Less", StandardConstructor::OrderingLess),
    known_constructor(
        "std/order.Ordering.Equal",
        StandardConstructor::OrderingEqual,
    ),
    known_constructor(
        "std/order.Ordering.Greater",
        StandardConstructor::OrderingGreater,
    ),
    known_type("std/int.ParseError", StandardType::IntParseError),
    known_constructor(
        "std/int.ParseError.Empty",
        StandardConstructor::IntParseEmpty,
    ),
    known_constructor(
        "std/int.ParseError.InvalidDigit",
        StandardConstructor::IntParseInvalidDigit,
    ),
    known_constructor(
        "std/int.ParseError.OutOfRange",
        StandardConstructor::IntParseOutOfRange,
    ),
    native_type("std/map.Map", StandardType::Map),
    native_type("std/set.Set", StandardType::Set),
    native_type("std/decimal.Decimal", StandardType::Decimal),
    native_type("std/bytes.Bytes", StandardType::Bytes),
    native_type("std/path.Path", StandardType::Path),
    known_interface("std/env.Environment", StandardType::Environment),
    known_interface("std/fs.Files", StandardType::Files),
    known_interface("std/signal.Signals", StandardType::Signals),
    native_type("std/json.Number", StandardType::JsonNumber),
    native_type("std/cause.Cause", StandardType::Cause),
    native_type("std/binding_probe.Pipe", StandardType::BindingHandle),
    known_interface("std/net.Network", StandardType::Network),
    known_interface("std/exec.Commands", StandardType::Commands),
    known_interface("std/terminal.Terminal", StandardType::Terminal),
    known_interface("std/log.Log", StandardType::Log),
    known_interface("std/metrics.Metrics", StandardType::Metrics),
    native_type("std/exec.Builder", StandardType::BindingHandle),
    native_type("std/exec.Process", StandardType::BindingHandle),
    native_type("std/exec.Pipe", StandardType::BindingHandle),
    native_type("std/net.Socket", StandardType::BindingHandle),
    native_type("std/net.ListenSocket", StandardType::BindingHandle),
    native_type("std/tls.TlsSession", StandardType::BindingHandle),
    native_type("std/postgres.PostgresGate", StandardType::BindingHandle),
    native_type("std/sqlite.SqliteDb", StandardType::BindingHandle),
    native_type("std/sqlite.SqliteStatement", StandardType::BindingHandle),
    known_type("std/result.Result", StandardType::Result),
    known_constructor("std/result.Result.Ok", StandardConstructor::ResultOk),
    known_constructor("std/result.Result.Err", StandardConstructor::ResultErr),
    native_type("std/test.Recorder", StandardType::TestRecorder),
    native_type("std/test.Script", StandardType::TestScript),
    native_type("std/test.Mailbox", StandardType::TestMailbox),
    known_type("std/json.Value", StandardType::JsonValue),
    known_constructor("std/json.Value.Null", StandardConstructor::JsonNull),
    known_constructor("std/json.Value.Boolean", StandardConstructor::JsonBoolean),
    known_constructor("std/json.Value.Number", StandardConstructor::JsonNumber),
    known_constructor("std/json.Value.Text", StandardConstructor::JsonText),
    known_constructor("std/json.Value.Array", StandardConstructor::JsonArray),
    known_constructor("std/json.Value.Object", StandardConstructor::JsonObject),
    known_type("std/json.DecodeError", StandardType::DecodeError),
    known_constructor(
        "std/json.DecodeError.InvalidJson",
        StandardConstructor::DecodeInvalidJson,
    ),
    known_type(
        "std/json.ConversionError",
        StandardType::JsonConversionError,
    ),
    known_constructor(
        "std/json.ConversionError.NotInteger",
        StandardConstructor::JsonNotInteger,
    ),
    known_constructor(
        "std/json.ConversionError.OutOfRange",
        StandardConstructor::JsonOutOfRange,
    ),
];

/// A property a standard function needs of one of its generic parameters that
/// its source cannot state: a `native fn` has no body to infer it from, and a
/// Chimi function whose result shows the structural order of its input needs
/// that order without comparing anything itself.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StatedRequirement {
    /// The qualified name of the function.
    pub(crate) function: &'static str,
    /// The name of its generic parameter.
    pub(crate) generic: &'static str,
    pub(crate) property: ValueProperty,
}

const fn requires(
    function: &'static str,
    generic: &'static str,
    property: ValueProperty,
) -> StatedRequirement {
    StatedRequirement {
        function,
        generic,
        property,
    }
}

/// `Map` keys and `Set` items are equatable wherever a native operation takes
/// or makes one. Enumeration visits them in ascending structural order, which
/// the conventions expose only for an ordered type, so each module's `reduce`,
/// which every enumerating function calls, needs it (open question 2 of the
/// standard library conventions).
pub(crate) const STATED_REQUIREMENTS: &[StatedRequirement] = &[
    requires("std/map.empty", "K", ValueProperty::Equatable),
    requires("std/map.get", "K", ValueProperty::Equatable),
    requires("std/map.put", "K", ValueProperty::Equatable),
    requires("std/map.remove", "K", ValueProperty::Equatable),
    requires("std/map.reduce", "K", ValueProperty::Ordered),
    requires("std/set.empty", "T", ValueProperty::Equatable),
    requires("std/set.contains", "T", ValueProperty::Equatable),
    requires("std/set.add", "T", ValueProperty::Equatable),
    requires("std/set.remove", "T", ValueProperty::Equatable),
    requires("std/set.reduce", "T", ValueProperty::Ordered),
    requires("std/test.assert_equal", "T", ValueProperty::Equatable),
    requires("std/test.assert_not_equal", "T", ValueProperty::Equatable),
];

/// The requirements [`STATED_REQUIREMENTS`] states for the standard function
/// with this qualified name.
pub(crate) fn stated_requirements(
    function: &str,
) -> impl Iterator<Item = &'static StatedRequirement> {
    STATED_REQUIREMENTS
        .iter()
        .filter(move |requirement| requirement.function == function)
}

const fn native_type(id: &'static str, kind: StandardType) -> RuntimeKnownDeclaration {
    RuntimeKnownDeclaration {
        id,
        identity: RuntimeIdentity::NativeType(kind),
    }
}

const fn known_interface(id: &'static str, kind: StandardType) -> RuntimeKnownDeclaration {
    RuntimeKnownDeclaration {
        id,
        identity: RuntimeIdentity::Interface(kind),
    }
}

const fn known_type(id: &'static str, kind: StandardType) -> RuntimeKnownDeclaration {
    RuntimeKnownDeclaration {
        id,
        identity: RuntimeIdentity::Type(kind),
    }
}

const fn known_constructor(
    id: &'static str,
    constructor: StandardConstructor,
) -> RuntimeKnownDeclaration {
    RuntimeKnownDeclaration {
        id,
        identity: RuntimeIdentity::Constructor(constructor),
    }
}

/// The operation a `native fn` with this qualified name binds.
pub(crate) fn operation(id: &str) -> Option<&'static Operation> {
    OPERATIONS.iter().find(|operation| operation.id == id)
}

/// The runtime identity of the source declaration with this qualified name.
pub(crate) fn runtime_known(id: &str) -> Option<RuntimeIdentity> {
    RUNTIME_KNOWN
        .iter()
        .find(|declaration| declaration.id == id)
        .map(|declaration| declaration.identity)
}

/// How a whole standard tree disagrees with the registry.
#[derive(Debug, Default)]
pub struct StandardTreeVerification {
    /// What checking the tree on its own reports, which includes a `native fn`
    /// that names no operation (`N3104`), a wrong arity (`T4320`), and a
    /// runtime-known declaration of the wrong shape (`N3105`).
    pub diagnostics: Vec<Diagnostic>,
    /// The registry's side: each registered operation with no `native fn` or
    /// several, and each runtime-known declaration missing from its module.
    pub mismatches: Vec<String>,
}

impl StandardTreeVerification {
    pub fn matches(&self) -> bool {
        self.diagnostics.is_empty() && self.mismatches.is_empty()
    }
}

/// The whole-tree check (`chimi std check`): `sources` are every module of one
/// standard tree, which must type-check on its own, bind every registered
/// operation to exactly one `native fn`, and declare every runtime-known item
/// of the modules it holds.
///
/// A runtime-known declaration of a module the tree does not hold keeps its
/// synthesized built-in, so it is not required.
pub(crate) fn verify_tree(sources: Vec<ProjectSource>) -> StandardTreeVerification {
    let modules = sources
        .iter()
        .map(|source| source.module_path().to_owned())
        .collect::<HashSet<_>>();
    let typed = check_project(resolve_project_sources(sources));
    let resolved = &typed.resolved;
    let mut declarations = HashMap::<String, usize>::new();
    for symbol in &resolved.symbols {
        if resolved.native_operation(symbol.id).is_none() {
            continue;
        }
        let module = &resolved.semantic.modules
            [symbol.module_id.expect("native fn is a declaration").raw() as usize];
        *declarations
            .entry(format!("{}.{}", module.path, symbol.name))
            .or_default() += 1;
    }
    let mut mismatches = Vec::new();
    for operation in OPERATIONS {
        match declarations.get(operation.id).copied().unwrap_or(0) {
            0 => mismatches.push(format!(
                "registered operation {} has no `native fn` declaration",
                operation.id
            )),
            1 => {}
            count => mismatches.push(format!(
                "registered operation {} has {count} `native fn` declarations",
                operation.id
            )),
        }
    }
    // A stated requirement names a function of a module the tree holds; its
    // generic is checked when the function's contract is lowered.
    for stated in STATED_REQUIREMENTS {
        let module = stated
            .function
            .split_once('.')
            .map_or(stated.function, |(module, _)| module);
        if !modules.contains(module) {
            continue;
        }
        let declared = resolved.symbols.iter().any(|symbol| {
            symbol.module_id.is_some_and(|module_id| {
                let module = &resolved.semantic.modules[module_id.raw() as usize];
                format!("{}.{}", module.path, symbol.name) == stated.function
            })
        });
        if !declared {
            mismatches.push(format!(
                "stated requirement names {}, which is not declared",
                stated.function
            ));
        }
    }
    for entry in RUNTIME_KNOWN {
        let module = entry
            .id
            .split_once('.')
            .map_or(entry.id, |(module, _)| module);
        if !modules.contains(module) {
            continue;
        }
        let identity = match entry.identity {
            RuntimeIdentity::Type(kind)
            | RuntimeIdentity::NativeType(kind)
            | RuntimeIdentity::Interface(kind) => {
                StandardSymbol::Type(kind)
            }
            RuntimeIdentity::Constructor(constructor) => StandardSymbol::Constructor(constructor),
        };
        let declared = resolved.symbols.iter().any(|symbol| {
            symbol.module_id.is_some() && resolved.standard_symbol(symbol.id) == Some(identity)
        });
        if !declared {
            mismatches.push(format!(
                "runtime-known {} is not declared in source",
                entry.id
            ));
        }
    }
    StandardTreeVerification {
        diagnostics: typed.diagnostics,
        mismatches,
    }
}

#[cfg(test)]
mod tests {
    use super::verify_tree;
    use crate::standard_tree::{embedded_module_paths, standard_source};

    #[test]
    fn the_embedded_tree_matches_the_registry() {
        let sources = embedded_module_paths()
            .map(|path| standard_source(path).expect("listed module is embedded"))
            .collect();
        let verification = verify_tree(sources);
        assert!(
            verification.diagnostics.is_empty(),
            "{:?}",
            verification.diagnostics
        );
        assert!(
            verification.mismatches.is_empty(),
            "{:?}",
            verification.mismatches
        );
    }
}
