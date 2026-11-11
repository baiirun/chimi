//! LSP transport: protocol messages in, `Workspace` calls, protocol messages out.
//!
//! Initialization options name the projects, because Chimi has no manifest:
//! `{"projects": [{"root": "<dir>", "entry": "<module>"}]}`. A relative root
//! resolves against the first workspace folder.

use crate::uri::{file_path, file_uri};
use crate::workspace::{
    DiagnosticOrigin, DocumentPath, DocumentVersion, EditorDiagnostic, FileDiagnostics, LineRange,
    ProjectConfig, Workspace,
};
use chimi::line_index::{LinePosition, PositionEncoding};
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument,
    Notification as _, PublishDiagnostics,
};
use lsp_types::request::{Formatting, Request as _};
use lsp_types::{
    Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, DocumentFormattingParams,
    InitializeParams, NumberOrString, Position, PositionEncodingKind, PublishDiagnosticsParams,
    Range, SaveOptions, ServerCapabilities, ServerInfo, TextDocumentSyncCapability,
    TextDocumentSyncKind, TextDocumentSyncOptions, TextDocumentSyncSaveOptions, TextEdit, Uri,
};
use std::error::Error;
use std::path::{Path, PathBuf};

type ServerResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

pub fn run(connection: &Connection) -> ServerResult<()> {
    let (id, params) = connection.initialize_start()?;
    let params: InitializeParams = serde_json::from_value(params)?;
    let encoding = negotiate_encoding(&params);
    let (projects, problems) = configured_projects(&params);
    let capabilities = ServerCapabilities {
        position_encoding: Some(match encoding {
            PositionEncoding::Utf8 => PositionEncodingKind::UTF8,
            PositionEncoding::Utf16 => PositionEncodingKind::UTF16,
        }),
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::FULL),
                save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                    include_text: Some(false),
                })),
                ..TextDocumentSyncOptions::default()
            },
        )),
        document_formatting_provider: Some(lsp_types::OneOf::Left(true)),
        ..ServerCapabilities::default()
    };
    connection.initialize_finish(
        id,
        serde_json::json!({
            "capabilities": capabilities,
            "serverInfo": ServerInfo {
                name: "chimi-lsp".to_owned(),
                version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            },
        }),
    )?;
    let mut server = Server {
        connection,
        workspace: Workspace::new(projects, encoding),
    };
    for problem in problems {
        server.show_warning(problem)?;
    }
    server.main_loop()
}

struct Server<'connection> {
    connection: &'connection Connection,
    workspace: Workspace,
}

impl Server<'_> {
    fn main_loop(&mut self) -> ServerResult<()> {
        for message in &self.connection.receiver {
            match message {
                Message::Request(request) => {
                    if self.connection.handle_shutdown(&request)? {
                        return Ok(());
                    }
                    self.handle_request(request)?;
                }
                Message::Notification(notification) => self.handle_notification(notification)?,
                Message::Response(_) => {}
            }
        }
        Ok(())
    }

    fn handle_request(&mut self, request: Request) -> ServerResult<()> {
        let response = match request.method.as_str() {
            Formatting::METHOD => {
                let (id, params) =
                    request.extract::<DocumentFormattingParams>(Formatting::METHOD)?;
                let edits = document(&params.text_document.uri)
                    .and_then(|path| self.workspace.format(&path))
                    .map(|replacement| {
                        vec![TextEdit {
                            range: range(replacement.range),
                            new_text: replacement.text,
                        }]
                    });
                Response::new_ok(id, edits)
            }
            _ => Response::new_err(
                request.id,
                ErrorCode::MethodNotFound as i32,
                format!("unsupported request `{}`", request.method),
            ),
        };
        self.connection.sender.send(response.into())?;
        Ok(())
    }

    fn handle_notification(&mut self, notification: Notification) -> ServerResult<()> {
        match notification.method.as_str() {
            DidOpenTextDocument::METHOD => {
                let params: DidOpenTextDocumentParams =
                    notification.extract(DidOpenTextDocument::METHOD)?;
                let document_uri = params.text_document;
                if let Some(path) = document(&document_uri.uri) {
                    self.workspace.open(
                        path.clone(),
                        DocumentVersion(document_uri.version),
                        document_uri.text,
                    );
                    self.analyze(&path)?;
                }
            }
            DidChangeTextDocument::METHOD => {
                let params: DidChangeTextDocumentParams =
                    notification.extract(DidChangeTextDocument::METHOD)?;
                // Full sync: the last change carries the complete text.
                if let (Some(path), Some(change)) = (
                    document(&params.text_document.uri),
                    params.content_changes.into_iter().last(),
                ) {
                    self.workspace.change(
                        &path,
                        DocumentVersion(params.text_document.version),
                        change.text,
                    );
                    self.analyze(&path)?;
                }
            }
            DidSaveTextDocument::METHOD => {
                let params: DidSaveTextDocumentParams =
                    notification.extract(DidSaveTextDocument::METHOD)?;
                if let Some(path) = document(&params.text_document.uri) {
                    self.analyze(&path)?;
                }
            }
            DidCloseTextDocument::METHOD => {
                let params: DidCloseTextDocumentParams =
                    notification.extract(DidCloseTextDocument::METHOD)?;
                if let Some(path) = document(&params.text_document.uri) {
                    let cleared = self.workspace.close(&path);
                    self.publish(cleared)?;
                    self.analyze(&path)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn analyze(&mut self, path: &DocumentPath) -> ServerResult<()> {
        let sets = self.workspace.analyze(path);
        self.publish(sets)
    }

    fn publish(&self, sets: Vec<FileDiagnostics>) -> ServerResult<()> {
        for set in sets {
            let params = PublishDiagnosticsParams {
                uri: file_uri(set.path.as_path()),
                diagnostics: set.diagnostics.into_iter().map(diagnostic).collect(),
                version: set.version.map(|version| version.0),
            };
            self.connection
                .sender
                .send(Notification::new(PublishDiagnostics::METHOD.to_owned(), params).into())?;
        }
        Ok(())
    }

    fn show_warning(&self, message: String) -> ServerResult<()> {
        let params = lsp_types::ShowMessageParams {
            typ: lsp_types::MessageType::WARNING,
            message,
        };
        self.connection.sender.send(
            Notification::new(
                lsp_types::notification::ShowMessage::METHOD.to_owned(),
                params,
            )
            .into(),
        )?;
        Ok(())
    }
}

/// Local documents that exist on disk; everything else is ignored.
fn document(uri: &Uri) -> Option<DocumentPath> {
    DocumentPath::canonicalize(file_path(uri)?).ok()
}

/// Prefers UTF-8, which needs no conversion from compiler offsets.
fn negotiate_encoding(params: &InitializeParams) -> PositionEncoding {
    let supports_utf8 = params
        .capabilities
        .general
        .as_ref()
        .and_then(|general| general.position_encodings.as_ref())
        .is_some_and(|encodings| encodings.contains(&PositionEncodingKind::UTF8));
    if supports_utf8 {
        PositionEncoding::Utf8
    } else {
        PositionEncoding::Utf16
    }
}

/// Reads configured projects, returning a message for each invalid entry.
fn configured_projects(params: &InitializeParams) -> (Vec<ProjectConfig>, Vec<String>) {
    let base = params
        .workspace_folders
        .as_ref()
        .and_then(|folders| folders.first())
        .and_then(|folder| file_path(&folder.uri));
    let entries = params
        .initialization_options
        .as_ref()
        .and_then(|options| options.get("projects"))
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut projects = Vec::new();
    let mut problems = Vec::new();
    for entry in entries {
        let root = entry.get("root").and_then(serde_json::Value::as_str);
        let module = entry.get("entry").and_then(serde_json::Value::as_str);
        let (Some(root), Some(module)) = (root, module) else {
            problems.push(format!(
                "chimi: ignoring project {entry}: expected string `root` and `entry`"
            ));
            continue;
        };
        let root = resolve_root(base.as_deref(), root);
        match ProjectConfig::new(&root, module) {
            Ok(project) => projects.push(project),
            Err(error) => problems.push(format!(
                "chimi: ignoring project root {}: {error}",
                root.display()
            )),
        }
    }
    (projects, problems)
}

fn resolve_root(base: Option<&Path>, root: &str) -> PathBuf {
    match base {
        Some(base) if Path::new(root).is_relative() => base.join(root),
        _ => PathBuf::from(root),
    }
}

fn diagnostic(diagnostic: EditorDiagnostic) -> Diagnostic {
    let code = match diagnostic.origin {
        DiagnosticOrigin::Compiler { code, .. } => Some(NumberOrString::String(code.to_owned())),
        DiagnosticOrigin::ProjectLoad => None,
    };
    Diagnostic {
        range: range(diagnostic.range),
        severity: Some(DiagnosticSeverity::ERROR),
        code,
        source: Some("chimi".to_owned()),
        message: diagnostic.message,
        ..Diagnostic::default()
    }
}

fn range(range: LineRange) -> Range {
    Range {
        start: position(range.start),
        end: position(range.end),
    }
}

const fn position(position: LinePosition) -> Position {
    Position {
        line: position.line,
        character: position.character,
    }
}
