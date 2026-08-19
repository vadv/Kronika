use super::GlobPattern;

pub(super) const SEARCH_MAX_CLAUSES: usize = 8;
pub(super) const SEARCH_MAX_VALUE_CHARS: usize = 256;
const SEARCH_MAX_EXPRESSION_CHARS: usize = 1_024;
const SEARCH_MAX_GROUP_DEPTH: usize = 4;
const SEARCH_MAX_TOKENS: usize = 31;
const SEARCH_MAX_SIGNIFICANT_DIGITS: usize = 38;
const SEARCH_MAX_FRACTIONAL_DIGITS: usize = 9;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StructuredSearch {
    pub(super) expr: Expr,
    pub(super) clauses: Vec<SearchClause>,
    canonical: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Expr {
    Predicate(SearchClause),
    And(Box<Self>, Box<Self>),
    Or {
        left: Box<Self>,
        right: Box<Self>,
        operator_span: (usize, usize),
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SearchClause {
    canonical: String,
    pub(super) key: &'static str,
    pub(super) operator: SearchOperator,
    pub(super) value: SearchValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SearchOperator {
    Colon,
    Greater,
    Less,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SearchValue {
    Identifier(String),
    Pattern(GlobPattern),
    Quantity(Quantity),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Quantity {
    pub(super) numerator: u128,
    pub(super) denominator: u128,
    canonical: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum QuantityKind {
    ByteRate,
    Bytes,
    Count,
    CountRate,
    Duration,
    DurationRate,
    Percentage,
    Scalar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ResultField {
    pub(super) metric: &'static str,
    pub(super) kind: QuantityKind,
    pub(super) dependencies: &'static [&'static str],
}

#[derive(Clone, Copy)]
pub(super) struct SearchField {
    pub(super) key: &'static str,
    aliases: &'static [&'static str],
    pub(super) columns: &'static [&'static str],
    kind: SearchFieldKind,
}

#[derive(Clone, Copy)]
enum SearchFieldKind {
    Identifier { signed: bool },
    String,
    Quantity(ResultField),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SearchDiagnostic {
    pub(super) code: &'static str,
    pub(super) start: usize,
    pub(super) end: usize,
}

impl StructuredSearch {
    pub(super) fn parse(raw: &str, logical_name: &str) -> Result<Self, SearchDiagnostic> {
        if raw.chars().count() > SEARCH_MAX_EXPRESSION_CHARS {
            return Err(diagnostic("expression_too_long", 0, raw.len()));
        }
        let fields = search_fields(logical_name);
        if fields.is_empty() {
            return Err(diagnostic("unknown_field", 0, raw.len()));
        }
        let first = skip_space(raw, 0);
        if first == raw.len() {
            return Err(diagnostic("missing_value", first, first));
        }
        if let Some((start, end)) = first_unsupported(raw) {
            return Err(diagnostic("unsupported_syntax", start, end));
        }
        if !has_structured_syntax(raw) {
            let value = raw.trim();
            if value.chars().count() > SEARCH_MAX_VALUE_CHARS {
                return Err(diagnostic("value_too_long", first, raw.len()));
            }
            let clause = SearchClause {
                canonical: value.to_owned(),
                key: "text",
                operator: SearchOperator::Colon,
                value: SearchValue::Pattern(GlobPattern::new(value)),
            };
            return Ok(Self {
                expr: Expr::Predicate(clause.clone()),
                clauses: vec![clause],
                canonical: value.to_owned(),
            });
        }
        let mut parser = Parser::new(raw, fields, first);
        let expr = parser.parse_expression(0)?;
        parser.cursor = skip_space(raw, parser.cursor);
        if parser.cursor != raw.len() {
            if raw.as_bytes().get(parser.cursor) == Some(&b')') {
                return Err(diagnostic(
                    "unbalanced_parenthesis",
                    parser.cursor,
                    parser.cursor + 1,
                ));
            }
            return Err(diagnostic(
                "expected_boolean_operator",
                parser.cursor,
                next_token(raw, parser.cursor),
            ));
        }
        let canonical = canonical_expr(&expr, 0);
        Ok(Self {
            expr,
            clauses: parser.clauses,
            canonical,
        })
    }

    pub(super) fn canonical(&self) -> &str {
        &self.canonical
    }

    pub(super) fn member_clauses(&self) -> impl Iterator<Item = &SearchClause> {
        self.clauses
            .iter()
            .filter(|clause| !matches!(clause.value, SearchValue::Quantity(_)))
    }

    pub(super) fn validate_grouped_phase(&self) -> Result<(), SearchDiagnostic> {
        phase(&self.expr).map(|_phase| ())
    }

    pub(super) fn matches_member(&self, mut predicate: impl FnMut(&SearchClause) -> bool) -> bool {
        evaluate(&self.expr, &mut |clause| {
            matches!(clause.value, SearchValue::Quantity(_)) || predicate(clause)
        })
    }

    pub(super) fn matches_result(&self, mut predicate: impl FnMut(&SearchClause) -> bool) -> bool {
        evaluate(&self.expr, &mut |clause| {
            !matches!(clause.value, SearchValue::Quantity(_)) || predicate(clause)
        })
    }

    pub(super) fn matches_all(&self, mut predicate: impl FnMut(&SearchClause) -> bool) -> bool {
        evaluate(&self.expr, &mut predicate)
    }

    pub(super) fn result_clauses(
        &self,
        logical_name: &str,
    ) -> impl Iterator<Item = (&SearchClause, ResultField)> {
        self.clauses.iter().filter_map(|clause| {
            let field = search_fields(logical_name)
                .iter()
                .find(|field| field.key == clause.key)?;
            match field.kind {
                SearchFieldKind::Quantity(result) => Some((clause, result)),
                SearchFieldKind::Identifier { .. } | SearchFieldKind::String => None,
            }
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Member,
    Result,
    Both,
}

fn phase(expr: &Expr) -> Result<Phase, SearchDiagnostic> {
    match expr {
        Expr::Predicate(clause) => Ok(if matches!(clause.value, SearchValue::Quantity(_)) {
            Phase::Result
        } else {
            Phase::Member
        }),
        Expr::And(left, right) => Ok(match (phase(left)?, phase(right)?) {
            (Phase::Member, Phase::Member) => Phase::Member,
            (Phase::Result, Phase::Result) => Phase::Result,
            _ => Phase::Both,
        }),
        Expr::Or {
            left,
            right,
            operator_span,
        } => {
            let left = phase(left)?;
            let right = phase(right)?;
            if left == right && !matches!(left, Phase::Both) {
                Ok(left)
            } else {
                Err(diagnostic(
                    "mixed_phase_or",
                    operator_span.0,
                    operator_span.1,
                ))
            }
        }
    }
}

fn evaluate(expr: &Expr, predicate: &mut impl FnMut(&SearchClause) -> bool) -> bool {
    match expr {
        Expr::Predicate(clause) => predicate(clause),
        Expr::And(left, right) => evaluate(left, predicate) && evaluate(right, predicate),
        Expr::Or { left, right, .. } => evaluate(left, predicate) || evaluate(right, predicate),
    }
}

fn canonical_expr(expr: &Expr, parent_precedence: u8) -> String {
    let (precedence, rendered) = match expr {
        Expr::Predicate(clause) => (3, canonical_clause(clause)),
        Expr::And(left, right) => (
            2,
            format!(
                "{} AND {}",
                canonical_expr(left, 2),
                canonical_expr(right, 2)
            ),
        ),
        Expr::Or { left, right, .. } => (
            1,
            format!(
                "{} OR {}",
                canonical_expr(left, 1),
                canonical_expr(right, 1)
            ),
        ),
    };
    if precedence < parent_precedence {
        format!("({rendered})")
    } else {
        rendered
    }
}

fn canonical_clause(clause: &SearchClause) -> String {
    clause.canonical.clone()
}

struct Parser<'a> {
    raw: &'a str,
    fields: &'static [SearchField],
    cursor: usize,
    clauses: Vec<SearchClause>,
    tokens: usize,
}

impl<'a> Parser<'a> {
    const fn new(raw: &'a str, fields: &'static [SearchField], cursor: usize) -> Self {
        Self {
            raw,
            fields,
            cursor,
            clauses: Vec::new(),
            tokens: 0,
        }
    }

    fn parse_expression(&mut self, depth: usize) -> Result<Expr, SearchDiagnostic> {
        self.parse_or(depth)
    }

    fn parse_or(&mut self, depth: usize) -> Result<Expr, SearchDiagnostic> {
        let mut left = self.parse_and(depth)?;
        loop {
            self.cursor = skip_space(self.raw, self.cursor);
            let Some((start, end)) = keyword_at(self.raw, self.cursor, "OR") else {
                return Ok(left);
            };
            self.consume_token(start, end)?;
            self.cursor = skip_space(self.raw, end);
            self.require_operand(start, end)?;
            let right = self.parse_and(depth)?;
            left = Expr::Or {
                left: Box::new(left),
                right: Box::new(right),
                operator_span: (start, end),
            };
        }
    }

    fn parse_and(&mut self, depth: usize) -> Result<Expr, SearchDiagnostic> {
        let mut left = self.parse_primary(depth)?;
        loop {
            self.cursor = skip_space(self.raw, self.cursor);
            let Some((start, end)) = keyword_at(self.raw, self.cursor, "AND") else {
                return Ok(left);
            };
            self.consume_token(start, end)?;
            self.cursor = skip_space(self.raw, end);
            self.require_operand(start, end)?;
            let right = self.parse_primary(depth)?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
    }

    fn parse_primary(&mut self, depth: usize) -> Result<Expr, SearchDiagnostic> {
        self.cursor = skip_space(self.raw, self.cursor);
        if let Some((start, end)) = keyword_at(self.raw, self.cursor, "NOT") {
            return Err(diagnostic("unsupported_syntax", start, end));
        }
        for keyword in ["AND", "OR"] {
            if let Some((start, end)) = keyword_at(self.raw, self.cursor, keyword) {
                return Err(diagnostic("missing_operand", start, end));
            }
        }
        if self.raw.as_bytes().get(self.cursor) == Some(&b')') {
            return Err(diagnostic(
                "unbalanced_parenthesis",
                self.cursor,
                self.cursor + 1,
            ));
        }
        if self.raw.as_bytes().get(self.cursor) != Some(&b'(') {
            return self.parse_predicate();
        }

        let open = self.cursor;
        self.consume_token(open, open + 1)?;
        if depth >= SEARCH_MAX_GROUP_DEPTH {
            return Err(diagnostic("group_too_deep", open, open + 1));
        }
        self.cursor = skip_space(self.raw, open + 1);
        if self.raw.as_bytes().get(self.cursor) == Some(&b')') {
            return Err(diagnostic("empty_group", open, self.cursor + 1));
        }
        if self.cursor == self.raw.len() {
            return Err(diagnostic("unbalanced_parenthesis", open, open + 1));
        }
        let expr = self.parse_expression(depth + 1)?;
        self.cursor = skip_space(self.raw, self.cursor);
        if self.raw.as_bytes().get(self.cursor) != Some(&b')') {
            return Err(diagnostic("unbalanced_parenthesis", open, open + 1));
        }
        let close = self.cursor;
        self.consume_token(close, close + 1)?;
        self.cursor += 1;
        Ok(expr)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "predicate parsing keeps field, operator, value, and diagnostic spans aligned"
    )]
    fn parse_predicate(&mut self) -> Result<Expr, SearchDiagnostic> {
        if self.cursor == self.raw.len() {
            return Err(diagnostic("missing_operand", self.cursor, self.cursor));
        }
        if self.clauses.len() >= SEARCH_MAX_CLAUSES {
            return Err(diagnostic("too_many_clauses", self.cursor, self.raw.len()));
        }
        let start = self.cursor;
        let key_start = self.cursor;
        while self
            .raw
            .as_bytes()
            .get(self.cursor)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            self.cursor += 1;
        }
        if self.cursor == key_start {
            return Err(diagnostic(
                "empty_clause",
                self.cursor,
                next_byte(self.raw, self.cursor),
            ));
        }
        let raw_key = self
            .raw
            .get(key_start..self.cursor)
            .expect("the ASCII field scan preserves UTF-8 boundaries");
        let Some(field) = self.fields.iter().find(|field| {
            field.key.eq_ignore_ascii_case(raw_key)
                || field
                    .aliases
                    .iter()
                    .any(|alias| alias.eq_ignore_ascii_case(raw_key))
        }) else {
            return Err(diagnostic("unknown_field", key_start, self.cursor));
        };

        self.cursor = skip_space(self.raw, self.cursor);
        let operator_start = self.cursor;
        while self
            .raw
            .as_bytes()
            .get(self.cursor)
            .is_some_and(|byte| matches!(byte, b'!' | b'<' | b'>' | b'=' | b':'))
        {
            self.cursor += 1;
        }
        let token = self
            .raw
            .get(operator_start..self.cursor)
            .expect("the ASCII operator scan preserves UTF-8 boundaries");
        let operator = match token {
            ":" => SearchOperator::Colon,
            ">" => SearchOperator::Greater,
            "<" => SearchOperator::Less,
            ">=" | "<=" | "==" | "!=" | "=" => {
                return Err(diagnostic(
                    "unsupported_operator",
                    operator_start,
                    self.cursor,
                ));
            }
            "" => {
                return Err(diagnostic(
                    "expected_colon",
                    operator_start,
                    next_byte(self.raw, operator_start),
                ));
            }
            _ => {
                return Err(diagnostic(
                    "malformed_operator",
                    operator_start,
                    self.cursor,
                ));
            }
        };
        let comparison = !matches!(operator, SearchOperator::Colon);
        if comparison != matches!(field.kind, SearchFieldKind::Quantity(_)) {
            return Err(diagnostic(
                "operator_not_allowed",
                operator_start,
                self.cursor,
            ));
        }
        self.cursor = skip_space(self.raw, self.cursor);
        if self.cursor == self.raw.len() || self.raw.as_bytes().get(self.cursor) == Some(&b')') {
            return Err(diagnostic("missing_value", self.cursor, self.cursor));
        }
        if comparison && self.raw.as_bytes().get(self.cursor) == Some(&b'"') {
            let (_, end) = parse_quoted(self.raw, self.cursor)?;
            return Err(diagnostic("quoted_quantity", self.cursor, end));
        }
        let value_start = self.cursor;
        let (value, next, quoted) = parse_value(self.raw, self.cursor)?;
        if value.is_empty() {
            return Err(diagnostic("missing_value", self.cursor, next));
        }
        if value.chars().count() > SEARCH_MAX_VALUE_CHARS {
            return Err(diagnostic("value_too_long", self.cursor, next));
        }
        let parsed_value = match field.kind {
            SearchFieldKind::String => SearchValue::Pattern(GlobPattern::new(&value)),
            SearchFieldKind::Identifier { signed } => {
                if !valid_identifier(&value, signed) {
                    return Err(diagnostic("invalid_identifier", self.cursor, next));
                }
                SearchValue::Identifier(value.clone())
            }
            SearchFieldKind::Quantity(result) => {
                if quoted {
                    return Err(diagnostic("quoted_quantity", self.cursor, next));
                }
                let after = skip_space(self.raw, next);
                if after > next {
                    let unit_end = next_token(self.raw, after);
                    let unit = self
                        .raw
                        .get(after..unit_end)
                        .expect("the unit scan preserves UTF-8 boundaries");
                    if looks_like_unit(unit) {
                        return Err(diagnostic("whitespace_before_unit", after, unit_end));
                    }
                }
                SearchValue::Quantity(parse_quantity(&value, result.kind, value_start)?)
            }
        };
        self.cursor = next;
        let operator_text = match operator {
            SearchOperator::Colon => ":",
            SearchOperator::Greater => ">",
            SearchOperator::Less => "<",
        };
        let canonical_value = match &parsed_value {
            SearchValue::Identifier(value) => value.clone(),
            SearchValue::Pattern(_) => canonical_value(&value),
            SearchValue::Quantity(quantity) => quantity.canonical.clone(),
        };
        let clause = SearchClause {
            canonical: format!("{}{operator_text}{canonical_value}", field.key),
            key: field.key,
            operator,
            value: parsed_value,
        };
        self.consume_token(start, next)?;
        self.clauses.push(clause.clone());
        Ok(Expr::Predicate(clause))
    }

    fn require_operand(
        &self,
        operator_start: usize,
        operator_end: usize,
    ) -> Result<(), SearchDiagnostic> {
        if self.cursor == self.raw.len() || self.raw.as_bytes().get(self.cursor) == Some(&b')') {
            return Err(diagnostic("missing_operand", operator_start, operator_end));
        }
        for keyword in ["AND", "OR"] {
            if let Some((start, end)) = keyword_at(self.raw, self.cursor, keyword) {
                return Err(diagnostic("missing_operand", start, end));
            }
        }
        Ok(())
    }

    const fn consume_token(&mut self, start: usize, end: usize) -> Result<(), SearchDiagnostic> {
        if self.tokens >= SEARCH_MAX_TOKENS {
            return Err(diagnostic("too_many_tokens", start, end));
        }
        self.tokens += 1;
        Ok(())
    }
}

pub(super) fn search_fields(logical_name: &str) -> &'static [SearchField] {
    match logical_name {
        "os_process" => PROCESS_SEARCH_FIELDS,
        "pg_stat_statements" => STATEMENT_SEARCH_FIELDS,
        "pg_store_plans" => PLAN_SEARCH_FIELDS,
        "pg_stat_user_tables" => TABLE_SEARCH_FIELDS,
        "pg_stat_user_indexes" => INDEX_SEARCH_FIELDS,
        _ => &[],
    }
}

pub(super) fn result_field(logical_name: &str, key: &str) -> Option<ResultField> {
    let field = search_fields(logical_name)
        .iter()
        .find(|field| field.key == key)?;
    match field.kind {
        SearchFieldKind::Quantity(result) => Some(result),
        SearchFieldKind::Identifier { .. } | SearchFieldKind::String => None,
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "one exhaustive quantity match keeps every public unit and bound in one audited conversion"
)]
#[expect(
    clippy::string_slice,
    reason = "the split position is found by scanning ASCII numeric bytes"
)]
fn parse_quantity(
    raw: &str,
    kind: QuantityKind,
    offset: usize,
) -> Result<Quantity, SearchDiagnostic> {
    if raw.starts_with('-') {
        return Err(diagnostic(
            "negative_not_allowed",
            offset,
            offset + raw.len(),
        ));
    }
    if raw.starts_with('+') {
        return Err(diagnostic("invalid_number", offset, offset + raw.len()));
    }
    if raw.contains(',') || raw.contains('_') {
        return Err(diagnostic("invalid_number", offset, offset + raw.len()));
    }
    let number_end = raw
        .bytes()
        .position(|byte| !(byte.is_ascii_digit() || byte == b'.'))
        .unwrap_or(raw.len());
    let number = &raw[..number_end];
    let unit = &raw[number_end..];
    if number.is_empty()
        || number.matches('.').count() > 1
        || number.ends_with('.')
        || number.contains(',')
        || number.contains('_')
        || unit.strip_prefix(['e', 'E']).is_some_and(|rest| {
            rest.starts_with(|character: char| {
                character.is_ascii_digit() || matches!(character, '+' | '-')
            })
        })
    {
        return Err(diagnostic("invalid_number", offset, offset + raw.len()));
    }
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || (whole.len() > 1 && whole.starts_with('0'))
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(diagnostic("invalid_number", offset, offset + number_end));
    }
    let significant = format!("{whole}{fraction}");
    let significant_digits = significant.trim_start_matches('0').len().max(1);
    if significant_digits > SEARCH_MAX_SIGNIFICANT_DIGITS
        || fraction.len() > SEARCH_MAX_FRACTIONAL_DIGITS
    {
        return Err(diagnostic("out_of_range", offset, offset + number_end));
    }
    let coefficient = significant
        .parse::<u128>()
        .map_err(|_error| diagnostic("out_of_range", offset, offset + number_end))?;
    let scale = checked_power(10, fraction.len())
        .ok_or_else(|| diagnostic("out_of_range", offset, offset + number_end))?;
    let trimmed = fraction.trim_end_matches('0');
    let canonical_number = if trimmed.is_empty() {
        whole.to_owned()
    } else {
        format!("{whole}.{trimmed}")
    };
    let (mut numerator, mut denominator) = match kind {
        QuantityKind::Bytes | QuantityKind::ByteRate => {
            if unit.is_empty() {
                return Err(diagnostic(
                    "unit_required",
                    offset + number_end,
                    offset + raw.len(),
                ));
            }
            let suffix = if kind == QuantityKind::ByteRate {
                "/s"
            } else {
                ""
            };
            let byte_unit = unit
                .strip_suffix(suffix)
                .filter(|unit| !unit.is_empty())
                .ok_or_else(|| {
                    diagnostic("invalid_unit", offset + number_end, offset + raw.len())
                })?;
            let multiplier = byte_multiplier(byte_unit).ok_or_else(|| {
                diagnostic("invalid_unit", offset + number_end, offset + raw.len())
            })?;
            let scaled = coefficient
                .checked_mul(multiplier)
                .ok_or_else(|| diagnostic("out_of_range", offset, offset + raw.len()))?;
            if kind == QuantityKind::Bytes && scaled % scale != 0 {
                return Err(diagnostic(
                    "non_integral_base_value",
                    offset,
                    offset + raw.len(),
                ));
            }
            if kind == QuantityKind::Bytes {
                (scaled / scale, 1)
            } else {
                (scaled, scale)
            }
        }
        QuantityKind::Duration | QuantityKind::DurationRate => {
            if unit.is_empty() {
                return Err(diagnostic(
                    "unit_required",
                    offset + number_end,
                    offset + raw.len(),
                ));
            }
            let duration_unit = if kind == QuantityKind::DurationRate {
                unit.strip_suffix("/s").filter(|unit| !unit.is_empty())
            } else {
                Some(unit)
            };
            let (numerator, denominator) =
                duration_unit.and_then(duration_factors).ok_or_else(|| {
                    diagnostic("invalid_unit", offset + number_end, offset + raw.len())
                })?;
            if kind == QuantityKind::DurationRate
                && !matches!(duration_unit, Some("ns" | "us" | "ms" | "s"))
            {
                return Err(diagnostic(
                    "invalid_unit",
                    offset + number_end,
                    offset + raw.len(),
                ));
            }
            (
                coefficient
                    .checked_mul(numerator)
                    .ok_or_else(|| diagnostic("out_of_range", offset, offset + raw.len()))?,
                scale
                    .checked_mul(denominator)
                    .ok_or_else(|| diagnostic("out_of_range", offset, offset + raw.len()))?,
            )
        }
        QuantityKind::Count => {
            if !unit.is_empty() {
                return Err(diagnostic(
                    "invalid_unit",
                    offset + number_end,
                    offset + raw.len(),
                ));
            }
            if !fraction.is_empty() {
                return Err(diagnostic(
                    "non_integral_base_value",
                    offset,
                    offset + raw.len(),
                ));
            }
            (coefficient, 1)
        }
        QuantityKind::CountRate => {
            if unit.is_empty() {
                return Err(diagnostic(
                    "unit_required",
                    offset + number_end,
                    offset + raw.len(),
                ));
            }
            if unit != "/s" {
                return Err(diagnostic(
                    "invalid_unit",
                    offset + number_end,
                    offset + raw.len(),
                ));
            }
            (coefficient, scale)
        }
        QuantityKind::Percentage => {
            if unit.is_empty() {
                return Err(diagnostic(
                    "unit_required",
                    offset + number_end,
                    offset + raw.len(),
                ));
            }
            if unit != "%" {
                return Err(diagnostic(
                    "invalid_unit",
                    offset + number_end,
                    offset + raw.len(),
                ));
            }
            if coefficient > 100_u128.saturating_mul(scale) {
                return Err(diagnostic("out_of_range", offset, offset + raw.len()));
            }
            (coefficient, scale)
        }
        QuantityKind::Scalar => {
            if !unit.is_empty() {
                return Err(diagnostic(
                    "invalid_unit",
                    offset + number_end,
                    offset + raw.len(),
                ));
            }
            (coefficient, scale)
        }
    };
    let divisor = greatest_common_divisor(numerator, denominator);
    numerator /= divisor;
    denominator /= divisor;
    Ok(Quantity {
        numerator,
        denominator,
        canonical: format!("{canonical_number}{unit}"),
    })
}

const fn byte_multiplier(unit: &str) -> Option<u128> {
    match unit.as_bytes() {
        b"B" => Some(1),
        b"kB" => Some(1_000),
        b"MB" => Some(1_000_000),
        b"GB" => Some(1_000_000_000),
        b"TB" => Some(1_000_000_000_000),
        b"PB" => Some(1_000_000_000_000_000),
        b"EB" => Some(1_000_000_000_000_000_000),
        b"KiB" => Some(1_024),
        b"MiB" => Some(1_048_576),
        b"GiB" => Some(1_073_741_824),
        b"TiB" => Some(1_099_511_627_776),
        b"PiB" => Some(1_125_899_906_842_624),
        b"EiB" => Some(1_152_921_504_606_846_976),
        _ => None,
    }
}

const fn duration_factors(unit: &str) -> Option<(u128, u128)> {
    match unit.as_bytes() {
        b"ns" => Some((1, 1_000_000)),
        b"us" => Some((1, 1_000)),
        b"ms" => Some((1, 1)),
        b"s" => Some((1_000, 1)),
        b"min" => Some((60_000, 1)),
        b"h" => Some((3_600_000, 1)),
        _ => None,
    }
}

const fn checked_power(base: u128, exponent: usize) -> Option<u128> {
    let mut result = 1_u128;
    let mut index = 0;
    while index < exponent {
        result = match result.checked_mul(base) {
            Some(value) => value,
            None => return None,
        };
        index += 1;
    }
    Some(result)
}

const fn greatest_common_divisor(mut left: u128, mut right: u128) -> u128 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

#[expect(
    clippy::string_slice,
    reason = "the caller provides a grammar boundary and next_space advances over whole ASCII bytes"
)]
fn parse_value(input: &str, start: usize) -> Result<(String, usize, bool), SearchDiagnostic> {
    if input.as_bytes().get(start) == Some(&b'"') {
        let (value, end) = parse_quoted(input, start)?;
        Ok((value, end, true))
    } else {
        let end = next_token(input, start);
        Ok((input[start..end].to_owned(), end, false))
    }
}

#[expect(
    clippy::string_slice,
    reason = "cursor starts at an ASCII quote and then advances by each character's UTF-8 width"
)]
fn parse_quoted(input: &str, start: usize) -> Result<(String, usize), SearchDiagnostic> {
    let mut value = String::new();
    let mut cursor = start + 1;
    while cursor < input.len() {
        let character = input[cursor..]
            .chars()
            .next()
            .ok_or_else(|| diagnostic("unterminated_quote", start, input.len()))?;
        if character == '"' {
            return Ok((value, cursor + 1));
        }
        if character == '\\' {
            let escaped = input[cursor + 1..]
                .chars()
                .next()
                .filter(|escaped| matches!(escaped, '"' | '\\'))
                .ok_or_else(|| {
                    diagnostic("invalid_escape", cursor, next_byte(input, cursor + 1))
                })?;
            value.push(escaped);
            cursor += 1 + escaped.len_utf8();
        } else {
            value.push(character);
            cursor += character.len_utf8();
        }
    }
    Err(diagnostic("unterminated_quote", start, input.len()))
}

fn canonical_value(value: &str) -> String {
    if value.bytes().all(|byte| {
        !byte.is_ascii_whitespace() && !matches!(byte, b':' | b'"' | b'\\' | b'(' | b')')
    }) {
        return value.to_owned();
    }
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn valid_identifier(value: &str, signed: bool) -> bool {
    if signed {
        if value == "-0" {
            return false;
        }
        let decimal = value.strip_prefix('-').unwrap_or(value);
        !decimal.is_empty()
            && (decimal.len() == 1 || !decimal.starts_with('0'))
            && decimal.bytes().all(|byte| byte.is_ascii_digit())
            && value.parse::<i64>().is_ok()
    } else {
        !value.is_empty()
            && (value.len() == 1 || !value.starts_with('0'))
            && value.bytes().all(|byte| byte.is_ascii_digit())
            && value.parse::<u64>().is_ok()
    }
}

fn has_structured_syntax(input: &str) -> bool {
    let mut quoted = false;
    let mut escaped = false;
    for (start, character) in input.char_indices() {
        if escaped {
            escaped = false;
        } else if quoted && character == '\\' {
            escaped = true;
        } else if character == '"' {
            quoted = !quoted;
        } else if !quoted {
            if matches!(character, ':' | '<' | '>' | '!' | '=' | '(' | ')') {
                return true;
            }
            if ["AND", "OR", "NOT"]
                .iter()
                .any(|keyword| keyword_at(input, start, keyword).is_some())
            {
                return true;
            }
        }
    }
    false
}

fn first_unsupported(input: &str) -> Option<(usize, usize)> {
    let mut quoted = false;
    let mut escaped = false;
    for (start, character) in input.char_indices() {
        if escaped {
            escaped = false;
        } else if quoted && character == '\\' {
            escaped = true;
        } else if character == '"' {
            quoted = !quoted;
        } else if !quoted && let Some(span) = keyword_at(input, start, "NOT") {
            return Some(span);
        }
    }
    None
}

fn keyword_at(input: &str, start: usize, keyword: &str) -> Option<(usize, usize)> {
    let end = start.checked_add(keyword.len())?;
    if !input
        .get(start..end)
        .is_some_and(|token| token.eq_ignore_ascii_case(keyword))
        || (start > 0
            && input
                .as_bytes()
                .get(start - 1)
                .is_some_and(|byte| !byte.is_ascii_whitespace() && !matches!(byte, b'(' | b')')))
        || input
            .as_bytes()
            .get(end)
            .is_some_and(|byte| !byte.is_ascii_whitespace() && !matches!(byte, b'(' | b')'))
    {
        return None;
    }
    Some((start, end))
}

fn looks_like_unit(token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    if token.eq_ignore_ascii_case("AND") || token.eq_ignore_ascii_case("OR") {
        return false;
    }
    byte_multiplier(token).is_some()
        || duration_factors(token).is_some()
        || matches!(token, "/s" | "%")
        || token
            .strip_suffix("/s")
            .is_some_and(|unit| byte_multiplier(unit).is_some() || duration_factors(unit).is_some())
        || token
            .bytes()
            .all(|byte| byte.is_ascii_alphabetic() || matches!(byte, b'/' | b'%'))
}

fn skip_space(input: &str, mut cursor: usize) -> usize {
    while input
        .as_bytes()
        .get(cursor)
        .is_some_and(u8::is_ascii_whitespace)
    {
        cursor += 1;
    }
    cursor
}

fn next_token(input: &str, mut cursor: usize) -> usize {
    while input
        .as_bytes()
        .get(cursor)
        .is_some_and(|byte| !byte.is_ascii_whitespace() && !matches!(byte, b'(' | b')'))
    {
        cursor += 1;
    }
    cursor
}

fn next_byte(input: &str, cursor: usize) -> usize {
    (cursor + 1).min(input.len())
}

const fn diagnostic(code: &'static str, start: usize, end: usize) -> SearchDiagnostic {
    SearchDiagnostic { code, start, end }
}

const fn search_string(
    key: &'static str,
    aliases: &'static [&'static str],
    columns: &'static [&'static str],
) -> SearchField {
    SearchField {
        key,
        aliases,
        columns,
        kind: SearchFieldKind::String,
    }
}

const fn search_id(
    key: &'static str,
    aliases: &'static [&'static str],
    columns: &'static [&'static str],
    signed: bool,
) -> SearchField {
    SearchField {
        key,
        aliases,
        columns,
        kind: SearchFieldKind::Identifier { signed },
    }
}

const fn search_quantity(
    key: &'static str,
    kind: QuantityKind,
    metric: &'static str,
    dependencies: &'static [&'static str],
) -> SearchField {
    SearchField {
        key,
        aliases: &[],
        columns: &[],
        kind: SearchFieldKind::Quantity(ResultField {
            metric,
            kind,
            dependencies,
        }),
    }
}

const fn search_quantity_aliases(
    key: &'static str,
    aliases: &'static [&'static str],
    kind: QuantityKind,
    metric: &'static str,
    dependencies: &'static [&'static str],
) -> SearchField {
    SearchField {
        key,
        aliases,
        columns: &[],
        kind: SearchFieldKind::Quantity(ResultField {
            metric,
            kind,
            dependencies,
        }),
    }
}

const STATEMENT_SEARCH_FIELDS: &[SearchField] = &[
    search_string("text", &["q"], &["query", "datname", "usename"]),
    search_id("query_id", &[], &["queryid"], true),
    search_string("database", &["db"], &["datname"]),
    search_string("role", &["user"], &["usename"]),
    search_quantity(
        "call_rate",
        QuantityKind::CountRate,
        "call_rate",
        &["calls"],
    ),
    search_quantity(
        "exec_time_rate",
        QuantityKind::DurationRate,
        "exec_time_rate",
        &["total_time", "total_exec_time"],
    ),
    search_quantity(
        "mean_exec",
        QuantityKind::Duration,
        "mean_exec",
        &["calls", "total_time", "total_exec_time"],
    ),
    search_quantity("row_rate", QuantityKind::CountRate, "row_rate", &["rows"]),
    search_quantity(
        "rows_per_call",
        QuantityKind::Scalar,
        "rows_per_call",
        &["calls", "rows"],
    ),
    search_quantity(
        "plan_rate",
        QuantityKind::CountRate,
        "plan_rate",
        &["plans"],
    ),
    search_quantity(
        "planning_time_rate",
        QuantityKind::DurationRate,
        "planning_time_rate",
        &["total_plan_time"],
    ),
    search_quantity(
        "planning_share",
        QuantityKind::Percentage,
        "planning_share",
        &["total_plan_time", "total_time", "total_exec_time"],
    ),
    search_quantity(
        "shared_buffer_hit_rate",
        QuantityKind::ByteRate,
        "shared_buffer_hit_rate",
        &["shared_blks_hit"],
    ),
    search_quantity(
        "shared_buffer_read_rate",
        QuantityKind::ByteRate,
        "shared_buffer_read_rate",
        &["shared_blks_read"],
    ),
    search_quantity(
        "shared_buffer_dirty_rate",
        QuantityKind::ByteRate,
        "shared_buffer_dirty_rate",
        &["shared_blks_dirtied"],
    ),
    search_quantity(
        "shared_buffer_write_rate",
        QuantityKind::ByteRate,
        "shared_buffer_write_rate",
        &["shared_blks_written"],
    ),
    search_quantity(
        "local_buffer_hit_rate",
        QuantityKind::ByteRate,
        "local_buffer_hit_rate",
        &["local_blks_hit"],
    ),
    search_quantity(
        "local_buffer_read_rate",
        QuantityKind::ByteRate,
        "local_buffer_read_rate",
        &["local_blks_read"],
    ),
    search_quantity(
        "local_buffer_dirty_rate",
        QuantityKind::ByteRate,
        "local_buffer_dirty_rate",
        &["local_blks_dirtied"],
    ),
    search_quantity(
        "local_buffer_write_rate",
        QuantityKind::ByteRate,
        "local_buffer_write_rate",
        &["local_blks_written"],
    ),
    search_quantity(
        "temp_buffer_read_rate",
        QuantityKind::ByteRate,
        "temp_buffer_read_rate",
        &["temp_blks_read"],
    ),
    search_quantity(
        "temp_buffer_write_rate",
        QuantityKind::ByteRate,
        "temp_buffer_write_rate",
        &["temp_blks_written"],
    ),
    search_quantity(
        "shared_read_time_rate",
        QuantityKind::DurationRate,
        "shared_read_time_rate",
        &["blk_read_time", "shared_blk_read_time"],
    ),
    search_quantity(
        "shared_write_time_rate",
        QuantityKind::DurationRate,
        "shared_write_time_rate",
        &["blk_write_time", "shared_blk_write_time"],
    ),
    search_quantity(
        "local_read_time_rate",
        QuantityKind::DurationRate,
        "local_read_time_rate",
        &["local_blk_read_time"],
    ),
    search_quantity(
        "local_write_time_rate",
        QuantityKind::DurationRate,
        "local_write_time_rate",
        &["local_blk_write_time"],
    ),
    search_quantity(
        "temp_read_time_rate",
        QuantityKind::DurationRate,
        "temp_read_time_rate",
        &["temp_blk_read_time"],
    ),
    search_quantity(
        "temp_write_time_rate",
        QuantityKind::DurationRate,
        "temp_write_time_rate",
        &["temp_blk_write_time"],
    ),
    search_quantity(
        "wal_rate",
        QuantityKind::ByteRate,
        "wal_rate",
        &["wal_bytes"],
    ),
    search_quantity(
        "wal_per_call",
        QuantityKind::Bytes,
        "wal_per_call",
        &["calls", "wal_bytes"],
    ),
    search_quantity(
        "buffer_hit",
        QuantityKind::Percentage,
        "buffer_hit",
        &["shared_blks_hit", "shared_blks_read"],
    ),
    search_quantity(
        "buffer_per_call",
        QuantityKind::Bytes,
        "buffer_per_call",
        &[
            "calls",
            "shared_blks_hit",
            "shared_blks_read",
            "shared_blks_dirtied",
            "shared_blks_written",
            "local_blks_hit",
            "local_blks_read",
            "local_blks_dirtied",
            "local_blks_written",
            "temp_blks_read",
            "temp_blks_written",
        ],
    ),
    search_quantity(
        "exec_cv",
        QuantityKind::Scalar,
        "exec_cv",
        &[
            "mean_time",
            "stddev_time",
            "mean_exec_time",
            "stddev_exec_time",
        ],
    ),
    search_quantity(
        "min_exec_since_reset",
        QuantityKind::Duration,
        "min_exec_since_reset",
        &["min_time", "min_exec_time"],
    ),
    search_quantity(
        "max_exec_since_reset",
        QuantityKind::Duration,
        "max_exec_since_reset",
        &["max_time", "max_exec_time"],
    ),
    search_quantity(
        "mean_exec_since_reset",
        QuantityKind::Duration,
        "mean_exec_since_reset",
        &["mean_time", "mean_exec_time"],
    ),
    search_quantity(
        "stddev_exec_since_reset",
        QuantityKind::Duration,
        "stddev_exec_since_reset",
        &["stddev_time", "stddev_exec_time"],
    ),
];
const PLAN_SEARCH_FIELDS: &[SearchField] = &[
    search_string("text", &["q"], &["plan", "datname", "usename"]),
    search_id(
        "query_id",
        &[],
        &["queryid", "queryid_stat_statements"],
        true,
    ),
    search_id("plan_id", &[], &["planid"], true),
    search_string("database", &["db"], &["datname"]),
    search_string("role", &["user"], &["usename"]),
    search_quantity(
        "call_rate",
        QuantityKind::CountRate,
        "call_rate",
        &["calls"],
    ),
    search_quantity(
        "exec_time_rate",
        QuantityKind::DurationRate,
        "exec_time_rate",
        &["total_time"],
    ),
    search_quantity(
        "mean_exec",
        QuantityKind::Duration,
        "mean_exec",
        &["calls", "total_time"],
    ),
    search_quantity("row_rate", QuantityKind::CountRate, "row_rate", &["rows"]),
    search_quantity(
        "rows_per_call",
        QuantityKind::Scalar,
        "rows_per_call",
        &["calls", "rows"],
    ),
    search_quantity(
        "planning_time_rate",
        QuantityKind::DurationRate,
        "planning_time_rate",
        &["total_plan_time"],
    ),
    search_quantity(
        "planning_share",
        QuantityKind::Percentage,
        "planning_share",
        &["total_plan_time", "total_time"],
    ),
    search_quantity(
        "shared_buffer_hit_rate",
        QuantityKind::ByteRate,
        "shared_buffer_hit_rate",
        &["shared_blks_hit"],
    ),
    search_quantity(
        "shared_buffer_read_rate",
        QuantityKind::ByteRate,
        "shared_buffer_read_rate",
        &["shared_blks_read"],
    ),
    search_quantity(
        "shared_buffer_dirty_rate",
        QuantityKind::ByteRate,
        "shared_buffer_dirty_rate",
        &["shared_blks_dirtied"],
    ),
    search_quantity(
        "shared_buffer_write_rate",
        QuantityKind::ByteRate,
        "shared_buffer_write_rate",
        &["shared_blks_written"],
    ),
    search_quantity(
        "local_buffer_hit_rate",
        QuantityKind::ByteRate,
        "local_buffer_hit_rate",
        &["local_blks_hit"],
    ),
    search_quantity(
        "local_buffer_read_rate",
        QuantityKind::ByteRate,
        "local_buffer_read_rate",
        &["local_blks_read"],
    ),
    search_quantity(
        "local_buffer_dirty_rate",
        QuantityKind::ByteRate,
        "local_buffer_dirty_rate",
        &["local_blks_dirtied"],
    ),
    search_quantity(
        "local_buffer_write_rate",
        QuantityKind::ByteRate,
        "local_buffer_write_rate",
        &["local_blks_written"],
    ),
    search_quantity(
        "temp_buffer_read_rate",
        QuantityKind::ByteRate,
        "temp_buffer_read_rate",
        &["temp_blks_read"],
    ),
    search_quantity(
        "temp_buffer_write_rate",
        QuantityKind::ByteRate,
        "temp_buffer_write_rate",
        &["temp_blks_written"],
    ),
    search_quantity(
        "shared_read_time_rate",
        QuantityKind::DurationRate,
        "shared_read_time_rate",
        &["blk_read_time", "shared_blk_read_time"],
    ),
    search_quantity(
        "shared_write_time_rate",
        QuantityKind::DurationRate,
        "shared_write_time_rate",
        &["blk_write_time", "shared_blk_write_time"],
    ),
    search_quantity(
        "local_read_time_rate",
        QuantityKind::DurationRate,
        "local_read_time_rate",
        &["local_blk_read_time"],
    ),
    search_quantity(
        "local_write_time_rate",
        QuantityKind::DurationRate,
        "local_write_time_rate",
        &["local_blk_write_time"],
    ),
    search_quantity(
        "temp_read_time_rate",
        QuantityKind::DurationRate,
        "temp_read_time_rate",
        &["temp_blk_read_time"],
    ),
    search_quantity(
        "temp_write_time_rate",
        QuantityKind::DurationRate,
        "temp_write_time_rate",
        &["temp_blk_write_time"],
    ),
    search_quantity(
        "buffer_hit",
        QuantityKind::Percentage,
        "buffer_hit",
        &["shared_blks_hit", "shared_blks_read"],
    ),
    search_quantity(
        "buffer_per_call",
        QuantityKind::Bytes,
        "buffer_per_call",
        &[
            "calls",
            "shared_blks_hit",
            "shared_blks_read",
            "shared_blks_dirtied",
            "shared_blks_written",
            "local_blks_hit",
            "local_blks_read",
            "local_blks_dirtied",
            "local_blks_written",
            "temp_blks_read",
            "temp_blks_written",
        ],
    ),
    search_quantity(
        "slow_call_rate",
        QuantityKind::CountRate,
        "slow_call_rate",
        &["slow_log_calls"],
    ),
    search_quantity(
        "exec_cv",
        QuantityKind::Scalar,
        "exec_cv",
        &["mean_time", "stddev_time"],
    ),
    search_quantity(
        "min_exec_since_reset",
        QuantityKind::Duration,
        "min_exec_since_reset",
        &["min_time"],
    ),
    search_quantity(
        "max_exec_since_reset",
        QuantityKind::Duration,
        "max_exec_since_reset",
        &["max_time"],
    ),
    search_quantity(
        "mean_exec_since_reset",
        QuantityKind::Duration,
        "mean_exec_since_reset",
        &["mean_time"],
    ),
    search_quantity(
        "stddev_exec_since_reset",
        QuantityKind::Duration,
        "stddev_exec_since_reset",
        &["stddev_time"],
    ),
];
const TABLE_SEARCH_FIELDS: &[SearchField] = &[
    search_string(
        "text",
        &["q"],
        &["datname", "schemaname", "relname", "tablespace"],
    ),
    search_string("database", &["db"], &["datname"]),
    search_string("schema", &[], &["schemaname"]),
    search_string("table_name", &["table"], &["relname"]),
    search_string("tablespace", &[], &["tablespace"]),
    search_quantity(
        "size",
        QuantityKind::Bytes,
        "displayed_storage_bytes",
        &["main_fork_bytes", "toast_bytes"],
    ),
    search_quantity("table_count", QuantityKind::Count, "table_count", &[]),
    search_quantity(
        "buffer_hit",
        QuantityKind::Percentage,
        "buffer_hit_pct",
        &[
            "heap_blks_hit",
            "heap_blks_read",
            "idx_blks_hit",
            "idx_blks_read",
            "toast_blks_hit",
            "toast_blks_read",
            "tidx_blks_hit",
            "tidx_blks_read",
        ],
    ),
    search_quantity(
        "seq_scan_rate",
        QuantityKind::CountRate,
        "seq_scan",
        &["seq_scan"],
    ),
    search_quantity(
        "change_rate",
        QuantityKind::CountRate,
        "dml_total",
        &["n_tup_ins", "n_tup_upd", "n_tup_del"],
    ),
    search_quantity(
        "autovacuum_rate",
        QuantityKind::CountRate,
        "autovacuum_count",
        &["autovacuum_count"],
    ),
    search_quantity(
        "autovacuum_mean",
        QuantityKind::Duration,
        "autovacuum_mean_ms",
        &["total_autovacuum_time", "autovacuum_count"],
    ),
    search_quantity("xid_age", QuantityKind::Count, "xid_age", &["xid_age"]),
];
const INDEX_SEARCH_FIELDS: &[SearchField] = &[
    search_string(
        "text",
        &["q"],
        &[
            "datname",
            "schemaname",
            "relname",
            "indexrelname",
            "tablespace",
            "amname",
            "indexdef",
        ],
    ),
    search_string("database", &["db"], &["datname"]),
    search_string("schema", &[], &["schemaname"]),
    search_string("table_name", &["table"], &["relname"]),
    search_string("index_name", &["index"], &["indexrelname"]),
    search_string("access_method", &["method"], &["amname"]),
    search_string("definition", &[], &["indexdef"]),
    search_string("tablespace", &[], &["tablespace"]),
    search_quantity(
        "size",
        QuantityKind::Bytes,
        "main_fork_bytes",
        &["main_fork_bytes"],
    ),
    search_quantity("index_count", QuantityKind::Count, "index_count", &[]),
    search_quantity(
        "buffer_hit",
        QuantityKind::Percentage,
        "buffer_hit_pct",
        &["idx_blks_hit", "idx_blks_read"],
    ),
    search_quantity(
        "scan_rate",
        QuantityKind::CountRate,
        "idx_scan",
        &["idx_scan"],
    ),
];
const PROCESS_SEARCH_FIELDS: &[SearchField] = &[
    search_string("text", &["q"], &["comm", "cmdline", "uid", "euid", "scope"]),
    search_string("user", &["username"], &["uid", "scope"]),
    search_string("effective_user", &["euser"], &["euid", "scope"]),
    search_id("user_id", &["uid"], &["uid"], false),
    search_id("effective_user_id", &["euid"], &["euid"], false),
    search_id("pid", &[], &["pid"], false),
    search_id("parent_pid", &["ppid"], &["ppid"], false),
    search_string("command", &["cmd"], &["comm", "cmdline"]),
    search_string("state", &[], &["state"]),
    search_quantity_aliases(
        "rss",
        &["resident_memory"],
        QuantityKind::Bytes,
        "rss",
        &["rmem_kb"],
    ),
    search_quantity_aliases(
        "vsz",
        &["virtual_memory"],
        QuantityKind::Bytes,
        "vsz",
        &["vmem_kb"],
    ),
    search_quantity("swap", QuantityKind::Bytes, "swap", &["vswap_kb"]),
    search_quantity("threads", QuantityKind::Count, "threads", &["num_threads"]),
    search_quantity(
        "cpu_cores",
        QuantityKind::Scalar,
        "cpu_cores",
        &["utime", "stime", "starttime"],
    ),
    search_quantity(
        "user_cpu_cores",
        QuantityKind::Scalar,
        "user_cpu_cores",
        &["utime", "starttime"],
    ),
    search_quantity(
        "system_cpu_cores",
        QuantityKind::Scalar,
        "system_cpu_cores",
        &["stime", "starttime"],
    ),
    search_quantity_aliases(
        "disk_read_rate",
        &["read_bytes_rate"],
        QuantityKind::ByteRate,
        "disk_read_rate",
        &["read_bytes", "starttime"],
    ),
    search_quantity_aliases(
        "disk_write_rate",
        &["write_bytes_rate"],
        QuantityKind::ByteRate,
        "disk_write_rate",
        &["write_bytes", "starttime"],
    ),
    search_quantity_aliases(
        "logical_read_rate",
        &["rchar_rate"],
        QuantityKind::ByteRate,
        "logical_read_rate",
        &["rchar", "starttime"],
    ),
    search_quantity_aliases(
        "logical_write_rate",
        &["wchar_rate"],
        QuantityKind::ByteRate,
        "logical_write_rate",
        &["wchar", "starttime"],
    ),
    search_quantity_aliases(
        "read_syscall_rate",
        &["syscr_rate"],
        QuantityKind::CountRate,
        "read_syscall_rate",
        &["syscr", "starttime"],
    ),
    search_quantity_aliases(
        "write_syscall_rate",
        &["syscw_rate"],
        QuantityKind::CountRate,
        "write_syscall_rate",
        &["syscw", "starttime"],
    ),
    search_quantity_aliases(
        "major_fault_rate",
        &["majflt_rate"],
        QuantityKind::CountRate,
        "major_fault_rate",
        &["majflt", "starttime"],
    ),
    search_quantity_aliases(
        "minor_fault_rate",
        &["minflt_rate"],
        QuantityKind::CountRate,
        "minor_fault_rate",
        &["minflt", "starttime"],
    ),
    search_quantity(
        "context_switch_rate",
        QuantityKind::CountRate,
        "context_switch_rate",
        &["nvcsw", "nivcsw", "starttime"],
    ),
    search_quantity_aliases(
        "voluntary_context_switch_rate",
        &["nvcsw_rate"],
        QuantityKind::CountRate,
        "voluntary_context_switch_rate",
        &["nvcsw", "starttime"],
    ),
    search_quantity_aliases(
        "involuntary_context_switch_rate",
        &["nivcsw_rate"],
        QuantityKind::CountRate,
        "involuntary_context_switch_rate",
        &["nivcsw", "starttime"],
    ),
    search_quantity_aliases(
        "run_delay",
        &["rundelay"],
        QuantityKind::DurationRate,
        "run_delay",
        &["rundelay_ns", "starttime"],
    ),
    search_quantity_aliases(
        "block_io_delay",
        &["blkdelay"],
        QuantityKind::DurationRate,
        "block_io_delay",
        &["blkdelay_ticks", "starttime"],
    ),
];
