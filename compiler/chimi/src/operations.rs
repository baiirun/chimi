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

use crate::standard::{StandardConstructor, StandardFunction, StandardType};

/// What an operation may touch, which decides where it may run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OperationClass {
    /// Deterministic and creates no runtime resource; constants may call it.
    Pure,
    /// Needs the scheduler or creates runtime resources.
    #[allow(dead_code)]
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
    /// Registered ahead of its module's port: the module is not importable yet
    /// (`std/map`, `std/json`) or the operation defects with
    /// "<id> is not implemented yet". The std-tests coverage check skips it; a
    /// port flips it with [`pure`] once a program reaches the operation.
    pub(crate) porting: bool,
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
        porting: false,
    }
}

/// A pure operation registered ahead of its module's port (see
/// [`Operation::porting`]).
const fn porting(id: &'static str, arity: usize, implementation: StandardFunction) -> Operation {
    Operation {
        porting: true,
        ..pure(id, arity, implementation)
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
    pure("std/list.length", 1, StandardFunction::ListLength),
    pure("std/list.get", 2, StandardFunction::ListGet),
    pure("std/list.push", 2, StandardFunction::ListPush),
    pure("std/list.slice", 2, StandardFunction::ListSlice),
    pure("std/list.set", 3, StandardFunction::ListSet),
    // std/map: the first three are the built-in intrinsics the old path runs.
    porting("std/map.empty", 0, StandardFunction::MapEmpty),
    porting("std/map.get", 2, StandardFunction::MapGet),
    porting("std/map.put", 3, StandardFunction::MapPut),
    porting("std/map.remove", 2, StandardFunction::MapRemove),
    porting("std/map.size", 1, StandardFunction::MapSize),
    porting("std/map.reduce_entries", 3, StandardFunction::MapReduceEntries),

    // std/set
    porting("std/set.empty", 0, StandardFunction::SetEmpty),
    porting("std/set.contains", 2, StandardFunction::SetContains),
    porting("std/set.add", 2, StandardFunction::SetAdd),
    porting("std/set.remove", 2, StandardFunction::SetRemove),
    porting("std/set.size", 1, StandardFunction::SetSize),
    porting("std/set.reduce_items", 3, StandardFunction::SetReduceItems),

    // std/json: all but number_to_decimal are the built-in intrinsics.
    porting("std/json.decode", 1, StandardFunction::JsonDecode),
    porting("std/json.encode", 1, StandardFunction::JsonEncode),
    porting("std/json.int", 1, StandardFunction::JsonInt),
    porting("std/json.number_to_int", 1, StandardFunction::JsonNumberToInt),
    porting("std/json.number_to_decimal", 1, StandardFunction::JsonNumberToDecimal),

    // std/debug
    pure("std/debug.render", 1, StandardFunction::DebugRender),
    pure("std/debug.render_indented", 1, StandardFunction::DebugRenderIndented),
    pure("std/debug.crash", 1, StandardFunction::DebugCrash),

    // std/decimal
    porting("std/decimal.of", 2, StandardFunction::DecimalOf),
    porting("std/decimal.parse_checked", 1, StandardFunction::DecimalParseChecked),
    porting("std/decimal.to_text", 1, StandardFunction::DecimalToText),
    porting("std/decimal.add", 2, StandardFunction::DecimalAdd),
    porting("std/decimal.subtract", 2, StandardFunction::DecimalSubtract),
    porting("std/decimal.multiply", 2, StandardFunction::DecimalMultiply),
    porting("std/decimal.negate", 1, StandardFunction::DecimalNegate),
    porting("std/decimal.divide_rounded", 4, StandardFunction::DecimalDivideRounded),
    porting("std/decimal.compare", 2, StandardFunction::DecimalCompare),
    porting("std/decimal.to_float", 1, StandardFunction::DecimalToFloat),
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
    known_type("std/json.NumberError", StandardType::NumberError),
    known_constructor(
        "std/json.NumberError.NotInteger",
        StandardConstructor::NumberNotInteger,
    ),
    known_constructor(
        "std/json.NumberError.OutOfRange",
        StandardConstructor::NumberOutOfRange,
    ),
];

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

/// The id of the operation that runs `function`, for its defect messages.
pub(crate) fn operation_id(function: StandardFunction) -> Option<&'static str> {
    OPERATIONS
        .iter()
        .find(|operation| operation.implementation == function)
        .map(|operation| operation.id)
}

/// The runtime identity of the source declaration with this qualified name.
pub(crate) fn runtime_known(id: &str) -> Option<RuntimeIdentity> {
    RUNTIME_KNOWN
        .iter()
        .find(|declaration| declaration.id == id)
        .map(|declaration| declaration.identity)
}

#[cfg(test)]
mod tests {
    use super::{OPERATIONS, RUNTIME_KNOWN, RuntimeIdentity};
    use crate::compiler::resolve_project_sources;
    use crate::standard::StandardSymbol;
    use crate::standard_tree::{embedded_module_paths, standard_source};
    use crate::typing::check_project;

    /// The whole-tree check: the embedded tree type-checks on its own, binds
    /// every registered operation to exactly one `native fn` with no operation
    /// left over, and declares every runtime-known item in source.
    #[test]
    fn the_embedded_tree_matches_the_registry() {
        let sources = embedded_module_paths()
            .map(|path| standard_source(path).expect("listed module is embedded"))
            .collect();
        let typed = check_project(resolve_project_sources(sources));
        assert!(
            typed.diagnostics.is_empty(),
            "{:?}",
            typed.diagnostics
        );
        let resolved = &typed.resolved;
        let mut bound = resolved
            .symbols
            .iter()
            .filter(|symbol| resolved.native_operation(symbol.id).is_some())
            .map(|symbol| {
                let module = &resolved.semantic.modules
                    [symbol.module_id.expect("native fn is a declaration").raw() as usize];
                format!("{}.{}", module.path, symbol.name)
            })
            .collect::<Vec<_>>();
        bound.sort();
        // An operation registered ahead of its module's port has no declaration yet.
        let mut registered = OPERATIONS
            .iter()
            .filter(|operation| !operation.porting)
            .map(|operation| operation.id)
            .collect::<Vec<_>>();
        registered.sort_unstable();
        assert_eq!(bound, registered);
        // A runtime-known declaration of a module not yet in the tree keeps its
        // synthesized built-in until that module's port.
        let ported = |id: &str| {
            let module = id.split_once('.').map_or(id, |(module, _)| module);
            crate::standard_tree::embedded_module(module).is_some()
        };
        for entry in RUNTIME_KNOWN.iter().filter(|entry| ported(entry.id)) {
            let identity = match entry.identity {
                RuntimeIdentity::Type(kind) | RuntimeIdentity::NativeType(kind) => {
                    StandardSymbol::Type(kind)
                }
                RuntimeIdentity::Constructor(constructor) => StandardSymbol::Constructor(constructor),
            };
            let declared = resolved.symbols.iter().any(|symbol| {
                symbol.module_id.is_some() && resolved.standard_symbol(symbol.id) == Some(identity)
            });
            assert!(declared, "{} is not declared in source", entry.id);
        }
    }
}
