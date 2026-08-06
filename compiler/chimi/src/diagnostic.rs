use crate::ids::{FileId, TextRange};
use crate::source::SourceSnapshot;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticStage {
    Syntax,
    Lowering,
    Name,
    Type,
    ExecutionSetup,
}

impl fmt::Display for DiagnosticStage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Syntax => "syntax",
            Self::Lowering => "lowering",
            Self::Name => "name",
            Self::Type => "type",
            Self::ExecutionSetup => "execution-setup",
        };
        formatter.write_str(name)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Severity {
    Error,
}

impl fmt::Display for Severity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("error")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticLabel {
    pub(crate) file_id: FileId,
    pub(crate) range: TextRange,
    pub(crate) message: String,
}

impl DiagnosticLabel {
    pub const fn range(&self) -> TextRange {
        self.range
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub(crate) const fn file_id(&self) -> FileId {
        self.file_id
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticEdit {
    pub(crate) file_id: FileId,
    pub(crate) range: TextRange,
    pub(crate) replacement: String,
}

impl DiagnosticEdit {
    pub const fn range(&self) -> TextRange {
        self.range
    }

    pub fn replacement(&self) -> &str {
        &self.replacement
    }

    pub(crate) const fn file_id(&self) -> FileId {
        self.file_id
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    stage: DiagnosticStage,
    code: &'static str,
    severity: Severity,
    file_id: FileId,
    range: TextRange,
    message: String,
    pub(crate) labels: Vec<DiagnosticLabel>,
    pub(crate) fixes: Vec<DiagnosticEdit>,
}

impl Diagnostic {
    pub(crate) fn error(
        stage: DiagnosticStage,
        code: &'static str,
        file_id: FileId,
        range: TextRange,
        message: impl Into<String>,
    ) -> Self {
        Self {
            stage,
            code,
            severity: Severity::Error,
            file_id,
            range,
            message: message.into(),
            labels: Vec::new(),
            fixes: Vec::new(),
        }
    }

    pub const fn stage(&self) -> DiagnosticStage {
        self.stage
    }

    pub const fn code(&self) -> &'static str {
        self.code
    }

    pub const fn range(&self) -> TextRange {
        self.range
    }

    pub const fn severity(&self) -> Severity {
        self.severity
    }

    pub(crate) const fn file_id(&self) -> FileId {
        self.file_id
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn labels(&self) -> &[DiagnosticLabel] {
        &self.labels
    }

    pub fn fixes(&self) -> &[DiagnosticEdit] {
        &self.fixes
    }

    pub fn render(&self, source: &SourceSnapshot) -> String {
        debug_assert_eq!(self.file_id, source.file_id());
        format!(
            "{}:{}..{}: {}[{}] {}: {}",
            source.path().display(),
            self.range.start(),
            self.range.end(),
            self.severity,
            self.code,
            self.stage,
            self.message,
        )
    }
}
