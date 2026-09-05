//! Presentation scope for the statements Kronika's own collector runs.
//!
//! The collector prefixes every statement it issues with the exact comment
//! `/* kronika:`. A workload scope hides those rows from statement rankings,
//! summaries and pages so a reader sees only the application's statements. The
//! classification runs once per physical layout: the statement texts of the
//! layout are resolved through the segment dictionary and the ids of texts that
//! carry the prefix are kept. Rows are then filtered by dictionary id, so no
//! text is compared per row.

use std::collections::HashSet;

use kronika_reader::{ReaderError, Row, Segment};
use kronika_registry::Cell;

/// Exact byte prefix in front of every statement the collector runs itself.
pub const COLLECTOR_STATEMENT_PREFIX: &[u8] = b"/* kronika:";

/// Logical section whose rows carry the statement text the scope inspects.
pub const STATEMENTS_SECTION: &str = "pg_stat_statements";

const QUERY_COLUMN: &str = "query";

/// Which statements a statement-bearing result includes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum StatementScope {
    /// Every recorded statement, including the collector's own.
    #[default]
    All,
    /// Only statements whose text does not start with the collector prefix.
    Workload,
}

impl StatementScope {
    /// Parse the public `scope` parameter value.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "all" => Some(Self::All),
            "workload" => Some(Self::Workload),
            _ => None,
        }
    }

    /// Public parameter value of this scope.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Workload => "workload",
        }
    }

    /// Whether rows of `logical_name` are filtered under this scope.
    #[must_use]
    pub fn filters(self, logical_name: &str) -> bool {
        self == Self::Workload && logical_name == STATEMENTS_SECTION
    }
}

/// Dictionary ids of one layout's statement texts that begin with the collector prefix.
#[derive(Debug, Default)]
pub(crate) struct CollectorStatements {
    ids: HashSet<u64>,
}

impl CollectorStatements {
    /// Classify every statement text stored in `type_id` rows of `segment`.
    ///
    /// A layout without a statement text column yields no exclusions.
    ///
    /// # Errors
    /// Returns the reader error when the rows or the dictionary cannot be read.
    pub(crate) fn scan(segment: &Segment, type_id: u32) -> Result<Self, ReaderError> {
        let Some(column) = kronika_registry::contract(type_id)
            .and_then(|contract| contract.column(QUERY_COLUMN))
            .map(|column| column.name)
        else {
            return Ok(Self::default());
        };
        let mut seen = HashSet::new();
        segment.visit_rows(type_id, &[column], 0, usize::MAX, |_ordinal, row| {
            if let Some(Cell::StrId(id)) = row.get(column) {
                seen.insert(*id);
            }
            true
        })?;
        if seen.is_empty() {
            return Ok(Self::default());
        }
        let dictionary = segment.dictionary_for(&seen)?;
        let ids = seen
            .into_iter()
            .filter(|id| {
                dictionary.resolve(*id).is_some_and(|value| {
                    value.stored_bytes().starts_with(COLLECTOR_STATEMENT_PREFIX)
                })
            })
            .collect();
        Ok(Self { ids })
    }

    /// Whether `row` is one of the collector's own statements.
    pub(crate) fn excludes(&self, row: &Row) -> bool {
        matches!(row.get(QUERY_COLUMN), Some(Cell::StrId(id)) if self.ids.contains(id))
    }
}

#[cfg(test)]
mod tests {
    use super::StatementScope;

    #[test]
    fn scope_parses_only_its_two_public_values() {
        assert_eq!(StatementScope::parse("all"), Some(StatementScope::All));
        assert_eq!(
            StatementScope::parse("workload"),
            Some(StatementScope::Workload)
        );
        assert_eq!(StatementScope::parse("Workload"), None);
        assert_eq!(StatementScope::parse(""), None);
        assert_eq!(StatementScope::All.as_str(), "all");
        assert_eq!(StatementScope::Workload.as_str(), "workload");
    }

    #[test]
    fn only_the_workload_scope_filters_statements() {
        assert!(StatementScope::Workload.filters("pg_stat_statements"));
        assert!(!StatementScope::Workload.filters("pg_store_plans"));
        assert!(!StatementScope::All.filters("pg_stat_statements"));
    }
}
