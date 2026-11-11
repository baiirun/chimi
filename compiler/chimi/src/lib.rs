pub mod backend;
mod capability;
mod compiler;
#[cfg(feature = "native-backend")]
#[doc(hidden)]
pub mod concurrency_lab;
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
pub mod line_index;
mod numeric;
pub mod project;
mod resolution;
mod semantic;
pub mod source;
mod standard;
mod syntax;
mod typing;

pub use backend::{BackendOperationProfile, BackendProfile, BackendProgram, BackendValueProfile};
pub use compiler::{
    CheckReport, Compiler, CompilerSession, GenericParameterKind, IncrementalStatistics,
    IncrementalTypeReport, ModuleDiagnostics, NameReport, ProjectPathError, ProjectSource,
    ReferenceInfo, SymbolHandle, SymbolInfo, TypeArenaStats, TypeReport, ValuePropertyStatus,
};
pub use formatter::FormatReport;
pub use inspection::{
    CallableInspection, ClausePosition, InspectedToken, InspectionError, InspectionReport,
    NeedInspection, ReferenceInspection, SymbolInspection, SyntaxFrame, ValuePropertyInspection,
};
