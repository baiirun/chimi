mod compiler;
pub mod diagnostic;
mod executable;
pub mod execution;
#[cfg(feature = "frontend-benchmarks")]
#[doc(hidden)]
pub mod frontend_benchmark;
mod ids;
mod resolution;
mod semantic;
pub mod source;
mod syntax;
mod typing;

pub use compiler::{
    CheckReport, Compiler, GenericParameterKind, NameReport, ProjectPathError, ProjectSource,
    ReferenceInfo, SymbolHandle, SymbolInfo, TypeArenaStats, TypeReport,
};
