mod compiler;
pub mod diagnostic;
mod executable;
pub mod execution;
mod formatter;
#[cfg(feature = "frontend-benchmarks")]
#[doc(hidden)]
pub mod frontend_benchmark;
mod ids;
mod inspection;
mod json;
pub mod project;
mod resolution;
mod semantic;
pub mod source;
mod syntax;
mod typing;

pub use compiler::{
    CheckReport, Compiler, GenericParameterKind, NameReport, ProjectPathError, ProjectSource,
    ReferenceInfo, SymbolHandle, SymbolInfo, TypeArenaStats, TypeReport,
};
pub use formatter::FormatReport;
pub use inspection::{
    CallableInspection, InspectedToken, InspectionError, InspectionReport, NeedInspection,
    ReferenceInspection, SymbolInspection, SyntaxFrame,
};
