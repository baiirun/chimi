#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum StandardModule {
    Io,
    Json,
    Map,
}

impl StandardModule {
    pub(crate) const ALL: [Self; 3] = [Self::Io, Self::Json, Self::Map];

    pub(crate) const fn path(self) -> &'static str {
        match self {
            Self::Io => "std/io",
            Self::Json => "std/json",
            Self::Map => "std/map",
        }
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
}

impl StandardType {
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
        }
    }

    pub(crate) const fn runtime_id(self) -> u32 {
        self as u32
    }
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
}

impl StandardFunction {
    pub(crate) const ALL: [Self; 14] = [
        Self::IoPath,
        Self::MapEmpty,
        Self::MapGet,
        Self::MapPut,
        Self::MapUpdate,
        Self::MapMapValues,
        Self::JsonDecode,
        Self::JsonEncode,
        Self::JsonBoolean,
        Self::JsonInt,
        Self::JsonText,
        Self::JsonArray,
        Self::JsonObject,
        Self::JsonNumberToInt,
    ];

    pub(crate) const fn module(self) -> Option<StandardModule> {
        match self {
            Self::IoPath => Some(StandardModule::Io),
            Self::MapEmpty | Self::MapGet | Self::MapPut | Self::MapUpdate | Self::MapMapValues => {
                Some(StandardModule::Map)
            }
            Self::JsonDecode
            | Self::JsonEncode
            | Self::JsonBoolean
            | Self::JsonInt
            | Self::JsonText
            | Self::JsonArray
            | Self::JsonObject
            | Self::JsonNumberToInt => Some(StandardModule::Json),
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
        }
    }

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
}

impl StandardConstructor {
    pub(crate) const ALL: [Self; 17] = [
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
        }
    }

    pub(crate) const fn module(self) -> Option<StandardModule> {
        match self.type_kind() {
            StandardType::ConsoleError | StandardType::FileError | StandardType::Utf8Error => {
                Some(StandardModule::Io)
            }
            StandardType::JsonValue
            | StandardType::JsonNumber
            | StandardType::DecodeError
            | StandardType::NumberError => Some(StandardModule::Json),
            StandardType::Option => None,
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
        }
    }

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
    NeedsRow,
    EffectRow,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum StandardGeneric {
    FileState,
    FileFailures,
    FileNeeds,
    FileEffects,
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
    MapUpdateEffects,
    MapValuesKey,
    MapValuesInput,
    MapValuesOutput,
    MapValuesFailures,
    MapValuesNeeds,
    MapValuesEffects,
}

impl StandardGeneric {
    pub(crate) const ALL: [Self; 24] = [
        Self::FileState,
        Self::FileFailures,
        Self::FileNeeds,
        Self::FileEffects,
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
        Self::MapUpdateEffects,
        Self::MapValuesKey,
        Self::MapValuesInput,
        Self::MapValuesOutput,
        Self::MapValuesFailures,
        Self::MapValuesNeeds,
        Self::MapValuesEffects,
    ];

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::FileState => "$file_state",
            Self::FileFailures => "$file_failures",
            Self::FileNeeds => "$file_needs",
            Self::FileEffects => "$file_effects",
            Self::ListFirstValue => "$list_first_value",
            Self::ListLengthValue => "$list_length_value",
            Self::OptionValue => "$option_value",
            Self::MapEmptyKey => "$map_empty_key",
            Self::MapEmptyValue => "$map_empty_value",
            Self::MapGetKey => "$map_get_key",
            Self::MapGetValue => "$map_get_value",
            Self::MapPutKey => "$map_put_key",
            Self::MapPutValue => "$map_put_value",
            Self::MapUpdateKey => "$map_update_key",
            Self::MapUpdateValue => "$map_update_value",
            Self::MapUpdateFailures => "$map_update_failures",
            Self::MapUpdateNeeds => "$map_update_needs",
            Self::MapUpdateEffects => "$map_update_effects",
            Self::MapValuesKey => "$map_values_key",
            Self::MapValuesInput => "$map_values_input",
            Self::MapValuesOutput => "$map_values_output",
            Self::MapValuesFailures => "$map_values_failures",
            Self::MapValuesNeeds => "$map_values_needs",
            Self::MapValuesEffects => "$map_values_effects",
        }
    }

    pub(crate) const fn kind(self) -> StandardGenericKind {
        match self {
            Self::FileFailures | Self::MapUpdateFailures | Self::MapValuesFailures => {
                StandardGenericKind::FailureRow
            }
            Self::FileNeeds | Self::MapUpdateNeeds | Self::MapValuesNeeds => {
                StandardGenericKind::NeedsRow
            }
            Self::FileEffects | Self::MapUpdateEffects | Self::MapValuesEffects => {
                StandardGenericKind::EffectRow
            }
            _ => StandardGenericKind::Value,
        }
    }
}
