//! Narrow adapter used by the feature-gated native frontend benchmark.

use crate::source::SourceSnapshot;
use crate::syntax::{self, ParseResult};

pub struct ParsedFrontend {
    parsed: ParseResult,
}

#[derive(Clone, Copy, Debug)]
pub struct FrontendStatistics {
    pub diagnostic_count: usize,
    pub node_count: usize,
    pub token_count: usize,
    pub missing_token_count: usize,
}

pub fn parse(source: &SourceSnapshot) -> ParsedFrontend {
    ParsedFrontend {
        parsed: syntax::parse(source),
    }
}

impl ParsedFrontend {
    pub fn validate(&self, source: &SourceSnapshot) -> Result<FrontendStatistics, String> {
        let statistics = syntax::validate_parse(&self.parsed, source)?;
        Ok(FrontendStatistics {
            diagnostic_count: self.parsed.diagnostics().len(),
            node_count: statistics.node_count,
            token_count: statistics.token_count,
            missing_token_count: statistics.missing_token_count,
        })
    }
}
