#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum StandardModule {
    Channel,
    Io,
    Task,
    Int,
    Float,
    Order,
    Range,
    Duration,
    Process,
    Supervisor,
    Clock,
    Select,
    Broadcast,
    Watch,
}

impl StandardModule {
    pub(crate) const ALL: [Self; 14] = [
        Self::Channel,
        Self::Io,
        Self::Task,
        Self::Int,
        Self::Float,
        Self::Order,
        Self::Range,
        Self::Duration,
        Self::Process,
        Self::Supervisor,
        Self::Clock,
        Self::Select,
        Self::Broadcast,
        Self::Watch,
    ];

    pub(crate) const fn path(self) -> &'static str {
        match self {
            Self::Channel => "std/channel",
            Self::Io => "std/io",
            Self::Task => "std/task",
            Self::Int => "std/int",
            Self::Float => "std/float",
            Self::Order => "std/order",
            Self::Range => "std/range",
            Self::Duration => "std/duration",
            Self::Process => "std/process",
            Self::Supervisor => "std/supervisor",
            Self::Clock => "std/clock",
            Self::Select => "std/select",
            Self::Broadcast => "std/broadcast",
            Self::Watch => "std/watch",
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
    JsonConversionError,
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
    /// `std/random.Random`: the random source the runner binds to a root
    /// `random` need; declared in Chimi source, unlike `Clock`.
    Random,
    // `native type`s that only the embedded standard tree declares
    // (`operations.rs`); no built-in module synthesizes them.
    /// `std/set.Set<T>`.
    Set,
    /// `std/decimal.Decimal`: an exact base-10 number.
    Decimal,
    /// `std/bytes.Bytes`: immutable binary data.
    Bytes,
    /// `std/clock.Instant`: signed nanoseconds on the execution's monotonic
    /// time source, meaningful only within the execution that read it.
    Instant,
    /// `std/clock.Timestamp`: signed nanoseconds since the Unix epoch, wall time.
    Timestamp,
    /// `std/task.Timeout`: the expected failure of `Task.timeout` and `Task.deadline`.
    Timeout,
    /// `std/select.Event<A, F>`: one wait that produces an `A` or fails with `F`;
    /// task-local.
    SelectEvent,
    /// `std/process.Monitor`: a watch one incarnation set on another.
    ProcessMonitor,
    /// `std/process.Down`: how a monitored incarnation ended.
    ProcessDown,
    /// `std/process.HandoffSpec<A, M, E>`: a body that takes an argument of type
    /// `A`, started once by `Supervisor.start_handoff` and never restarted.
    ProcessHandoffSpec,
    /// `std/supervisor.HandoffError<A>`: a refused handoff start, which hands
    /// the argument back.
    SupervisorHandoffError,
    /// `std/cause.Cause`: the diagnostic record beside a failure, a defect, or a
    /// cancellation.
    Cause,
    /// `std/broadcast.Publisher<T>`: publishes to and subscribes to one
    /// broadcast; shareable.
    BroadcastPublisher,
    /// `std/watch.Publisher<T>`: publishes to and subscribes to one watch;
    /// shareable.
    WatchPublisher,
    /// `std/watch.Subscription<T>`: receives a watch's latest unseen value;
    /// exclusive, like a Receiver.
    WatchSubscription,
    /// `std/result.Result<T, E>`, which `Task.results` builds. Declared only in
    /// source; `std/task` loads `std/result`.
    Result,
    /// `std/test.Recorder<T>` and `std/test.Script<T>`: test-only handles to
    /// logs the test execution owns (testing.md, "Recorders and scripted fakes").
    TestRecorder,
    TestScript,
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
                | Self::JsonConversionError
                | Self::ChannelSendError
                | Self::ChannelError
                | Self::IntParseError
                | Self::FloatParseError
                | Self::FloatConversionError
                | Self::DurationParseError
                | Self::ProcessSendError
                | Self::SupervisorStartError
                | Self::SupervisorHandoffError
                | Self::Timeout
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
            Self::JsonConversionError => "ConversionError",
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
            Self::Random => "Random",
            Self::Set => "Set",
            Self::Decimal => "Decimal",
            Self::Bytes => "Bytes",
            Self::Instant => "Instant",
            Self::Timestamp => "Timestamp",
            Self::Timeout => "Timeout",
            Self::SelectEvent => "Event",
            Self::ProcessMonitor => "Monitor",
            Self::ProcessDown => "Down",
            Self::ProcessHandoffSpec => "HandoffSpec",
            Self::SupervisorHandoffError => "HandoffError",
            Self::Cause => "Cause",
            Self::BroadcastPublisher | Self::WatchPublisher => "Publisher",
            Self::WatchSubscription => "Subscription",
            Self::Result => "Result",
            Self::TestRecorder => "Recorder",
            Self::TestScript => "Script",
        }
    }

    /// The equatable and ordered properties this module publishes for the type.
    /// Its private representation grants neither silently.
    pub(crate) const fn properties(self) -> StandardProperties {
        match self {
            Self::Console | Self::FileSystem | Self::Clock | Self::Random => {
                StandardProperties::Opaque("a provider")
            }
            Self::ProcessAddress
            | Self::ProcessInbox
            | Self::ProcessSpec
            | Self::ProcessHandoffSpec
            | Self::ProcessMonitor
            | Self::SupervisorMember
            | Self::SupervisorPolicy
            | Self::SupervisorRestarts
            | Self::SupervisorRunning
            | Self::SupervisorManagedProcess
            | Self::SupervisorManagedSupervisor => {
                StandardProperties::Opaque("a process or supervisor capability")
            }
            Self::Task => StandardProperties::Opaque("a `Task` capability"),
            Self::TestRecorder => StandardProperties::Opaque("a `Test.Recorder`"),
            Self::TestScript => StandardProperties::Opaque("a `Test.Script`"),
            Self::SelectEvent => StandardProperties::Opaque("a `Select.Event`"),
            Self::ChannelSender
            | Self::ChannelReceiver
            | Self::ChannelCompletion
            | Self::BroadcastPublisher
            | Self::WatchPublisher
            | Self::WatchSubscription => StandardProperties::Opaque("a channel capability"),
            Self::Map | Self::Set => StandardProperties::Unordered,
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
            | Self::JsonConversionError
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
            | Self::ProcessDown
            | Self::SupervisorRestart
            | Self::SupervisorStartError
            | Self::SupervisorHandoffError
            | Self::Decimal
            | Self::Bytes
            | Self::Instant
            | Self::Timestamp
            | Self::Timeout
            | Self::Result => StandardProperties::Structural,
            Self::Cause => StandardProperties::Opaque(
                "a `Cause`, diagnostic data that equality and order never read",
            ),
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
    /// native runtime's `CHIMI_INTRINSIC_*` order (`runtime_id`), and the
    /// process intrinsics follow `CHIMI_INTRINSIC_CLOCK_SLEEP` there.
    ClockSleep,
    // The rest of `std/clock`, then `Task.timeout` and `Task.deadline`, which
    // run on the same scheduler clock; they precede the process functions for
    // the same reason.
    /// `Clock.now`: the current `Instant` of the clock provider.
    ClockNow,
    /// `Clock.timestamp`: the current wall time of the clock provider.
    ClockTimestamp,
    /// `Clock.since(start)`: `Clock.between(start, Clock.now())`.
    ClockSince,
    ClockSleepUntil,
    /// Defects on overflow of the `Instant` range.
    ClockAdd,
    /// `end - start`; defects on overflow of the `Duration` range.
    ClockBetween,
    /// RFC 3339 in UTC with the fraction trimmed of trailing zeros.
    ClockTimestampToText,
    ClockUnixNanoseconds,
    ClockFromUnixNanoseconds,
    ClockTimestampBetween,
    /// A `Clock` whose `timestamp` starts at the given value and advances with
    /// the wrapped clock's `now`.
    ClockStartingAt,
    /// `Task.timeout(operation, limit)`: runs the operation in a cancellation
    /// scope of the calling task.
    TaskTimeout,
    /// `Task.deadline(operation, at)`.
    TaskDeadline,
    /// `Task.all(operations)`: every success in input order, or the first
    /// expected failure after the other children drain.
    TaskAll,
    /// `Task.results(operations)`: every outcome as a `Result`, in input order.
    TaskResults,
    /// `Task.race(operations)`: the first outcome after the others drain.
    TaskRace,
    // `std/select`, in `CHIMI_INTRINSIC_SELECT_*` order. Building an event
    // parks nothing and consumes nothing; `Select.first` waits.
    SelectReceive,
    SelectInbox,
    SelectTask,
    /// Measures its `Duration` from the start of the `Select.first` call.
    SelectAfter,
    SelectAt,
    SelectFirst,
    // The channel variants (G29), in `CHIMI_INTRINSIC_*` order.
    /// `Channel.add_receiver(receiver)`: another receiver of the same queue.
    ChannelAddReceiver,
    BroadcastBounded,
    /// Returns how many subscriptions the publish reached.
    BroadcastPublish,
    BroadcastSubscribe,
    WatchStartingWith,
    WatchPublish,
    WatchSubscribe,
    WatchReceive,
    SelectWatch,
    ProcessSpec,
    ProcessRegister,
    ProcessReceive,
    ProcessSend,
    /// `Process.address(inbox)`: the owning incarnation's direct address.
    ProcessOwnAddress,
    /// `Process.queued(inbox)`: messages and notices waiting in the mailbox.
    ProcessQueued,
    /// `Process.capacity(inbox)`: the spec's mailbox capacity.
    ProcessCapacity,
    ProcessMonitor,
    ProcessUnmonitor,
    /// `Process.handoff_spec(body, capacity, shutdown_within)`.
    ProcessHandoffSpec,
    SupervisorProcess,
    SupervisorSupervisor,
    SupervisorPolicy,
    SupervisorRestarts,
    SupervisorStart,
    SupervisorStartProcess,
    SupervisorStartSupervisor,
    /// `Supervisor.start_handoff(running, spec, argument)`: starts one
    /// never-restarted incarnation that owns `argument`.
    SupervisorStartHandoff,
    SupervisorAddress,
    SupervisorRemoveProcess,
    SupervisorRemoveSupervisor,
    SupervisorStop,
    SupervisorWait,
    /// `std/debug.crash`: ends the unit of work with a defect carrying the message.
    DebugCrash,
    // Operations that only a `native fn` in the embedded standard tree binds
    // (`operations.rs`); no built-in module exposes them. They follow every
    // other function, because declaration order is the native runtime's
    // `CHIMI_INTRINSIC_*` order.
    OptionUnwrapOrFail,
    TextConcat,
    TextByteLength,
    TextByteSlice,
    TextContains,
    TextStartsWith,
    TextEndsWith,
    TextSplit,
    TextReplace,
    TextTrimStart,
    TextTrimEnd,
    TextLowercase,
    TextUppercase,
    TextGraphemes,
    ListGet,
    ListPush,
    ListSlice,
    ListSet,
    // The native cores of the phase 2 ports (steps 5 and 6): `std/map`,
    // `std/set`, `std/json`, and `std/decimal`. Each module's variants stay
    // contiguous: the native runtime dispatches by range.
    MapRemove,
    MapSize,
    MapReduceEntries,
    SetEmpty,
    SetContains,
    SetAdd,
    SetRemove,
    SetSize,
    SetReduceItems,
    JsonDecode,
    JsonEncode,
    JsonInt,
    JsonNumberToInt,
    JsonNumberToDecimal,
    DecimalScaled,
    DecimalRead,
    DecimalToText,
    DecimalAddChecked,
    DecimalSubtractChecked,
    DecimalMultiplyChecked,
    DecimalDivideRounded,
    DecimalToFloat,
    // The native core of `std/bytes`.
    BytesFrom,
    BytesLength,
    BytesGet,
    BytesIndexOf,
    BytesConcat,
    BytesSlice,
    BytesToList,
    BytesFromText,
    BytesToTextLossy,
    BytesValidUtf8Length,
    BytesReadIntCode,
    BytesIntBytes,
    // `std/embed`: lowering replaces the path argument with the file's contents
    // (`text`) or their lowercase hex (`bytes`).
    EmbedText,
    EmbedBytes,
    // The native core of `std/cause`; the readers of a code map it to an enum
    // in source.
    CauseContext,
    CauseOf,
    CauseKindCode,
    CauseSummary,
    CauseFrames,
    CauseCausedBy,
    CauseSuppressed,
    CauseReasonCode,
    CauseToText,
    // `std/test` assertions. Each defects at its own call, so the defect's node
    // is the caller's location (testing.md, Failure reports).
    TestAssertEqual,
    TestAssertNotEqual,
    TestAssert,
    TestFail,
    /// `std/resource.scoped`: acquire, use, and release with release masked
    /// against cancellation.
    ResourceScoped,
    // Test-only (testing.md, Defects): T4401 when reachable from `main`.
    TestCaptureDefect,
    TestRecorder,
    TestRecord,
    TestRecorded,
    TestScript,
    TestNext,
}

impl StandardFunction {
    pub(crate) const ALL: [Self; 105] = [
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
        Self::ProcessOwnAddress,
        Self::ProcessQueued,
        Self::ProcessCapacity,
        Self::ProcessMonitor,
        Self::ProcessUnmonitor,
        Self::ProcessHandoffSpec,
        Self::SupervisorProcess,
        Self::SupervisorSupervisor,
        Self::SupervisorPolicy,
        Self::SupervisorRestarts,
        Self::SupervisorStart,
        Self::SupervisorStartProcess,
        Self::SupervisorStartSupervisor,
        Self::SupervisorStartHandoff,
        Self::SupervisorAddress,
        Self::SupervisorRemoveProcess,
        Self::SupervisorRemoveSupervisor,
        Self::SupervisorStop,
        Self::SupervisorWait,
        Self::ClockSleep,
        Self::ClockNow,
        Self::ClockTimestamp,
        Self::ClockSince,
        Self::ClockSleepUntil,
        Self::ClockAdd,
        Self::ClockBetween,
        Self::ClockTimestampToText,
        Self::ClockUnixNanoseconds,
        Self::ClockFromUnixNanoseconds,
        Self::ClockTimestampBetween,
        Self::ClockStartingAt,
        Self::TaskTimeout,
        Self::TaskDeadline,
        Self::TaskAll,
        Self::TaskResults,
        Self::TaskRace,
        Self::SelectReceive,
        Self::SelectInbox,
        Self::SelectTask,
        Self::SelectAfter,
        Self::SelectAt,
        Self::SelectFirst,
        Self::ChannelAddReceiver,
        Self::BroadcastBounded,
        Self::BroadcastPublish,
        Self::BroadcastSubscribe,
        Self::WatchStartingWith,
        Self::WatchPublish,
        Self::WatchSubscribe,
        Self::WatchReceive,
        Self::SelectWatch,
    ];

    pub(crate) const fn module(self) -> Option<StandardModule> {
        match self {
            Self::BroadcastBounded | Self::BroadcastPublish | Self::BroadcastSubscribe => {
                Some(StandardModule::Broadcast)
            }
            Self::WatchStartingWith
            | Self::WatchPublish
            | Self::WatchSubscribe
            | Self::WatchReceive => Some(StandardModule::Watch),
            Self::ChannelAddReceiver => Some(StandardModule::Channel),
            Self::SelectWatch => Some(StandardModule::Select),
            Self::ProcessSpec
            | Self::ProcessRegister
            | Self::ProcessReceive
            | Self::ProcessSend
            | Self::ProcessOwnAddress
            | Self::ProcessQueued
            | Self::ProcessCapacity
            | Self::ProcessMonitor
            | Self::ProcessUnmonitor
            | Self::ProcessHandoffSpec => Some(StandardModule::Process),
            Self::SupervisorProcess
            | Self::SupervisorSupervisor
            | Self::SupervisorPolicy
            | Self::SupervisorRestarts
            | Self::SupervisorStart
            | Self::SupervisorStartProcess
            | Self::SupervisorStartSupervisor
            | Self::SupervisorStartHandoff
            | Self::SupervisorAddress
            | Self::SupervisorRemoveProcess
            | Self::SupervisorRemoveSupervisor
            | Self::SupervisorStop
            | Self::SupervisorWait => Some(StandardModule::Supervisor),
            Self::ClockSleep
            | Self::ClockNow
            | Self::ClockTimestamp
            | Self::ClockSince
            | Self::ClockSleepUntil
            | Self::ClockAdd
            | Self::ClockBetween
            | Self::ClockTimestampToText
            | Self::ClockUnixNanoseconds
            | Self::ClockFromUnixNanoseconds
            | Self::ClockTimestampBetween
            | Self::ClockStartingAt => Some(StandardModule::Clock),
            Self::IoPath => Some(StandardModule::Io),
            Self::SelectReceive
            | Self::SelectInbox
            | Self::SelectTask
            | Self::SelectAfter
            | Self::SelectAt
            | Self::SelectFirst => Some(StandardModule::Select),
            Self::TaskStart
            | Self::TaskResult
            | Self::TaskCancel
            | Self::TaskTimeout
            | Self::TaskDeadline
            | Self::TaskAll
            | Self::TaskResults
            | Self::TaskRace => Some(StandardModule::Task),
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
            Self::DebugRender
            | Self::DebugRenderIndented
            | Self::DebugCrash
            | Self::ListFirst
            | Self::ListLength
            | Self::TextAppend
            | Self::TextEqual
            | Self::OptionUnwrapOrFail
            | Self::TextConcat
            | Self::TextByteLength
            | Self::TextByteSlice
            | Self::TextContains
            | Self::TextStartsWith
            | Self::TextEndsWith
            | Self::TextSplit
            | Self::TextReplace
            | Self::TextTrimStart
            | Self::TextTrimEnd
            | Self::TextLowercase
            | Self::TextUppercase
            | Self::TextGraphemes
            | Self::ListGet
            | Self::ListPush
            | Self::ListSlice
            | Self::ListSet
            | Self::MapEmpty
            | Self::MapGet
            | Self::MapPut
            | Self::MapRemove
            | Self::MapSize
            | Self::MapReduceEntries
            | Self::SetEmpty
            | Self::SetContains
            | Self::SetAdd
            | Self::SetRemove
            | Self::SetSize
            | Self::SetReduceItems
            | Self::JsonDecode
            | Self::JsonEncode
            | Self::JsonInt
            | Self::JsonNumberToInt
            | Self::JsonNumberToDecimal
            | Self::DecimalScaled
            | Self::DecimalRead
            | Self::DecimalToText
            | Self::DecimalAddChecked
            | Self::DecimalSubtractChecked
            | Self::DecimalMultiplyChecked
            | Self::DecimalDivideRounded
            | Self::DecimalToFloat
            | Self::BytesFrom
            | Self::BytesLength
            | Self::BytesGet
            | Self::BytesIndexOf
            | Self::BytesConcat
            | Self::BytesSlice
            | Self::BytesToList
            | Self::BytesFromText
            | Self::BytesToTextLossy
            | Self::BytesValidUtf8Length
            | Self::BytesReadIntCode
            | Self::BytesIntBytes
            | Self::EmbedText
            | Self::EmbedBytes
            | Self::CauseContext
            | Self::CauseOf
            | Self::CauseKindCode
            | Self::CauseSummary
            | Self::CauseFrames
            | Self::CauseCausedBy
            | Self::CauseSuppressed
            | Self::CauseReasonCode
            | Self::CauseToText => None,
            Self::TestAssertEqual
            | Self::TestAssertNotEqual
            | Self::TestAssert
            | Self::TestFail => None,
            Self::ResourceScoped => None,
            Self::TestCaptureDefect
            | Self::TestRecorder
            | Self::TestRecord
            | Self::TestRecorded
            | Self::TestScript
            | Self::TestNext => None,
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
            Self::RangeStart => "start",
            Self::RangeEnd => "end",
            Self::RangeLength => "length",
            Self::RangeContains => "contains",
            Self::JsonDecode => "decode",
            Self::JsonEncode => "encode",
            Self::JsonInt => "int",
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
            Self::ProcessOwnAddress => "address",
            Self::ProcessQueued => "queued",
            Self::ProcessCapacity => "capacity",
            Self::ProcessMonitor => "monitor",
            Self::ProcessUnmonitor => "unmonitor",
            Self::ProcessHandoffSpec => "handoff_spec",
            Self::SupervisorProcess => "process",
            Self::SupervisorSupervisor => "supervisor",
            Self::SupervisorPolicy => "policy",
            Self::SupervisorRestarts => "restarts",
            Self::SupervisorStart => "start",
            Self::SupervisorStartProcess => "start_process",
            Self::SupervisorStartSupervisor => "start_supervisor",
            Self::SupervisorStartHandoff => "start_handoff",
            Self::SupervisorAddress => "address",
            Self::SupervisorRemoveProcess => "remove_process",
            Self::SupervisorRemoveSupervisor => "remove_supervisor",
            Self::SupervisorStop => "stop",
            Self::SupervisorWait => "wait",
            Self::ClockSleep => "sleep",
            Self::ClockNow => "now",
            Self::ClockTimestamp => "timestamp",
            Self::ClockSince => "since",
            Self::ClockSleepUntil => "sleep_until",
            Self::ClockAdd => "add",
            Self::ClockBetween => "between",
            Self::ClockTimestampToText => "timestamp_to_text",
            Self::ClockUnixNanoseconds => "unix_nanoseconds",
            Self::ClockFromUnixNanoseconds => "from_unix_nanoseconds",
            Self::ClockTimestampBetween => "timestamp_between",
            Self::ClockStartingAt => "starting_at",
            Self::TaskTimeout => "timeout",
            Self::TaskDeadline => "deadline",
            Self::TaskAll => "all",
            Self::TaskResults => "results",
            Self::TaskRace => "race",
            Self::SelectReceive => "receive",
            Self::SelectInbox => "inbox",
            Self::SelectTask => "task",
            Self::SelectAfter => "after",
            Self::SelectAt => "at",
            Self::SelectFirst => "first",
            Self::ChannelAddReceiver => "add_receiver",
            Self::BroadcastBounded => "bounded",
            Self::BroadcastPublish | Self::WatchPublish => "publish",
            Self::BroadcastSubscribe | Self::WatchSubscribe => "subscribe",
            Self::WatchStartingWith => "starting_with",
            Self::WatchReceive => "receive",
            Self::SelectWatch => "watch",
            Self::DebugCrash => "crash",
            Self::OptionUnwrapOrFail => "unwrap_or_fail",
            Self::TextConcat => "concat",
            Self::TextByteLength => "byte_length",
            Self::TextByteSlice => "byte_slice",
            Self::TextContains => "contains",
            Self::TextStartsWith => "starts_with",
            Self::TextEndsWith => "ends_with",
            Self::TextSplit => "split",
            Self::TextReplace => "replace",
            Self::TextTrimStart => "trim_start",
            Self::TextTrimEnd => "trim_end",
            Self::TextLowercase => "lowercase",
            Self::TextUppercase => "uppercase",
            Self::TextGraphemes => "graphemes",
            Self::ListGet => "get",
            Self::ListPush => "push",
            Self::ListSlice => "slice",
            Self::ListSet => "set",
            Self::MapRemove | Self::SetRemove => "remove",
            Self::MapSize | Self::SetSize => "size",
            Self::MapReduceEntries => "reduce_entries",
            Self::SetEmpty => "empty",
            Self::SetContains => "contains",
            Self::SetAdd => "add",
            Self::SetReduceItems => "reduce_items",
            Self::JsonNumberToDecimal => "number_to_decimal",
            Self::DecimalScaled => "scaled",
            Self::DecimalRead => "read",
            Self::DecimalToText => "to_text",
            Self::DecimalAddChecked => "add_checked",
            Self::DecimalSubtractChecked => "subtract_checked",
            Self::DecimalMultiplyChecked => "multiply_checked",
            Self::DecimalDivideRounded => "divide_rounded",
            Self::DecimalToFloat => "to_float",
            Self::BytesFrom => "from",
            Self::BytesLength => "length",
            Self::BytesGet => "get",
            Self::BytesIndexOf => "index_of",
            Self::BytesConcat => "concat",
            Self::BytesSlice => "slice",
            Self::BytesToList => "to_list",
            Self::BytesFromText => "from_text",
            Self::BytesToTextLossy => "to_text_lossy",
            Self::BytesValidUtf8Length => "valid_utf8_length",
            Self::BytesReadIntCode => "read_int_code",
            Self::BytesIntBytes => "int_bytes",
            Self::EmbedText => "text",
            Self::EmbedBytes => "bytes",
            Self::CauseContext => "context",
            Self::CauseOf => "of",
            Self::CauseKindCode => "kind_code",
            Self::CauseSummary => "summary",
            Self::CauseFrames => "frames",
            Self::CauseCausedBy => "caused_by",
            Self::CauseSuppressed => "suppressed",
            Self::CauseReasonCode => "reason_code",
            Self::CauseToText => "to_text",
            Self::TestAssertEqual => "assert_equal",
            Self::TestAssertNotEqual => "assert_not_equal",
            Self::TestAssert => "assert",
            Self::TestFail => "fail",
            Self::ResourceScoped => "scoped",
            Self::TestCaptureDefect => "capture_defect",
            Self::TestRecorder => "recorder",
            Self::TestRecord => "record",
            Self::TestRecorded => "recorded",
            Self::TestScript => "script",
            Self::TestNext => "next",
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
                | Self::ProcessOwnAddress
                | Self::ProcessQueued
                | Self::ProcessCapacity
                | Self::ProcessMonitor
                | Self::ProcessUnmonitor
                | Self::SupervisorStart
                | Self::SupervisorStartProcess
                | Self::SupervisorStartSupervisor
                | Self::SupervisorStartHandoff
                | Self::SupervisorAddress
                | Self::SupervisorRemoveProcess
                | Self::SupervisorRemoveSupervisor
                | Self::SupervisorStop
                | Self::SupervisorWait
                | Self::ClockSleep
        ) || self.is_native_clock_operation()
    }

    /// The `std/clock` and timeout operations beyond `Clock.sleep`. They run only
    /// on the native backend, on the scheduler's clock, like `Clock.sleep`; the
    /// interpreter defects at each of them. The pure ones are among them because
    /// the interpreter has no `Instant` or `Timestamp` value yet.
    pub(crate) const fn is_native_clock_operation(self) -> bool {
        matches!(
            self,
            Self::ClockNow
                | Self::ClockTimestamp
                | Self::ClockSince
                | Self::ClockSleepUntil
                | Self::ClockAdd
                | Self::ClockBetween
                | Self::ClockTimestampToText
                | Self::ClockUnixNanoseconds
                | Self::ClockFromUnixNanoseconds
                | Self::ClockTimestampBetween
                | Self::ClockStartingAt
                | Self::TaskTimeout
                | Self::TaskDeadline
        )
    }

    /// The `std/select` functions. They run only on the native backend, where
    /// the scheduler can wait on several sources; the interpreter has no task,
    /// channel, or timer runtime and defects at each of them.
    pub(crate) const fn is_select_operation(self) -> bool {
        matches!(
            self,
            Self::SelectReceive
                | Self::SelectInbox
                | Self::SelectTask
                | Self::SelectAfter
                | Self::SelectAt
                | Self::SelectFirst
                | Self::SelectWatch
        )
    }

    /// The channel variants (G29) outside `std/select`. Like the bounded
    /// channel, they run only on the native backend.
    pub(crate) const fn is_channel_variant(self) -> bool {
        matches!(
            self,
            Self::ChannelAddReceiver
                | Self::BroadcastBounded
                | Self::BroadcastPublish
                | Self::BroadcastSubscribe
                | Self::WatchStartingWith
                | Self::WatchPublish
                | Self::WatchSubscribe
                | Self::WatchReceive
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
                | Self::TaskAll
                | Self::TaskResults
                | Self::TaskRace
                | Self::ChannelSend
                | Self::ChannelReceive
                | Self::ChannelClose
                | Self::ChannelAbort
                | Self::ChannelBounded
        ) || self.requires_process_runtime()
            || self.is_select_operation()
            || self.is_channel_variant()
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
    JsonNotInteger,
    JsonOutOfRange,
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
    /// `Timeout.TimedOut(limit: Duration)`.
    TimeoutTimedOut,
    DownReturned,
    DownFailed,
    DownDefected,
    DownStopped,
    DownUnavailable,
    /// `std/result.Result.Ok(value: T)`.
    ResultOk,
    /// `std/result.Result.Err(error: E)`.
    ResultErr,
    /// `HandoffError.CapacityReached(argument: A)`.
    HandoffCapacityReached,
    /// `HandoffError.Stopped(argument: A)`.
    HandoffStopped,
}

impl StandardConstructor {
    pub(crate) const ALL: [Self; 53] = [
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
        Self::JsonNotInteger,
        Self::JsonOutOfRange,
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
        Self::TimeoutTimedOut,
        Self::DownReturned,
        Self::DownFailed,
        Self::DownDefected,
        Self::DownStopped,
        Self::DownUnavailable,
        Self::ResultOk,
        Self::ResultErr,
        Self::HandoffCapacityReached,
        Self::HandoffStopped,
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
            Self::JsonNotInteger | Self::JsonOutOfRange => StandardType::JsonConversionError,
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
            Self::TimeoutTimedOut => StandardType::Timeout,
            Self::ResultOk | Self::ResultErr => StandardType::Result,
            Self::DownReturned
            | Self::DownFailed
            | Self::DownDefected
            | Self::DownStopped
            | Self::DownUnavailable => StandardType::ProcessDown,
            Self::HandoffCapacityReached | Self::HandoffStopped => {
                StandardType::SupervisorHandoffError
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
            StandardType::Option => None,
            StandardType::ChannelSender
            | StandardType::ChannelReceiver
            | StandardType::ChannelCompletion
            | StandardType::ChannelReceive
            | StandardType::ChannelError => Some(StandardModule::Channel),
            StandardType::BroadcastPublisher => Some(StandardModule::Broadcast),
            StandardType::WatchPublisher | StandardType::WatchSubscription => {
                Some(StandardModule::Watch)
            }
            // SendError owns both Closed and Aborted, but only type-qualified
            // access is valid: Channel.Aborted names the receive failure.
            StandardType::ChannelSendError => None,
            StandardType::Task => None,
            // Std docs write these type-qualified (`SendError.Full`,
            // `Supervisor.Restart.Never`), and `std/channel` already owns a
            // `SendError`, so none is a module member.
            StandardType::ProcessSendError
            | StandardType::ProcessDown
            | StandardType::SupervisorRestart
            | StandardType::SupervisorStartError
            | StandardType::SupervisorHandoffError => None,
            StandardType::ProcessAddress
            | StandardType::ProcessInbox
            | StandardType::ProcessSpec
            | StandardType::SupervisorMember
            | StandardType::SupervisorPolicy
            | StandardType::SupervisorRestarts
            | StandardType::SupervisorRunning
            | StandardType::SupervisorManagedProcess
            | StandardType::SupervisorManagedSupervisor
            | StandardType::Clock
            | StandardType::Random
            | StandardType::Instant
            | StandardType::Timestamp
            | StandardType::SelectEvent
            | StandardType::ProcessMonitor
            | StandardType::ProcessHandoffSpec => None,
            // Written type-qualified, `Timeout.TimedOut`, as the spec does.
            StandardType::Timeout => None,
            StandardType::Result => None,
            StandardType::Console
            | StandardType::FileSystem
            | StandardType::Map
            | StandardType::Path
            | StandardType::Duration
            | StandardType::Set
            | StandardType::Decimal
            | StandardType::Bytes
            | StandardType::Cause
            | StandardType::TestRecorder
            | StandardType::TestScript
            | StandardType::JsonValue
            | StandardType::JsonNumber
            | StandardType::DecodeError
            | StandardType::JsonConversionError => None,
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
            Self::JsonNotInteger => "NotInteger",
            Self::JsonOutOfRange => "OutOfRange",
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
            Self::TimeoutTimedOut => "TimedOut",
            Self::ResultOk => "Ok",
            Self::ResultErr => "Err",
            Self::DownReturned => "Returned",
            Self::DownFailed => "Failed",
            Self::DownDefected => "Defected",
            Self::DownStopped => "Stopped",
            Self::DownUnavailable => "Unavailable",
            Self::HandoffCapacityReached => "CapacityReached",
            Self::HandoffStopped => "Stopped",
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
            Self::ChannelSendClosed | Self::ChannelSendAborted | Self::ChannelReceived => {
                &["value"]
            }
            Self::ChannelDrained | Self::ChannelAborted => &[],
            Self::JsonNotInteger
            | Self::JsonOutOfRange
            | Self::IntParseEmpty
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
            Self::HandoffCapacityReached | Self::HandoffStopped => &["argument"],
            Self::TimeoutTimedOut => &["limit"],
            Self::ResultOk => &["value"],
            Self::ResultErr => &["error"],
            Self::RestartNever
            | Self::RestartOnDefect
            | Self::StartCapacityReached
            | Self::StartStopped
            | Self::DownReturned
            | Self::DownFailed
            | Self::DownDefected
            | Self::DownStopped
            | Self::DownUnavailable => &[],
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
    TaskValue,
    TaskFailures,
    TaskNeeds,
    ChannelValue,
    OrderCompareValue,
    OrderMinValue,
    OrderMaxValue,
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
    /// `Task.timeout`, `Task.deadline`, `Task.all`, `Task.results`, and
    /// `Task.race` share these: each call instantiates them afresh.
    TimeoutValue,
    TimeoutFailures,
    TimeoutNeeds,
    /// `std/result.Result<T, E>`'s parameters, which `Ok` and `Err` take.
    ResultValue,
    ResultError,
    /// The `std/select` functions share these, as the timeouts do: `A`, the
    /// event's value; `F`, its mapping function's failures; `R`, that
    /// function's needs; `T`, what the source delivers; `E`, a task's failures.
    SelectValue,
    SelectFailures,
    SelectNeeds,
    SelectSource,
    SelectTaskFailures,
    ProcessOwnAddressMessage,
    ProcessQueuedMessage,
    ProcessCapacityMessage,
    ProcessMonitorMessage,
    ProcessMonitorTarget,
    ProcessUnmonitorMessage,
    ProcessHandoffSpecArgument,
    ProcessHandoffSpecMessage,
    ProcessHandoffSpecFailures,
    ProcessHandoffSpecNeeds,
    /// Also the parameter of the `HandoffError` constructors.
    SupervisorStartHandoffArgument,
    SupervisorStartHandoffMessage,
    SupervisorStartHandoffFailures,
}

impl StandardGeneric {
    pub(crate) const ALL: [Self; 49] = [
        Self::FileState,
        Self::FileFailures,
        Self::FileNeeds,
        Self::ListFirstValue,
        Self::ListLengthValue,
        Self::OptionValue,
        Self::TaskValue,
        Self::TaskFailures,
        Self::TaskNeeds,
        Self::ChannelValue,
        Self::OrderCompareValue,
        Self::OrderMinValue,
        Self::OrderMaxValue,
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
        Self::TimeoutValue,
        Self::TimeoutFailures,
        Self::TimeoutNeeds,
        Self::ResultValue,
        Self::ResultError,
        Self::SelectValue,
        Self::SelectFailures,
        Self::SelectNeeds,
        Self::SelectSource,
        Self::SelectTaskFailures,
        Self::ProcessOwnAddressMessage,
        Self::ProcessQueuedMessage,
        Self::ProcessCapacityMessage,
        Self::ProcessMonitorMessage,
        Self::ProcessMonitorTarget,
        Self::ProcessUnmonitorMessage,
        Self::ProcessHandoffSpecArgument,
        Self::ProcessHandoffSpecMessage,
        Self::ProcessHandoffSpecFailures,
        Self::ProcessHandoffSpecNeeds,
        Self::SupervisorStartHandoffArgument,
        Self::SupervisorStartHandoffMessage,
        Self::SupervisorStartHandoffFailures,
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
            Self::TaskValue => "T",
            Self::TaskFailures => "E",
            Self::TaskNeeds => "R",
            Self::ChannelValue => "T",
            Self::OrderCompareValue | Self::OrderMinValue | Self::OrderMaxValue => "A",
            Self::ProcessSpecMessage
            | Self::ProcessRegisterMessage
            | Self::ProcessReceiveMessage
            | Self::ProcessSendMessage
            | Self::SupervisorProcessMessage
            | Self::SupervisorStartProcessMessage
            | Self::SupervisorAddressMessage
            | Self::SupervisorRemoveMessage
            | Self::ProcessOwnAddressMessage
            | Self::ProcessQueuedMessage
            | Self::ProcessCapacityMessage
            | Self::ProcessMonitorMessage
            | Self::ProcessUnmonitorMessage
            | Self::ProcessHandoffSpecMessage
            | Self::SupervisorStartHandoffMessage => "M",
            Self::ProcessHandoffSpecArgument | Self::SupervisorStartHandoffArgument => "A",
            Self::ProcessMonitorTarget => "T",
            Self::ProcessSpecFailures
            | Self::ProcessRegisterFailures
            | Self::SupervisorProcessFailures
            | Self::SupervisorStartProcessFailures
            | Self::ProcessHandoffSpecFailures
            | Self::SupervisorStartHandoffFailures => "E",
            Self::ProcessSpecNeeds | Self::ProcessHandoffSpecNeeds => "R",
            Self::TimeoutValue => "T",
            Self::TimeoutFailures => "E",
            Self::TimeoutNeeds => "R",
            Self::ResultValue => "T",
            Self::ResultError => "E",
            Self::SelectValue => "A",
            Self::SelectFailures => "F",
            Self::SelectNeeds => "R",
            Self::SelectSource => "T",
            Self::SelectTaskFailures => "E",
        }
    }

    pub(crate) const fn kind(self) -> StandardGenericKind {
        match self {
            Self::FileFailures | Self::TimeoutFailures => StandardGenericKind::FailureRow,
            Self::TaskFailures
            | Self::ProcessSpecFailures
            | Self::ProcessRegisterFailures
            | Self::SupervisorProcessFailures
            | Self::SupervisorStartProcessFailures
            | Self::SelectFailures
            | Self::SelectTaskFailures
            | Self::ProcessHandoffSpecFailures
            | Self::SupervisorStartHandoffFailures => StandardGenericKind::FailureSet,
            Self::FileNeeds
            | Self::ProcessSpecNeeds
            | Self::ProcessHandoffSpecNeeds
            | Self::TimeoutNeeds
            | Self::SelectNeeds => StandardGenericKind::NeedsRow,
            _ => StandardGenericKind::Value,
        }
    }
}
