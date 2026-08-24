//! Descriptors for values and boundaries computed by this crate.

/// Where a semantic value is defined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticOrigin {
    /// Computed by Kronika from recorded inputs.
    KronikaDerived,
}

/// Unit carried by a semantic value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticUnit {
    /// A value from zero to one hundred.
    Percent,
    /// Milliseconds.
    Milliseconds,
    /// An exact recorded count.
    Count,
}

/// Comparison used by one fixed indexed boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticOperator {
    /// Strictly less than.
    Lt,
    /// Less than or equal to.
    Lte,
    /// Equal to.
    Eq,
    /// Strictly greater than.
    Gt,
    /// Greater than or equal to.
    Gte,
}

/// One fixed boundary attached to an indexed finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticBoundary {
    /// Compare the formula result with one exact rational value.
    Compare {
        /// Comparison operator.
        operator: SemanticOperator,
        /// Signed numerator.
        numerator: i64,
        /// Positive denominator.
        denominator: u64,
    },
    /// A recorded counter increased from its preceding usable sample.
    Increase,
    /// A recorded list contains at least one value.
    Nonempty,
}

/// A stable description of one value or boundary computed by `kronika-index`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemanticDefinition {
    /// Stable identifier referenced by result values.
    pub id: &'static str,
    /// Logical source or derived series family.
    pub logical_name: Option<&'static str>,
    /// Recorded or derived result field.
    pub field: Option<&'static str>,
    /// Definition owner.
    pub origin: SemanticOrigin,
    /// Value unit, when the definition describes a numeric value.
    pub unit: Option<SemanticUnit>,
    /// Exact formula applied by the adjacent evaluator.
    pub formula: Option<&'static str>,
    /// Recorded or derived operands used by the evaluator.
    pub operands: &'static [&'static str],
    /// Fixed boundary, when the definition describes a finding.
    pub boundary: Option<SemanticBoundary>,
}

/// Resolve one evaluator-owned definition by its stable identifier.
#[must_use]
pub fn semantic_definition(id: &str) -> Option<SemanticDefinition> {
    crate::health::HEALTH_SEMANTICS
        .iter()
        .chain(crate::detect::FINDING_SEMANTICS)
        .find(|definition| definition.id == id)
        .copied()
}

#[cfg(test)]
#[path = "semantic/tests.rs"]
mod tests;
