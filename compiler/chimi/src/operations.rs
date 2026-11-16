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
    #[allow(dead_code)]
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
}

const fn pure(id: &'static str, arity: usize, implementation: StandardFunction) -> Operation {
    Operation {
        id,
        arity,
        class: OperationClass::Pure,
        implementation,
    }
}

const fn runtime(id: &'static str, arity: usize, implementation: StandardFunction) -> Operation {
    Operation {
        id,
        arity,
        class: OperationClass::Runtime,
        implementation,
    }
}

pub(crate) const OPERATIONS: &[Operation] = &[
    pure("std/option.unwrap_or_fail", 2, StandardFunction::OptionUnwrapOrFail),
    pure("std/int.parse", 1, StandardFunction::IntParse),
    pure("std/int.to_text", 1, StandardFunction::IntToText),
    pure("std/int.add_checked", 2, StandardFunction::IntAddChecked),
    pure("std/int.subtract_checked", 2, StandardFunction::IntSubtractChecked),
    pure("std/int.multiply_checked", 2, StandardFunction::IntMultiplyChecked),
    pure("std/int.divide_checked", 2, StandardFunction::IntDivideChecked),
    pure("std/int.remainder_checked", 2, StandardFunction::IntRemainderChecked),
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
    pure("std/map.reduce_entries", 3, StandardFunction::MapReduceEntries),
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
    pure("std/json.number_to_int", 1, StandardFunction::JsonNumberToInt),
    pure("std/json.number_to_decimal", 1, StandardFunction::JsonNumberToDecimal),

    // std/debug
    pure("std/debug.render", 1, StandardFunction::DebugRender),
    pure("std/debug.render_indented", 1, StandardFunction::DebugRenderIndented),
    pure("std/debug.crash", 1, StandardFunction::DebugCrash),

    // std/decimal: `+`, `-`, `*`, and comparisons are operators, so only the
    // checked forms and the constructors that validate in Chimi are native.
    pure("std/decimal.scaled", 2, StandardFunction::DecimalScaled),
    pure("std/decimal.read", 1, StandardFunction::DecimalRead),
    pure("std/decimal.to_text", 1, StandardFunction::DecimalToText),
    pure("std/decimal.add_checked", 2, StandardFunction::DecimalAddChecked),
    pure("std/decimal.subtract_checked", 2, StandardFunction::DecimalSubtractChecked),
    pure("std/decimal.multiply_checked", 2, StandardFunction::DecimalMultiplyChecked),
    pure("std/decimal.divide_rounded", 4, StandardFunction::DecimalDivideRounded),
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
    pure("std/bytes.to_text_lossy", 1, StandardFunction::BytesToTextLossy),
    pure("std/bytes.valid_utf8_length", 1, StandardFunction::BytesValidUtf8Length),
    pure("std/bytes.read_int_code", 3, StandardFunction::BytesReadIntCode),
    pure("std/bytes.int_bytes", 2, StandardFunction::BytesIntBytes),

    // std/embed: build-time only; see `CoreLowerer::embed`.
    pure("std/embed.text", 1, StandardFunction::EmbedText),
    pure("std/embed.bytes", 1, StandardFunction::EmbedBytes),

    // std/resource
    runtime("std/resource.scoped", 3, StandardFunction::ResourceScoped),
];

pub(crate) const RUNTIME_KNOWN: &[RuntimeKnownDeclaration] = &[
    known_type("std/option.Option", StandardType::Option),
    known_constructor("std/option.Option.None", StandardConstructor::OptionNone),
    known_constructor("std/option.Option.Some", StandardConstructor::OptionSome),
    known_type("std/order.Ordering", StandardType::Ordering),
    known_constructor("std/order.Ordering.Less", StandardConstructor::OrderingLess),
    known_constructor("std/order.Ordering.Equal", StandardConstructor::OrderingEqual),
    known_constructor("std/order.Ordering.Greater", StandardConstructor::OrderingGreater),
    known_type("std/int.ParseError", StandardType::IntParseError),
    known_constructor("std/int.ParseError.Empty", StandardConstructor::IntParseEmpty),
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
    native_type("std/json.Number", StandardType::JsonNumber),
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
    known_type("std/json.ConversionError", StandardType::JsonConversionError),
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
        let module = stated.function.split_once('.').map_or(stated.function, |(module, _)| module);
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
            mismatches.push(format!("stated requirement names {}, which is not declared", stated.function));
        }
    }
    for entry in RUNTIME_KNOWN {
        let module = entry.id.split_once('.').map_or(entry.id, |(module, _)| module);
        if !modules.contains(module) {
            continue;
        }
        let identity = match entry.identity {
            RuntimeIdentity::Type(kind) | RuntimeIdentity::NativeType(kind) => {
                StandardSymbol::Type(kind)
            }
            RuntimeIdentity::Constructor(constructor) => StandardSymbol::Constructor(constructor),
        };
        let declared = resolved.symbols.iter().any(|symbol| {
            symbol.module_id.is_some() && resolved.standard_symbol(symbol.id) == Some(identity)
        });
        if !declared {
            mismatches.push(format!("runtime-known {} is not declared in source", entry.id));
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
