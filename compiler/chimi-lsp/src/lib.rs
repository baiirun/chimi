//! Editor support for Chimi.
//!
//! `workspace` owns editor state and analysis and knows nothing about the LSP
//! wire format. The transport converts URIs and protocol messages at the edge.

pub mod server;
mod uri;
pub mod workspace;

pub use workspace::{
    DocumentPath, DocumentVersion, EditorDiagnostic, FileDiagnostics, LineRange, ProjectConfig,
    TextReplacement, Workspace,
};
