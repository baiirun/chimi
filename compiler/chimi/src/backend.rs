use crate::executable::{CoreLiteral, CoreOperation, CorePattern, CoreProgram};
use crate::execution::{ExecutionConfig, ExecutionReport, execute};

#[cfg(any(
    feature = "embedded-bytecode-prototype",
    feature = "native-aot-prototype"
))]
mod prototype_program_data;

#[cfg(feature = "native-aot-prototype")]
#[doc(hidden)]
pub mod native_aot_prototype;

#[cfg(feature = "embedded-bytecode-prototype")]
#[doc(hidden)]
pub mod embedded_bytecode_prototype;

/// The backend-neutral executable input produced by the verified compiler pipeline.
///
/// Its representation remains opaque outside the compiler crate so backend layout
/// decisions cannot become source-language or typed-contract API by accident.
#[derive(Clone)]
pub struct BackendProgram {
    core: CoreProgram,
}

impl BackendProgram {
    pub(crate) const fn new(core: CoreProgram) -> Self {
        Self { core }
    }

    pub fn profile(&self) -> BackendProfile {
        BackendProfile::from_core(&self.core)
    }

    /// Execute through the disposable interpreter used as the backend conformance oracle.
    pub fn execute_oracle(&self, config: ExecutionConfig) -> ExecutionReport {
        execute(&self.core, config)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BackendProfile {
    pub types: usize,
    pub symbols: usize,
    pub functions: usize,
    pub bindings: usize,
    pub nodes: usize,
    pub origins: usize,
    pub operations: BackendOperationProfile,
    pub values: BackendValueProfile,
}

impl BackendProfile {
    fn from_core(core: &CoreProgram) -> Self {
        let mut profile = Self {
            types: core.type_count(),
            symbols: core.symbols.len(),
            functions: core.functions.len(),
            bindings: core.bindings.len(),
            nodes: core.nodes.len(),
            origins: core.origins.len(),
            ..Self::default()
        };

        for node in &core.nodes {
            profile.record_operation(&node.operation);
        }
        profile
    }

    fn record_operation(&mut self, operation: &CoreOperation) {
        let operations = &mut self.operations;
        let values = &mut self.values;
        match operation {
            CoreOperation::Constant(literal) => {
                operations.constants += 1;
                values.record_literal(literal);
            }
            CoreOperation::Reference(_) => operations.references += 1,
            CoreOperation::Select { .. } => operations.selects += 1,
            CoreOperation::DependencyMethod { .. } => operations.dependency_methods += 1,
            CoreOperation::Construct { arguments, .. } => {
                operations.constructions += 1;
                values.constructor_arguments += arguments.len();
                values.max_constructor_arguments =
                    values.max_constructor_arguments.max(arguments.len());
            }
            CoreOperation::Call { arguments, .. } => {
                operations.calls += 1;
                values.call_arguments += arguments.len();
                values.max_call_arguments = values.max_call_arguments.max(arguments.len());
            }
            CoreOperation::List(elements) => {
                operations.lists += 1;
                values.list_elements += elements.len();
                values.max_list_elements = values.max_list_elements.max(elements.len());
            }
            CoreOperation::Record(fields) => {
                operations.records += 1;
                values.record_fields += fields.len();
                values.max_record_fields = values.max_record_fields.max(fields.len());
            }
            CoreOperation::Unary { .. } => operations.unary += 1,
            CoreOperation::Binary { .. } => operations.binary += 1,
            CoreOperation::Identity(_) => operations.identities += 1,
            CoreOperation::Provision { providers, .. } => {
                operations.provisions += 1;
                values.provider_fields += providers.len();
                values.max_provider_fields = values.max_provider_fields.max(providers.len());
            }
            CoreOperation::Sequence(steps) => {
                operations.sequences += 1;
                values.sequence_steps += steps.len();
                values.max_sequence_steps = values.max_sequence_steps.max(steps.len());
            }
            CoreOperation::Closure { parameters, .. } => {
                operations.closures += 1;
                values.closure_parameters += parameters.len();
                values.max_closure_parameters = values.max_closure_parameters.max(parameters.len());
            }
            CoreOperation::Match { subjects, arms } => {
                operations.matches += 1;
                values.match_subjects += subjects.len();
                values.match_arms += arms.len();
                values.max_match_subjects = values.max_match_subjects.max(subjects.len());
                values.max_match_arms = values.max_match_arms.max(arms.len());
                for arm in arms {
                    for pattern in &arm.patterns {
                        values.record_pattern(pattern);
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BackendOperationProfile {
    pub constants: usize,
    pub references: usize,
    pub selects: usize,
    pub dependency_methods: usize,
    pub constructions: usize,
    pub calls: usize,
    pub lists: usize,
    pub records: usize,
    pub unary: usize,
    pub binary: usize,
    pub identities: usize,
    pub provisions: usize,
    pub sequences: usize,
    pub closures: usize,
    pub matches: usize,
}

impl BackendOperationProfile {
    pub const fn total(self) -> usize {
        self.constants
            + self.references
            + self.selects
            + self.dependency_methods
            + self.constructions
            + self.calls
            + self.lists
            + self.records
            + self.unary
            + self.binary
            + self.identities
            + self.provisions
            + self.sequences
            + self.closures
            + self.matches
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BackendValueProfile {
    pub integer_literals: usize,
    pub text_literals: usize,
    pub text_literal_bytes: usize,
    pub boolean_literals: usize,
    pub list_elements: usize,
    pub record_fields: usize,
    pub provider_fields: usize,
    pub constructor_arguments: usize,
    pub call_arguments: usize,
    pub sequence_steps: usize,
    pub closure_parameters: usize,
    pub match_subjects: usize,
    pub match_arms: usize,
    pub pattern_nodes: usize,
    pub max_list_elements: usize,
    pub max_record_fields: usize,
    pub max_provider_fields: usize,
    pub max_constructor_arguments: usize,
    pub max_call_arguments: usize,
    pub max_sequence_steps: usize,
    pub max_closure_parameters: usize,
    pub max_match_subjects: usize,
    pub max_match_arms: usize,
    pub max_pattern_depth: usize,
}

impl BackendValueProfile {
    fn record_literal(&mut self, literal: &CoreLiteral) {
        match literal {
            CoreLiteral::Integer(_) => self.integer_literals += 1,
            CoreLiteral::Text(value) => {
                self.text_literals += 1;
                self.text_literal_bytes += value.len();
            }
            CoreLiteral::Boolean(_) => self.boolean_literals += 1,
        }
    }

    fn record_pattern(&mut self, pattern: &CorePattern) {
        self.max_pattern_depth = self.max_pattern_depth.max(pattern_depth(pattern));
        self.record_pattern_nodes(pattern);
    }

    fn record_pattern_nodes(&mut self, pattern: &CorePattern) {
        self.pattern_nodes += 1;
        match pattern {
            CorePattern::Literal(literal) => self.record_literal(literal),
            CorePattern::Constructor { fields, .. } => {
                for field in fields {
                    self.record_pattern_nodes(field);
                }
            }
            CorePattern::Outcome { value, .. } => self.record_pattern_nodes(value),
            CorePattern::Wildcard | CorePattern::Binding(_) => {}
        }
    }
}

fn pattern_depth(pattern: &CorePattern) -> usize {
    1 + match pattern {
        CorePattern::Constructor { fields, .. } => {
            fields.iter().map(pattern_depth).max().unwrap_or(0)
        }
        CorePattern::Outcome { value, .. } => pattern_depth(value),
        CorePattern::Wildcard | CorePattern::Binding(_) | CorePattern::Literal(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use crate::compiler::{Compiler, ProjectSource};
    use crate::execution::{EventKind, ExecutionConfig, ExecutionOutcome, TraceMode};
    use crate::source::SourceSnapshot;

    fn rich_program() -> super::BackendProgram {
        Compiler::prepare_backend(vec![
            ProjectSource::new(
                "comparison/rich",
                SourceSnapshot::from_text(
                    "prototypes/backend-comparison/rich-success.chimi",
                    include_str!("../../../prototypes/backend-comparison/rich-success.chimi"),
                ),
            )
            .expect("comparison module path is valid"),
        ])
        .expect("rich backend workload prepares")
    }

    #[test]
    fn profile_accounts_for_every_core_node_and_runtime_shape() {
        let profile = rich_program().profile();
        assert_eq!(profile.operations.total(), profile.nodes);
        let operations = profile.operations;
        assert!(
            [
                operations.constants,
                operations.references,
                operations.selects,
                operations.dependency_methods,
                operations.constructions,
                operations.calls,
                operations.lists,
                operations.records,
                operations.unary,
                operations.binary,
                operations.identities,
                operations.provisions,
                operations.sequences,
                operations.closures,
                operations.matches,
            ]
            .into_iter()
            .all(|count| count > 0)
        );
        assert!(profile.values.boolean_literals >= 3);
        assert!(profile.values.max_match_arms >= 2);
        assert!(profile.values.max_sequence_steps >= 2);
    }

    #[test]
    fn prepared_program_preserves_oracle_outcome_across_trace_modes() {
        let program = rich_program();
        let full = program
            .execute_oracle(ExecutionConfig::new("backend-full").expect("execution ID is valid"));
        let boundaries = program.execute_oracle(
            ExecutionConfig::new("backend-boundaries")
                .expect("execution ID is valid")
                .with_trace_mode(TraceMode::Boundaries),
        );
        let off = program.execute_oracle(
            ExecutionConfig::new("backend-off")
                .expect("execution ID is valid")
                .with_trace_mode(TraceMode::Off),
        );

        assert_eq!(boundaries.outcome, full.outcome);
        assert_eq!(off.outcome, full.outcome);
        assert!(off.events.is_empty());
        assert!(matches!(full.outcome, ExecutionOutcome::Success(_)));
        assert_eq!(boundaries.events[0].kind, EventKind::ComputationStarted);
        assert_eq!(
            boundaries.events.last().expect("completion event").kind,
            EventKind::ComputationCompleted
        );
        assert!(full.events.len() > boundaries.events.len());
    }
}
