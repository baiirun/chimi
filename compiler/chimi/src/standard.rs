#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum StandardModule {
    Channel,
    Io,
    Json,
    Map,
    Task,
    Int,
    Float,
    Debug,
    Order,
    Range,
    Duration,
    Process,
    Supervisor,
    Clock,
}

impl StandardModule {
    pub(crate) const ALL: [Self; 14] = [
        Self::Channel,
        Self::Io,
        Self::Json,
        Self::Map,
        Self::Task,
        Self::Int,
        Self::Float,
        Self::Debug,
        Self::Order,
        Self::Range,
        Self::Duration,
        Self::Process,
        Self::Supervisor,
        Self::Clock,
    ];

    pub(crate) const fn path(self) -> &'static str {
        match self {
            Self::Channel => "std/channel",
            Self::Io => "std/io",
            Self::Json => "std/json",
            Self::Map => "std/map",
            Self::Task => "std/task",
            Self::Int => "std/int",
            Self::Float => "std/float",
            Self::Debug => "std/debug",
            Self::Order => "std/order",
            Self::Range => "std/range",
            Self::Duration => "std/duration",
            Self::Process => "std/process",
            Self::Supervisor => "std/supervisor",
            Self::Clock => "std/clock",
        }
    }

    /// The conventional import alias, as in `import std/map as Map`.
    pub(crate) fn qualifier(self) -> String {
        let name = self.path().trim_start_matches("std/");
        let mut characters = name.chars();
        characters.next().map_or_else(String::new, |first| {
            first.to_ascii_uppercase().to_string() + characters.as_str()
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum StandardType {
    Console,
    ConsoleError,
    FileError,
    FileSystem,
    Map,
    Path,
    Utf8Error,
    JsonValue,
    JsonNumber,
    DecodeError,
    NumberError,
    Option,
    ChannelSender,
    ChannelReceiver,
    ChannelCompletion,
    ChannelSendError,
    ChannelReceive,
    ChannelError,
    Task,
    IntParseError,
    FloatParseError,
    FloatConversionError,
    Ordering,
    /// A signed 64-bit count of nanoseconds (`std/duration`).
    Duration,
    DurationParseError,
    /// `std/process.Address<M>`: where messages of type `M` are routed.
    ProcessAddress,
    /// `std/process.Inbox<M>`: a process body's mailbox; task-local.
    ProcessInbox,
    /// `std/process.Spec<M, E>`: a body, a mailbox capacity, a shutdown bound,
    /// and an optional `process name`.
    ProcessSpec,
    ProcessSendError,
    SupervisorMember,
    SupervisorPolicy,
    SupervisorRestarts,
    SupervisorRestart,
    SupervisorRunning,
    SupervisorManagedProcess,
    SupervisorManagedSupervisor,
    SupervisorStartError,
    /// `std/clock.Clock`: the time provider the runner binds to a root `clock` need.
    Clock,
}

impl StandardType {
    /// Whether this standard type is an enum of expected failures, so its name
    /// denotes the set of its variants in a failure row, like a source enum.
    pub(crate) const fn is_failure_enum(self) -> bool {
        matches!(
            self,
            Self::ConsoleError
                | Self::FileError
                | Self::Utf8Error
                | Self::DecodeError
                | Self::NumberError
                | Self::ChannelSendError
                | Self::ChannelError
                | Self::IntParseError
                | Self::FloatParseError
                | Self::FloatConversionError
                | Self::DurationParseError
                | Self::ProcessSendError
                | Self::SupervisorStartError
        )
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Console => "Console",
            Self::ConsoleError => "ConsoleError",
            Self::FileError => "FileError",
            Self::FileSystem => "FileSystem",
            Self::Map => "Map",
            Self::Path => "Path",
            Self::Utf8Error => "Utf8Error",
            Self::JsonValue => "Value",
            Self::JsonNumber => "Number",
            Self::DecodeError => "DecodeError",
            Self::NumberError => "NumberError",
            Self::Option => "Option",
            Self::ChannelSender => "Sender",
            Self::ChannelReceiver => "Receiver",
            Self::ChannelCompletion => "Completion",
            Self::ChannelSendError => "SendError",
            Self::ChannelReceive => "Receive",
            Self::ChannelError => "ChannelError",
            Self::Task => "Task",
            Self::IntParseError | Self::FloatParseError | Self::DurationParseError => "ParseError",
            Self::FloatConversionError => "ConversionError",
            Self::Ordering => "Ordering",
            Self::Duration => "Duration",
            Self::ProcessAddress => "Address",
            Self::ProcessInbox => "Inbox",
            Self::ProcessSpec => "Spec",
            Self::ProcessSendError => "SendError",
            Self::SupervisorMember => "Member",
            Self::SupervisorPolicy => "Policy",
            Self::SupervisorRestarts => "Restarts",
            Self::SupervisorRestart => "Restart",
            Self::SupervisorRunning => "Running",
            Self::SupervisorManagedProcess => "ManagedProcess",
            Self::SupervisorManagedSupervisor => "ManagedSupervisor",
            Self::SupervisorStartError => "StartError",
            Self::Clock => "Clock",
        }
    }

    /// The equatable and ordered properties this module publishes for the type.
    /// Its private representation grants neither silently.
    pub(crate) const fn properties(self) -> StandardProperties {
        match self {
            Self::Console | Self::FileSystem | Self::Clock => {
                StandardProperties::Opaque("a provider")
            }
            Self::ProcessAddress
            | Self::ProcessInbox
            | Self::ProcessSpec
            | Self::SupervisorMember
            | Self::SupervisorPolicy
            | Self::SupervisorRestarts
            | Self::SupervisorRunning
            | Self::SupervisorManagedProcess
            | Self::SupervisorManagedSupervisor => {
                StandardProperties::Opaque("a process or supervisor capability")
            }
            Self::Task => StandardProperties::Opaque("a `Task` capability"),
            Self::ChannelSender | Self::ChannelReceiver | Self::ChannelCompletion => {
                StandardProperties::Opaque("a channel capability")
            }
            Self::Map => StandardProperties::Unordered,
            Self::JsonValue => StandardProperties::EquatableOnly(
                "`Value`, which is not ordered because an `Object` holds a `Map`",
            ),
            Self::Path => StandardProperties::EquatableOnly("`Path`, which is not ordered"),
            Self::JsonNumber
            | Self::Option
            | Self::ConsoleError
            | Self::FileError
            | Self::Utf8Error
            | Self::DecodeError
            | Self::NumberError
            | Self::ChannelSendError
            | Self::ChannelReceive
            | Self::ChannelError
            | Self::IntParseError
            | Self::FloatParseError
            | Self::FloatConversionError
            | Self::Ordering
            | Self::Duration
            | Self::DurationParseError
            | Self::ProcessSendError
            | Self::SupervisorRestart
            | Self::SupervisorStartError => StandardProperties::Structural,
        }
    }

    #[cfg(any(
        feature = "embedded-bytecode-prototype",
        feature = "native-backend",
        feature = "native-aot-prototype"
    ))]
    pub(crate) const fn runtime_id(self) -> u32 {
        self as u32
    }
}

/// How a standard type has the equatable and ordered properties.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StandardProperties {
    /// Both, whenever every type argument has them, like a source enum.
    Structural,
    /// Equatable when its key and value types are; never ordered (`Map`).
    Unordered,
    /// Always equatable, never ordered, for the stated reason.
    EquatableOnly(&'static str),
    /// Neither; the text says what the value is.
    Opaque(&'static str),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum StandardFunction {
    IoPath,
    ListFirst,
    ListLength,
    MapEmpty,
    MapGet,
    MapPut,
    MapUpdate,
    MapMapValues,
    JsonDecode,
    JsonEncode,
    JsonBoolean,
    JsonInt,
    JsonText,
    JsonArray,
    JsonObject,
    JsonNumberToInt,
    TextAppend,
    TextEqual,
    TaskStart,
    TaskResult,
    TaskCancel,
    ChannelSend,
    ChannelReceive,
    ChannelClose,
    ChannelAbort,
    ChannelBounded,
    IntParse,
    IntToText,
    IntAddChecked,
    IntSubtractChecked,
    IntMultiplyChecked,
    IntDivideChecked,
    IntRemainderChecked,
    IntModulo,
    FloatFromInt,
    FloatTruncate,
    FloatFloor,
    FloatCeiling,
    FloatRound,
    FloatToInt,
    FloatToText,
    FloatParse,
    /// `std/debug.render`: any value as one line of text in the debug format.
    DebugRender,
    /// `std/debug.render_indented`: the debug format with one part per line.
    DebugRenderIndented,
    OrderCompare,
    OrderMin,
    OrderMax,
    /// `std/map.from`: a map of the `(key, value)` pairs; a later pair wins.
    MapFrom,
    RangeStart,
    /// The exclusive end.
    RangeEnd,
    /// 0 for an empty range; defects when the count exceeds `Int`.
    RangeLength,
    RangeContains,
    DurationNanoseconds,
    DurationMicroseconds,
    DurationMilliseconds,
    DurationSeconds,
    DurationMinutes,
    DurationHours,
    DurationToNanoseconds,
    DurationToMicroseconds,
    DurationToMilliseconds,
    DurationToSeconds,
    DurationAdd,
    DurationSubtract,
    DurationMultiply,
    /// Truncates toward zero; a zero divisor defects.
    DurationDivide,
    DurationNegate,
    DurationAbsolute,
    /// The canonical text form, which `Debug.render` also uses.
    DurationToText,
    DurationParse,
    /// Declared before the process functions: declaration order is the
    /// native runtime's `CHIMI_INTRINSIC_*` order (`runtime_id`), which has
    /// `Clock.sleep` and no process operation yet.
    ClockSleep,
    ProcessSpec,
    ProcessRegister,
    ProcessReceive,
    ProcessSend,
    SupervisorProcess,
    SupervisorSupervisor,
    SupervisorPolicy,
    SupervisorRestarts,
    SupervisorStart,
    SupervisorStartProcess,
    SupervisorStartSupervisor,
    SupervisorAddress,
    SupervisorRemoveProcess,
    SupervisorRemoveSupervisor,
    SupervisorStop,
    SupervisorWait,
}

impl StandardFunction {
    pub(crate) const ALL: [Self; 83] = [
        Self::DebugRender,
        Self::DebugRenderIndented,
        Self::IntParse,
        Self::IntToText,
        Self::IntAddChecked,
        Self::IntSubtractChecked,
        Self::IntMultiplyChecked,
        Self::IntDivideChecked,
        Self::IntRemainderChecked,
        Self::IntModulo,
        Self::FloatFromInt,
        Self::FloatTruncate,
        Self::FloatFloor,
        Self::FloatCeiling,
        Self::FloatRound,
        Self::FloatToInt,
        Self::FloatToText,
        Self::FloatParse,
        Self::OrderCompare,
        Self::OrderMin,
        Self::OrderMax,
        Self::RangeStart,
        Self::RangeEnd,
        Self::RangeLength,
        Self::RangeContains,
        Self::DurationNanoseconds,
        Self::DurationMicroseconds,
        Self::DurationMilliseconds,
        Self::DurationSeconds,
        Self::DurationMinutes,
        Self::DurationHours,
        Self::DurationToNanoseconds,
        Self::DurationToMicroseconds,
        Self::DurationToMilliseconds,
        Self::DurationToSeconds,
        Self::DurationAdd,
        Self::DurationSubtract,
        Self::DurationMultiply,
        Self::DurationDivide,
        Self::DurationNegate,
        Self::DurationAbsolute,
        Self::DurationToText,
        Self::DurationParse,
        Self::IoPath,
        Self::MapEmpty,
        Self::MapGet,
        Self::MapPut,
        Self::MapUpdate,
        Self::MapMapValues,
        Self::MapFrom,
        Self::JsonDecode,
        Self::JsonEncode,
        Self::JsonBoolean,
        Self::JsonInt,
        Self::JsonText,
        Self::JsonArray,
        Self::JsonObject,
        Self::JsonNumberToInt,
        Self::TaskStart,
        Self::TaskResult,
        Self::TaskCancel,
        Self::ChannelSend,
        Self::ChannelReceive,
        Self::ChannelClose,
        Self::ChannelAbort,
        Self::ChannelBounded,
        Self::ProcessSpec,
        Self::ProcessRegister,
        Self::ProcessReceive,
        Self::ProcessSend,
        Self::SupervisorProcess,
        Self::SupervisorSupervisor,
        Self::SupervisorPolicy,
        Self::SupervisorRestarts,
        Self::SupervisorStart,
        Self::SupervisorStartProcess,
        Self::SupervisorStartSupervisor,
        Self::SupervisorAddress,
        Self::SupervisorRemoveProcess,
        Self::SupervisorRemoveSupervisor,
        Self::SupervisorStop,
        Self::SupervisorWait,
        Self::ClockSleep,
    ];

    pub(crate) const fn module(self) -> Option<StandardModule> {
        match self {
            Self::ProcessSpec
            | Self::ProcessRegister
            | Self::ProcessReceive
            | Self::ProcessSend => Some(StandardModule::Process),
            Self::SupervisorProcess
            | Self::SupervisorSupervisor
            | Self::SupervisorPolicy
            | Self::SupervisorRestarts
            | Self::SupervisorStart
            | Self::SupervisorStartProcess
            | Self::SupervisorStartSupervisor
            | Self::SupervisorAddress
            | Self::SupervisorRemoveProcess
            | Self::SupervisorRemoveSupervisor
            | Self::SupervisorStop
            | Self::SupervisorWait => Some(StandardModule::Supervisor),
            Self::ClockSleep => Some(StandardModule::Clock),
            Self::IoPath => Some(StandardModule::Io),
            Self::MapEmpty
            | Self::MapGet
            | Self::MapPut
            | Self::MapUpdate
            | Self::MapMapValues
            | Self::MapFrom => Some(StandardModule::Map),
            Self::JsonDecode
            | Self::JsonEncode
            | Self::JsonBoolean
            | Self::JsonInt
            | Self::JsonText
            | Self::JsonArray
            | Self::JsonObject
            | Self::JsonNumberToInt => Some(StandardModule::Json),
            Self::TaskStart | Self::TaskResult | Self::TaskCancel => Some(StandardModule::Task),
            Self::ChannelSend
            | Self::ChannelReceive
            | Self::ChannelClose
            | Self::ChannelAbort
            | Self::ChannelBounded => Some(StandardModule::Channel),
            Self::IntParse
            | Self::IntToText
            | Self::IntAddChecked
            | Self::IntSubtractChecked
            | Self::IntMultiplyChecked
            | Self::IntDivideChecked
            | Self::IntRemainderChecked
            | Self::IntModulo => Some(StandardModule::Int),
            Self::FloatFromInt
            | Self::FloatTruncate
            | Self::FloatFloor
            | Self::FloatCeiling
            | Self::FloatRound
            | Self::FloatToInt
            | Self::FloatToText
            | Self::FloatParse => Some(StandardModule::Float),
            Self::DebugRender | Self::DebugRenderIndented => Some(StandardModule::Debug),
            Self::OrderCompare | Self::OrderMin | Self::OrderMax => Some(StandardModule::Order),
            Self::RangeStart | Self::RangeEnd | Self::RangeLength | Self::RangeContains => {
                Some(StandardModule::Range)
            }
            Self::DurationNanoseconds
            | Self::DurationMicroseconds
            | Self::DurationMilliseconds
            | Self::DurationSeconds
            | Self::DurationMinutes
            | Self::DurationHours
            | Self::DurationToNanoseconds
            | Self::DurationToMicroseconds
            | Self::DurationToMilliseconds
            | Self::DurationToSeconds
            | Self::DurationAdd
            | Self::DurationSubtract
            | Self::DurationMultiply
            | Self::DurationDivide
            | Self::DurationNegate
            | Self::DurationAbsolute
            | Self::DurationToText
            | Self::DurationParse => Some(StandardModule::Duration),
            Self::ListFirst | Self::ListLength | Self::TextAppend | Self::TextEqual => None,
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::IoPath => "path",
            Self::ListFirst => "first",
            Self::ListLength => "length",
            Self::MapEmpty => "empty",
            Self::MapGet => "get",
            Self::MapPut => "put",
            Self::MapUpdate => "update",
            Self::MapMapValues => "map_values",
            Self::MapFrom => "from",
            Self::RangeStart => "start",
            Self::RangeEnd => "end",
            Self::RangeLength => "length",
            Self::RangeContains => "contains",
            Self::JsonDecode => "decode",
            Self::JsonEncode => "encode",
            Self::JsonBoolean => "boolean",
            Self::JsonInt => "int",
            Self::JsonText => "text",
            Self::JsonArray => "array",
            Self::JsonObject => "object",
            Self::JsonNumberToInt => "number_to_int",
            Self::TextAppend => "append",
            Self::TextEqual => "equal",
            Self::TaskStart => "start",
            Self::TaskResult => "result",
            Self::TaskCancel => "cancel",
            Self::ChannelSend => "send",
            Self::ChannelReceive => "receive",
            Self::ChannelClose => "close",
            Self::ChannelAbort => "abort",
            Self::ChannelBounded => "bounded",
            Self::IntParse | Self::FloatParse | Self::DurationParse => "parse",
            Self::IntToText | Self::FloatToText | Self::DurationToText => "to_text",
            Self::DurationNanoseconds => "nanoseconds",
            Self::DurationMicroseconds => "microseconds",
            Self::DurationMilliseconds => "milliseconds",
            Self::DurationSeconds => "seconds",
            Self::DurationMinutes => "minutes",
            Self::DurationHours => "hours",
            Self::DurationToNanoseconds => "to_nanoseconds",
            Self::DurationToMicroseconds => "to_microseconds",
            Self::DurationToMilliseconds => "to_milliseconds",
            Self::DurationToSeconds => "to_seconds",
            Self::DurationAdd => "add",
            Self::DurationSubtract => "subtract",
            Self::DurationMultiply => "multiply",
            Self::DurationDivide => "divide",
            Self::DurationNegate => "negate",
            Self::DurationAbsolute => "absolute",
            Self::IntAddChecked => "add_checked",
            Self::IntSubtractChecked => "subtract_checked",
            Self::IntMultiplyChecked => "multiply_checked",
            Self::IntDivideChecked => "divide_checked",
            Self::IntRemainderChecked => "remainder_checked",
            Self::IntModulo => "modulo",
            Self::FloatFromInt => "from_int",
            Self::FloatTruncate => "truncate",
            Self::FloatFloor => "floor",
            Self::FloatCeiling => "ceiling",
            Self::FloatRound => "round",
            Self::FloatToInt => "to_int",
            Self::DebugRender => "render",
            Self::DebugRenderIndented => "render_indented",
            Self::OrderCompare => "compare",
            Self::OrderMin => "min",
            Self::OrderMax => "max",
            Self::ProcessSpec => "spec",
            Self::ProcessRegister => "register",
            Self::ProcessReceive => "receive",
            Self::ProcessSend => "send",
            Self::SupervisorProcess => "process",
            Self::SupervisorSupervisor => "supervisor",
            Self::SupervisorPolicy => "policy",
            Self::SupervisorRestarts => "restarts",
            Self::SupervisorStart => "start",
            Self::SupervisorStartProcess => "start_process",
            Self::SupervisorStartSupervisor => "start_supervisor",
            Self::SupervisorAddress => "address",
            Self::SupervisorRemoveProcess => "remove_process",
            Self::SupervisorRemoveSupervisor => "remove_supervisor",
            Self::SupervisorStop => "stop",
            Self::SupervisorWait => "wait",
            Self::ClockSleep => "sleep",
        }
    }

    /// Whether this function needs the supervised process runtime (or its clock) to
    /// run. The process data constructors (`Process.spec`, `register`, and the
    /// `Supervisor` member, policy, and restart constructors) do not: they build
    /// plain values, so module constants may call them.
    pub(crate) const fn requires_process_runtime(self) -> bool {
        matches!(
            self,
            Self::ProcessReceive
                | Self::ProcessSend
                | Self::SupervisorStart
                | Self::SupervisorStartProcess
                | Self::SupervisorStartSupervisor
                | Self::SupervisorAddress
                | Self::SupervisorRemoveProcess
                | Self::SupervisorRemoveSupervisor
                | Self::SupervisorStop
                | Self::SupervisorWait
                | Self::ClockSleep
        )
    }

    /// Whether the native runtime runs this function on the fiber scheduler, so
    /// a program that reaches it runs its root as a scheduler task.
    #[cfg(any(
        feature = "embedded-bytecode-prototype",
        feature = "native-backend",
        feature = "native-aot-prototype"
    ))]
    pub(crate) const fn requires_scheduler(self) -> bool {
        matches!(
            self,
            Self::TaskStart
                | Self::TaskResult
                | Self::TaskCancel
                | Self::ChannelSend
                | Self::ChannelReceive
                | Self::ChannelClose
                | Self::ChannelAbort
                | Self::ChannelBounded
                | Self::ClockSleep
        )
    }

    /// The unit of a `Duration` constructor (`Duration.seconds`) or of a whole-unit
    /// conversion (`Duration.to_seconds`).
    pub(crate) const fn duration_unit(self) -> Option<(crate::duration::Unit, DurationUnitRole)> {
        use crate::duration::Unit;
        let (unit, role) = match self {
            Self::DurationNanoseconds => (Unit::Nanoseconds, DurationUnitRole::Constructor),
            Self::DurationMicroseconds => (Unit::Microseconds, DurationUnitRole::Constructor),
            Self::DurationMilliseconds => (Unit::Milliseconds, DurationUnitRole::Constructor),
            Self::DurationSeconds => (Unit::Seconds, DurationUnitRole::Constructor),
            Self::DurationMinutes => (Unit::Minutes, DurationUnitRole::Constructor),
            Self::DurationHours => (Unit::Hours, DurationUnitRole::Constructor),
            Self::DurationToNanoseconds => (Unit::Nanoseconds, DurationUnitRole::Conversion),
            Self::DurationToMicroseconds => (Unit::Microseconds, DurationUnitRole::Conversion),
            Self::DurationToMilliseconds => (Unit::Milliseconds, DurationUnitRole::Conversion),
            Self::DurationToSeconds => (Unit::Seconds, DurationUnitRole::Conversion),
            _ => return None,
        };
        Some((unit, role))
    }

    #[cfg(any(
        feature = "embedded-bytecode-prototype",
        feature = "native-backend",
        feature = "native-aot-prototype"
    ))]
    pub(crate) const fn runtime_id(self) -> u32 {
        self as u32
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DurationUnitRole {
    /// `fn(Int) -> Duration`.
    Constructor,
    /// `fn(Duration) -> Int`.
    Conversion,
}

/// A standard module value that is not a function: a module constant whose value
/// the registry defines.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum StandardConstant {
    /// `Duration.zero`; lowers to `Duration.nanoseconds(0)`.
    DurationZero,
}

impl StandardConstant {
    pub(crate) const ALL: [Self; 1] = [Self::DurationZero];

    pub(crate) const fn module(self) -> StandardModule {
        match self {
            Self::DurationZero => StandardModule::Duration,
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::DurationZero => "zero",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum StandardConstructor {
    ConsoleWriteFailed,
    FileClose,
    FileLineTooLong,
    FileOpen,
    FileRead,
    Utf8Invalid,
    OptionSome,
    OptionNone,
    JsonNull,
    JsonBoolean,
    JsonNumber,
    JsonText,
    JsonArray,
    JsonObject,
    DecodeInvalidJson,
    NumberNotInteger,
    NumberOutOfRange,
    ChannelSendClosed,
    ChannelSendAborted,
    ChannelReceived,
    ChannelDrained,
    ChannelAborted,
    IntParseEmpty,
    IntParseInvalidDigit,
    IntParseOutOfRange,
    FloatParseEmpty,
    FloatParseInvalid,
    FloatParseOutOfRange,
    FloatNotInteger,
    FloatOutOfRange,
    OrderingLess,
    OrderingEqual,
    OrderingGreater,
    DurationParseEmpty,
    DurationParseInvalid,
    DurationParseOutOfRange,
    ProcessSendFull,
    ProcessSendUnavailable,
    RestartNever,
    RestartOnDefect,
    StartCapacityReached,
    StartNameTaken,
    StartStopped,
}

impl StandardConstructor {
    pub(crate) const ALL: [Self; 43] = [
        Self::IntParseEmpty,
        Self::IntParseInvalidDigit,
        Self::IntParseOutOfRange,
        Self::FloatParseEmpty,
        Self::FloatParseInvalid,
        Self::FloatParseOutOfRange,
        Self::FloatNotInteger,
        Self::FloatOutOfRange,
        Self::OrderingLess,
        Self::OrderingEqual,
        Self::OrderingGreater,
        Self::DurationParseEmpty,
        Self::DurationParseInvalid,
        Self::DurationParseOutOfRange,
        Self::ConsoleWriteFailed,
        Self::FileClose,
        Self::FileLineTooLong,
        Self::FileOpen,
        Self::FileRead,
        Self::Utf8Invalid,
        Self::OptionSome,
        Self::OptionNone,
        Self::JsonNull,
        Self::JsonBoolean,
        Self::JsonNumber,
        Self::JsonText,
        Self::JsonArray,
        Self::JsonObject,
        Self::DecodeInvalidJson,
        Self::NumberNotInteger,
        Self::NumberOutOfRange,
        Self::ChannelSendClosed,
        Self::ChannelSendAborted,
        Self::ChannelReceived,
        Self::ChannelDrained,
        Self::ChannelAborted,
        Self::ProcessSendFull,
        Self::ProcessSendUnavailable,
        Self::RestartNever,
        Self::RestartOnDefect,
        Self::StartCapacityReached,
        Self::StartNameTaken,
        Self::StartStopped,
    ];

    pub(crate) const fn type_kind(self) -> StandardType {
        match self {
            Self::ConsoleWriteFailed => StandardType::ConsoleError,
            Self::FileClose | Self::FileLineTooLong | Self::FileOpen | Self::FileRead => {
                StandardType::FileError
            }
            Self::Utf8Invalid => StandardType::Utf8Error,
            Self::OptionSome | Self::OptionNone => StandardType::Option,
            Self::JsonNull
            | Self::JsonBoolean
            | Self::JsonNumber
            | Self::JsonText
            | Self::JsonArray
            | Self::JsonObject => StandardType::JsonValue,
            Self::DecodeInvalidJson => StandardType::DecodeError,
            Self::NumberNotInteger | Self::NumberOutOfRange => StandardType::NumberError,
            Self::ChannelSendClosed | Self::ChannelSendAborted => StandardType::ChannelSendError,
            Self::ChannelReceived | Self::ChannelDrained => StandardType::ChannelReceive,
            Self::ChannelAborted => StandardType::ChannelError,
            Self::IntParseEmpty | Self::IntParseInvalidDigit | Self::IntParseOutOfRange => {
                StandardType::IntParseError
            }
            Self::FloatParseEmpty | Self::FloatParseInvalid | Self::FloatParseOutOfRange => {
                StandardType::FloatParseError
            }
            Self::FloatNotInteger | Self::FloatOutOfRange => StandardType::FloatConversionError,
            Self::OrderingLess | Self::OrderingEqual | Self::OrderingGreater => {
                StandardType::Ordering
            }
            Self::DurationParseEmpty
            | Self::DurationParseInvalid
            | Self::DurationParseOutOfRange => StandardType::DurationParseError,
            Self::ProcessSendFull | Self::ProcessSendUnavailable => StandardType::ProcessSendError,
            Self::RestartNever | Self::RestartOnDefect => StandardType::SupervisorRestart,
            Self::StartCapacityReached | Self::StartNameTaken | Self::StartStopped => {
                StandardType::SupervisorStartError
            }
        }
    }

    pub(crate) const fn module(self) -> Option<StandardModule> {
        // Both `std/float` failure types own an `OutOfRange`, so neither is a module
        // member; only type-qualified access names them.
        if matches!(self, Self::FloatParseOutOfRange | Self::FloatOutOfRange) {
            return None;
        }
        match self.type_kind() {
            StandardType::IntParseError => Some(StandardModule::Int),
            StandardType::Ordering => Some(StandardModule::Order),
            StandardType::DurationParseError => Some(StandardModule::Duration),
            StandardType::FloatParseError | StandardType::FloatConversionError => {
                Some(StandardModule::Float)
            }
            StandardType::ConsoleError | StandardType::FileError | StandardType::Utf8Error => {
                Some(StandardModule::Io)
            }
            StandardType::JsonValue
            | StandardType::JsonNumber
            | StandardType::DecodeError
            | StandardType::NumberError => Some(StandardModule::Json),
            StandardType::Option => None,
            StandardType::ChannelSender
            | StandardType::ChannelReceiver
            | StandardType::ChannelCompletion
            | StandardType::ChannelReceive
            | StandardType::ChannelError => Some(StandardModule::Channel),
            // SendError owns both Closed and Aborted, but only type-qualified
            // access is valid: Channel.Aborted names the receive failure.
            StandardType::ChannelSendError => None,
            StandardType::Task => None,
            // Std docs write these type-qualified (`SendError.Full`,
            // `Supervisor.Restart.Never`), and `std/channel` already owns a
            // `SendError`, so none is a module member.
            StandardType::ProcessSendError
            | StandardType::SupervisorRestart
            | StandardType::SupervisorStartError => None,
            StandardType::ProcessAddress
            | StandardType::ProcessInbox
            | StandardType::ProcessSpec
            | StandardType::SupervisorMember
            | StandardType::SupervisorPolicy
            | StandardType::SupervisorRestarts
            | StandardType::SupervisorRunning
            | StandardType::SupervisorManagedProcess
            | StandardType::SupervisorManagedSupervisor
            | StandardType::Clock => None,
            StandardType::Console
            | StandardType::FileSystem
            | StandardType::Map
            | StandardType::Path
            | StandardType::Duration => None,
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::ConsoleWriteFailed => "WriteFailed",
            Self::FileClose => "Close",
            Self::FileLineTooLong => "LineTooLong",
            Self::FileOpen => "Open",
            Self::FileRead => "Read",
            Self::Utf8Invalid => "InvalidUtf8",
            Self::OptionSome => "Some",
            Self::OptionNone => "None",
            Self::JsonNull => "Null",
            Self::JsonBoolean => "Boolean",
            Self::JsonNumber => "Number",
            Self::JsonText => "Text",
            Self::JsonArray => "Array",
            Self::JsonObject => "Object",
            Self::DecodeInvalidJson => "InvalidJson",
            Self::NumberNotInteger => "NotInteger",
            Self::NumberOutOfRange => "OutOfRange",
            Self::ChannelSendClosed => "Closed",
            Self::ChannelSendAborted | Self::ChannelAborted => "Aborted",
            Self::ChannelReceived => "Received",
            Self::ChannelDrained => "Drained",
            Self::IntParseEmpty | Self::FloatParseEmpty | Self::DurationParseEmpty => "Empty",
            Self::IntParseInvalidDigit => "InvalidDigit",
            Self::FloatParseInvalid | Self::DurationParseInvalid => "Invalid",
            Self::IntParseOutOfRange
            | Self::FloatParseOutOfRange
            | Self::FloatOutOfRange
            | Self::DurationParseOutOfRange => "OutOfRange",
            Self::FloatNotInteger => "NotInteger",
            Self::OrderingLess => "Less",
            Self::OrderingEqual => "Equal",
            Self::OrderingGreater => "Greater",
            Self::ProcessSendFull => "Full",
            Self::ProcessSendUnavailable => "Unavailable",
            Self::RestartNever => "Never",
            Self::RestartOnDefect => "OnDefect",
            Self::StartCapacityReached => "CapacityReached",
            Self::StartNameTaken => "NameTaken",
            Self::StartStopped => "Stopped",
        }
    }

    /// Position among its type's variants in declaration order, which the derived
    /// order follows. `std/option` declares `None` before `Some`, unlike this
    /// enum's discriminants; every other type lists its variants here in order.
    pub(crate) fn ordinal(self) -> u32 {
        match self {
            Self::OptionNone => 0,
            Self::OptionSome => 1,
            _ => Self::ALL
                .iter()
                .filter(|other| {
                    other.type_kind() == self.type_kind() && (**other as u32) < self as u32
                })
                .count() as u32,
        }
    }

    pub(crate) const fn field_names(self) -> &'static [&'static str] {
        match self {
            Self::ConsoleWriteFailed => &["stream", "message"],
            Self::FileClose | Self::FileOpen | Self::FileRead => &["path", "message"],
            Self::FileLineTooLong => &["path", "limit"],
            Self::Utf8Invalid => &["path", "line"],
            Self::OptionSome => &["value"],
            Self::OptionNone | Self::JsonNull => &[],
            Self::JsonBoolean | Self::JsonNumber | Self::JsonText => &["value"],
            Self::JsonArray => &["values"],
            Self::JsonObject => &["fields"],
            Self::DecodeInvalidJson => &["message", "offset"],
            Self::NumberNotInteger | Self::NumberOutOfRange => &["value"],
            Self::ChannelSendClosed | Self::ChannelSendAborted | Self::ChannelReceived => {
                &["value"]
            }
            Self::ChannelDrained | Self::ChannelAborted => &[],
            Self::IntParseEmpty
            | Self::IntParseInvalidDigit
            | Self::IntParseOutOfRange
            | Self::FloatParseEmpty
            | Self::FloatParseInvalid
            | Self::FloatParseOutOfRange
            | Self::FloatNotInteger
            | Self::FloatOutOfRange
            | Self::OrderingLess
            | Self::OrderingEqual
            | Self::OrderingGreater
            | Self::DurationParseEmpty
            | Self::DurationParseInvalid
            | Self::DurationParseOutOfRange => &[],
            Self::ProcessSendFull | Self::ProcessSendUnavailable => &["message"],
            Self::StartNameTaken => &["name"],
            Self::RestartNever
            | Self::RestartOnDefect
            | Self::StartCapacityReached
            | Self::StartStopped => &[],
        }
    }

    #[cfg(any(
        feature = "embedded-bytecode-prototype",
        feature = "native-backend",
        feature = "native-aot-prototype"
    ))]
    pub(crate) const fn runtime_id(self) -> u32 {
        self as u32
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum StandardSymbol {
    Module(StandardModule),
    Type(StandardType),
    Function(StandardFunction),
    Constructor(StandardConstructor),
    Generic(StandardGeneric),
    Constant(StandardConstant),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum StandardGenericKind {
    Value,
    FailureRow,
    FailureSet,
    NeedsRow,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum StandardGeneric {
    FileState,
    FileFailures,
    FileNeeds,
    ListFirstValue,
    ListLengthValue,
    OptionValue,
    MapEmptyKey,
    MapEmptyValue,
    MapGetKey,
    MapGetValue,
    MapPutKey,
    MapPutValue,
    MapUpdateKey,
    MapUpdateValue,
    MapUpdateFailures,
    MapUpdateNeeds,
    MapValuesKey,
    MapValuesInput,
    MapValuesOutput,
    MapValuesFailures,
    MapValuesNeeds,
    TaskValue,
    TaskFailures,
    TaskNeeds,
    ChannelValue,
    DebugRenderValue,
    DebugRenderIndentedValue,
    OrderCompareValue,
    OrderMinValue,
    OrderMaxValue,
    MapFromKey,
    MapFromValue,
    ProcessSpecMessage,
    ProcessSpecFailures,
    ProcessSpecNeeds,
    ProcessRegisterMessage,
    ProcessRegisterFailures,
    ProcessReceiveMessage,
    ProcessSendMessage,
    SupervisorProcessMessage,
    SupervisorProcessFailures,
    SupervisorStartProcessMessage,
    SupervisorStartProcessFailures,
    SupervisorAddressMessage,
    SupervisorRemoveMessage,
}

impl StandardGeneric {
    pub(crate) const ALL: [Self; 45] = [
        Self::DebugRenderValue,
        Self::DebugRenderIndentedValue,
        Self::FileState,
        Self::FileFailures,
        Self::FileNeeds,
        Self::ListFirstValue,
        Self::ListLengthValue,
        Self::OptionValue,
        Self::MapEmptyKey,
        Self::MapEmptyValue,
        Self::MapGetKey,
        Self::MapGetValue,
        Self::MapPutKey,
        Self::MapPutValue,
        Self::MapUpdateKey,
        Self::MapUpdateValue,
        Self::MapUpdateFailures,
        Self::MapUpdateNeeds,
        Self::MapValuesKey,
        Self::MapValuesInput,
        Self::MapValuesOutput,
        Self::MapValuesFailures,
        Self::MapValuesNeeds,
        Self::TaskValue,
        Self::TaskFailures,
        Self::TaskNeeds,
        Self::ChannelValue,
        Self::OrderCompareValue,
        Self::OrderMinValue,
        Self::OrderMaxValue,
        Self::MapFromKey,
        Self::MapFromValue,
        Self::ProcessSpecMessage,
        Self::ProcessSpecFailures,
        Self::ProcessSpecNeeds,
        Self::ProcessRegisterMessage,
        Self::ProcessRegisterFailures,
        Self::ProcessReceiveMessage,
        Self::ProcessSendMessage,
        Self::SupervisorProcessMessage,
        Self::SupervisorProcessFailures,
        Self::SupervisorStartProcessMessage,
        Self::SupervisorStartProcessFailures,
        Self::SupervisorAddressMessage,
        Self::SupervisorRemoveMessage,
    ];

    /// The name shown in contracts and diagnostics. Names repeat across
    /// functions; each parameter is a distinct symbol, so they never collide.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::FileState => "S",
            Self::FileFailures => "E",
            Self::FileNeeds => "R",
            Self::ListFirstValue => "T",
            Self::ListLengthValue => "T",
            Self::OptionValue => "T",
            Self::MapEmptyKey => "K",
            Self::MapEmptyValue => "V",
            Self::MapGetKey => "K",
            Self::MapGetValue | Self::MapFromValue => "V",
            Self::MapPutKey => "K",
            Self::MapPutValue => "V",
            Self::MapUpdateKey => "K",
            Self::MapUpdateValue => "V",
            Self::MapUpdateFailures => "E",
            Self::MapUpdateNeeds => "R",
            Self::MapValuesKey | Self::MapFromKey => "K",
            Self::MapValuesInput => "A",
            Self::MapValuesOutput => "B",
            Self::MapValuesFailures => "E",
            Self::MapValuesNeeds => "R",
            Self::TaskValue => "T",
            Self::TaskFailures => "E",
            Self::TaskNeeds => "R",
            Self::ChannelValue => "T",
            Self::DebugRenderValue | Self::DebugRenderIndentedValue => "T",
            Self::OrderCompareValue | Self::OrderMinValue | Self::OrderMaxValue => "A",
            Self::ProcessSpecMessage
            | Self::ProcessRegisterMessage
            | Self::ProcessReceiveMessage
            | Self::ProcessSendMessage
            | Self::SupervisorProcessMessage
            | Self::SupervisorStartProcessMessage
            | Self::SupervisorAddressMessage
            | Self::SupervisorRemoveMessage => "M",
            Self::ProcessSpecFailures
            | Self::ProcessRegisterFailures
            | Self::SupervisorProcessFailures
            | Self::SupervisorStartProcessFailures => "E",
            Self::ProcessSpecNeeds => "R",
        }
    }

    pub(crate) const fn kind(self) -> StandardGenericKind {
        match self {
            Self::FileFailures | Self::MapUpdateFailures | Self::MapValuesFailures => {
                StandardGenericKind::FailureRow
            }
            Self::TaskFailures
            | Self::ProcessSpecFailures
            | Self::ProcessRegisterFailures
            | Self::SupervisorProcessFailures
            | Self::SupervisorStartProcessFailures => StandardGenericKind::FailureSet,
            Self::FileNeeds
            | Self::MapUpdateNeeds
            | Self::MapValuesNeeds
            | Self::ProcessSpecNeeds => StandardGenericKind::NeedsRow,
            _ => StandardGenericKind::Value,
        }
    }
}
