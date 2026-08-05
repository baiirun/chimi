use crate::diagnostic::Diagnostic;
use crate::executable::{lower, seal_entry};
use crate::execution::{ExecutionConfig, ExecutionReport, execute};
use crate::resolution::resolve;
use crate::semantic;
use crate::source::SourceSnapshot;
use crate::typing::check;
use std::sync::Arc;

#[derive(Debug)]
pub struct CheckReport {
    source: Arc<SourceSnapshot>,
    diagnostics: Vec<Diagnostic>,
}

impl CheckReport {
    fn success(source: Arc<SourceSnapshot>) -> Self {
        Self {
            source,
            diagnostics: Vec::new(),
        }
    }

    fn failure(source: Arc<SourceSnapshot>, diagnostics: Vec<Diagnostic>) -> Self {
        debug_assert!(!diagnostics.is_empty());
        Self {
            source,
            diagnostics,
        }
    }

    pub fn is_success(&self) -> bool {
        self.diagnostics.is_empty()
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn render_diagnostics(&self) -> String {
        let mut output = self
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.render(&self.source))
            .collect::<Vec<_>>()
            .join("\n");
        if !output.is_empty() {
            output.push('\n');
        }
        output
    }
}

pub struct Compiler;

impl Compiler {
    pub fn check(source: SourceSnapshot) -> CheckReport {
        let source = Arc::new(source);
        match seal(source.clone()) {
            Ok(_) => CheckReport::success(source),
            Err(diagnostics) => CheckReport::failure(source, diagnostics),
        }
    }

    pub fn run(
        source: SourceSnapshot,
        config: ExecutionConfig,
    ) -> Result<ExecutionReport, CheckReport> {
        let source = Arc::new(source);
        let verified = seal(source.clone())
            .map_err(|diagnostics| CheckReport::failure(source, diagnostics))?;
        let core = lower(verified);
        Ok(execute(core, config))
    }
}

fn seal(source: Arc<SourceSnapshot>) -> Result<crate::executable::VerifiedRoot, Vec<Diagnostic>> {
    let semantic = semantic::lower(source);
    let resolved = resolve(semantic);
    let typed = check(resolved);
    seal_entry(typed)
}

#[cfg(test)]
mod tests {
    use super::Compiler;
    use crate::diagnostic::DiagnosticStage;
    use crate::execution::{ExecutionConfig, ExecutionOutcome};
    use crate::source::SourceSnapshot;

    const VALID: &str = "struct Answer { value: Int } pub fn main() { Answer(value: 42) }";

    fn source(text: &str) -> SourceSnapshot {
        SourceSnapshot::from_text("test.chimi", text)
    }

    #[test]
    fn actual_source_executes_through_the_closed_root() {
        let report = Compiler::run(
            source(VALID),
            ExecutionConfig::new("test-execution").unwrap(),
        )
        .expect("valid source executes");

        assert_eq!(report.events.len(), 2);
        assert_eq!(report.events[0].sequence, 0);
        assert_eq!(report.events[1].sequence, 1);
        let expression_start = VALID.find("Answer(value: 42)").unwrap() as u32;
        assert_eq!(report.events[0].source_range.start(), expression_start);
        assert_eq!(
            report.events[0].source_range.end(),
            expression_start + "Answer(value: 42)".len() as u32
        );
        assert_eq!(report.events[0].origin_id, report.events[0].semantic_id);
        assert_eq!(report.events[0].file_id, 0);
        let ExecutionOutcome::Success(value) = report.outcome;
        assert_eq!(value.type_name, "Answer");
        assert_eq!(value.field_name, "value");
        assert_eq!(value.integer_value, 42);
    }

    #[test]
    fn source_ranges_survive_lossless_cst_lowering() {
        let text = concat!(
            "// retained trivia\r\n",
            "struct Answer {\n  value: Int\n}\n\n",
            "pub fn main() {\n  Answer( value: 42 ) // retained too\n}\n",
        );
        let report = Compiler::run(
            source(text),
            ExecutionConfig::new("source-range-test").unwrap(),
        )
        .expect("source with trivia executes");

        let expression_start = text.find("Answer( value: 42 )").unwrap() as u32;
        assert_eq!(report.events[0].source_range.start(), expression_start);
        assert_eq!(
            report.events[0].source_range.end(),
            expression_start + "Answer( value: 42 )".len() as u32
        );
    }

    #[test]
    fn missing_or_renamed_main_cannot_seal() {
        for text in [
            "struct Answer { value: Int }",
            "struct Answer { value: Int } pub fn start() { Answer(value: 42) }",
        ] {
            let report = Compiler::check(source(text));
            assert_eq!(report.diagnostics().len(), 1);
            assert_eq!(
                report.diagnostics()[0].stage(),
                DiagnosticStage::ExecutionSetup
            );
            assert_eq!(report.diagnostics()[0].code(), "E5001");
        }
    }

    #[test]
    fn unresolved_constructor_stops_at_name_resolution() {
        let text = "struct Answer { value: Int } pub fn main() { Missing(value: 42) }";
        let report = Compiler::check(source(text));

        assert_eq!(report.diagnostics().len(), 1);
        assert_eq!(report.diagnostics()[0].stage(), DiagnosticStage::Name);
        assert_eq!(report.diagnostics()[0].code(), "N3001");
        assert_eq!(
            report.diagnostics()[0].range().start() as usize,
            text.find("Missing").unwrap()
        );
    }

    #[test]
    fn invalid_field_value_stops_at_type_checking() {
        let text = "struct Answer { value: Int } pub fn main() { Answer(value: \"forty-two\") }";
        let report = Compiler::check(source(text));

        assert_eq!(report.diagnostics().len(), 1);
        assert_eq!(report.diagnostics()[0].stage(), DiagnosticStage::Type);
        assert_eq!(report.diagnostics()[0].code(), "T4001");
        assert_eq!(
            report.diagnostics()[0].range().start() as usize,
            text.find("\"forty-two\"").unwrap()
        );
    }

    #[test]
    fn malformed_source_is_diagnosed_without_panicking() {
        for text in [
            "struct Answer { value: Int } pub fn main() { Answer(value: 999999999999999999999999999999999999) }",
            "struct Answer { value: Int } pub fn main() { Answer(value: λ) }",
        ] {
            let report = Compiler::check(source(text));
            assert!(!report.is_success());
            assert_eq!(report.diagnostics()[0].stage(), DiagnosticStage::Syntax);
        }
    }

    #[test]
    fn syntax_beyond_the_executable_subset_cannot_silently_seal() {
        let text = concat!(
            "struct Answer { value: Int }\n",
            "struct Other { value: Int }\n",
            "pub fn main() { Answer(value: 42) }\n",
        );
        let report = Compiler::check(source(text));

        assert!(!report.is_success());
        assert_eq!(report.diagnostics()[0].stage(), DiagnosticStage::Lowering);
        assert_eq!(report.diagnostics()[0].code(), "L2001");
    }
}
