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
}

const fn pure(id: &'static str, arity: usize, implementation: StandardFunction) -> Operation {
    Operation {
        id,
        arity,
        class: OperationClass::Pure,
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
    pure("std/list.length", 1, StandardFunction::ListLength),
    pure("std/list.get", 2, StandardFunction::ListGet),
    pure("std/list.push", 2, StandardFunction::ListPush),
    pure("std/list.slice", 2, StandardFunction::ListSlice),
    pure("std/list.set", 3, StandardFunction::ListSet),
];

pub(crate) const RUNTIME_KNOWN: &[RuntimeKnownDeclaration] = &[
    known_type("std/option.Option", StandardType::Option),
    known_constructor("std/option.Option.None", StandardConstructor::OptionNone),
    known_constructor("std/option.Option.Some", StandardConstructor::OptionSome),
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
];

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
