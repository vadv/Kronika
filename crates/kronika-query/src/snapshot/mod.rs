//! Typed current-state snapshot query semantics.

mod search;

pub use search::{
    Expr, GlobPattern, Quantity, QuantityKind, ResultField, SEARCH_MAX_CLAUSES,
    SEARCH_MAX_VALUE_CHARS, SearchClause, SearchDiagnostic, SearchField, SearchFieldKind,
    SearchOperator, SearchValue, StructuredSearch, result_field, search_fields,
    search_value_matches, valid_identifier,
};
