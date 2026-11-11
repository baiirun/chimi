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
}

impl StandardModule {
    pub(crate) const ALL: [Self; 9] = [
        Self::Channel,
        Self::Io,
        Self::Json,
        Self::Map,
        Self::Task,
        Self::Int,
        Self::Float,
        Self::Debug,
        Self::Order,
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
            Self::IntParseError | Self::FloatParseError => "ParseError",
            Self::FloatConversionError => "ConversionError",
            Self::Ordering => "Ordering",
        }
    }

    /// The equatable and ordered properties this module publishes for the type.
    /// Its private representation grants neither silently.
    pub(crate) const fn properties(self) -> StandardProperties {
        match self {
            Self::Console | Self::FileSystem => StandardProperties::Opaque("a provider"),
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
            | Self::Ordering => StandardProperties::Structural,
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
}

impl StandardFunction {
    pub(crate) const ALL: [Self; 44] = [
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
    ];

    pub(crate) const fn module(self) -> Option<StandardModule> {
        match self {
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
            Self::IntParse | Self::FloatParse => "parse",
            Self::IntToText | Self::FloatToText => "to_text",
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
}

impl StandardConstructor {
    pub(crate) const ALL: [Self; 33] = [
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
            StandardType::Console
            | StandardType::FileSystem
            | StandardType::Map
            | StandardType::Path => None,
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
            Self::IntParseEmpty | Self::FloatParseEmpty => "Empty",
            Self::IntParseInvalidDigit => "InvalidDigit",
            Self::FloatParseInvalid => "Invalid",
            Self::IntParseOutOfRange | Self::FloatParseOutOfRange | Self::FloatOutOfRange => {
                "OutOfRange"
            }
            Self::FloatNotInteger => "NotInteger",
            Self::OrderingLess => "Less",
            Self::OrderingEqual => "Equal",
            Self::OrderingGreater => "Greater",
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
            | Self::OrderingGreater => &[],
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
}

impl StandardGeneric {
    pub(crate) const ALL: [Self; 32] = [
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
        }
    }

    pub(crate) const fn kind(self) -> StandardGenericKind {
        match self {
            Self::FileFailures | Self::MapUpdateFailures | Self::MapValuesFailures => {
                StandardGenericKind::FailureRow
            }
            Self::TaskFailures => StandardGenericKind::FailureSet,
            Self::FileNeeds | Self::MapUpdateNeeds | Self::MapValuesNeeds => {
                StandardGenericKind::NeedsRow
            }
            _ => StandardGenericKind::Value,
        }
    }
}
