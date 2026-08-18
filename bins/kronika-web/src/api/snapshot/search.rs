use super::GlobPattern;

pub(super) const SEARCH_MAX_CLAUSES: usize = 8;
pub(super) const SEARCH_MAX_VALUE_CHARS: usize = 256;
const SEARCH_MAX_EXPRESSION_BYTES: usize = 1_024;
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
    And(Vec<Self>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SearchClause {
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
    Bytes,
    Count,
    CountRate,
    Duration,
    Percentage,
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
    #[expect(
        clippy::too_many_lines,
        reason = "one bounded pass keeps diagnostics, canonicalization, and clause order aligned"
    )]
    #[expect(
        clippy::string_slice,
        reason = "all parser offsets advance only across ASCII grammar bytes or UTF-8 char widths"
    )]
    pub(super) fn parse(raw: &str, logical_name: &str) -> Result<Self, SearchDiagnostic> {
        if raw.len() > SEARCH_MAX_EXPRESSION_BYTES {
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
        if !has_structured_syntax(raw) {
            if let Some((start, end)) = standalone_and(raw) {
                return Err(diagnostic("expected_colon", start, end));
            }
            let value = raw.trim();
            if value.chars().count() > SEARCH_MAX_VALUE_CHARS {
                return Err(diagnostic("value_too_long", first, raw.len()));
            }
            let clause = SearchClause {
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

        let mut clauses = Vec::new();
        let mut canonical = Vec::new();
        let mut cursor = first;
        while cursor < raw.len() {
            if clauses.len() >= SEARCH_MAX_CLAUSES {
                return Err(diagnostic("too_many_clauses", cursor, raw.len()));
            }
            if let Some((start, end)) = reserved_at(raw, cursor) {
                return Err(diagnostic("unsupported_syntax", start, end));
            }
            let key_start = cursor;
            while raw
                .as_bytes()
                .get(cursor)
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
            {
                cursor += 1;
            }
            if cursor == key_start {
                return Err(diagnostic("empty_clause", cursor, next_byte(raw, cursor)));
            }
            let raw_key = &raw[key_start..cursor];
            let Some(field) = fields.iter().find(|field| {
                field.key.eq_ignore_ascii_case(raw_key)
                    || field
                        .aliases
                        .iter()
                        .any(|alias| alias.eq_ignore_ascii_case(raw_key))
            }) else {
                return Err(diagnostic("unknown_field", key_start, cursor));
            };

            cursor = skip_space(raw, cursor);
            let operator_start = cursor;
            while raw
                .as_bytes()
                .get(cursor)
                .is_some_and(|byte| matches!(byte, b'!' | b'<' | b'>' | b'=' | b':'))
            {
                cursor += 1;
            }
            let token = &raw[operator_start..cursor];
            let operator = match token {
                ":" => SearchOperator::Colon,
                ">" => SearchOperator::Greater,
                "<" => SearchOperator::Less,
                ">=" | "<=" | "==" | "!=" | "=" => {
                    return Err(diagnostic("unsupported_operator", operator_start, cursor));
                }
                "" => {
                    return Err(diagnostic(
                        "expected_colon",
                        operator_start,
                        next_byte(raw, operator_start),
                    ));
                }
                _ => return Err(diagnostic("malformed_operator", operator_start, cursor)),
            };
            let comparison = !matches!(operator, SearchOperator::Colon);
            if comparison != matches!(field.kind, SearchFieldKind::Quantity(_)) {
                return Err(diagnostic("operator_not_allowed", operator_start, cursor));
            }
            cursor = skip_space(raw, cursor);
            if cursor == raw.len() {
                return Err(diagnostic("missing_value", cursor, cursor));
            }
            if comparison && raw.as_bytes().get(cursor) == Some(&b'"') {
                let (_, end) = parse_quoted(raw, cursor)?;
                return Err(diagnostic("quoted_quantity", cursor, end));
            }
            let (value, next, quoted) = parse_value(raw, cursor)?;
            if value.is_empty() {
                return Err(diagnostic("missing_value", cursor, next));
            }
            if value.chars().count() > SEARCH_MAX_VALUE_CHARS {
                return Err(diagnostic("value_too_long", cursor, next));
            }

            let parsed_value = match field.kind {
                SearchFieldKind::String => SearchValue::Pattern(GlobPattern::new(&value)),
                SearchFieldKind::Identifier { signed } => {
                    if !valid_identifier(&value, signed) {
                        return Err(diagnostic("invalid_identifier", cursor, next));
                    }
                    SearchValue::Identifier(value.clone())
                }
                SearchFieldKind::Quantity(result) => {
                    if quoted {
                        return Err(diagnostic("quoted_quantity", cursor, next));
                    }
                    if let Some((offset, _)) =
                        value.char_indices().find(|(_, c)| matches!(c, '(' | ')'))
                    {
                        return Err(diagnostic(
                            "unsupported_syntax",
                            cursor + offset,
                            cursor + offset + 1,
                        ));
                    }
                    let after = skip_space(raw, next);
                    if after > next {
                        let unit_end = next_space(raw, after);
                        if looks_like_unit(&raw[after..unit_end]) {
                            return Err(diagnostic("whitespace_before_unit", after, unit_end));
                        }
                    }
                    SearchValue::Quantity(parse_quantity(&value, result.kind, cursor)?)
                }
            };
            let rendered_value = match &parsed_value {
                SearchValue::Identifier(value) => value.clone(),
                SearchValue::Pattern(_) => canonical_value(&value),
                SearchValue::Quantity(quantity) => quantity.canonical.clone(),
            };
            let rendered_operator = match operator {
                SearchOperator::Colon => ":",
                SearchOperator::Greater => ">",
                SearchOperator::Less => "<",
            };
            canonical.push(format!("{}{rendered_operator}{rendered_value}", field.key));
            clauses.push(SearchClause {
                key: field.key,
                operator,
                value: parsed_value,
            });
            cursor = skip_space(raw, next);
            if cursor == raw.len() {
                break;
            }
            if let Some((start, end)) = reserved_at(raw, cursor) {
                return Err(diagnostic("unsupported_syntax", start, end));
            }
            let rest = &raw[cursor..];
            if rest
                .get(..3)
                .is_none_or(|token| !token.eq_ignore_ascii_case("AND"))
                || rest
                    .as_bytes()
                    .get(3)
                    .is_none_or(|byte| !byte.is_ascii_whitespace())
            {
                return Err(diagnostic("expected_and", cursor, next_space(raw, cursor)));
            }
            cursor = skip_space(raw, cursor + 3);
            if cursor == raw.len() {
                return Err(diagnostic("empty_clause", raw.len() - 3, raw.len()));
            }
        }
        let expr = if clauses.len() == 1 {
            Expr::Predicate(clauses[0].clone())
        } else {
            Expr::And(clauses.iter().cloned().map(Expr::Predicate).collect())
        };
        Ok(Self {
            expr,
            clauses,
            canonical: canonical.join(" AND "),
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

pub(super) fn search_fields(logical_name: &str) -> &'static [SearchField] {
    match logical_name {
        "pg_stat_statements" => STATEMENT_SEARCH_FIELDS,
        "pg_store_plans" => PLAN_SEARCH_FIELDS,
        "pg_stat_user_tables" => TABLE_SEARCH_FIELDS,
        "pg_stat_user_indexes" => INDEX_SEARCH_FIELDS,
        _ => &[],
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
        QuantityKind::Bytes => {
            if unit.is_empty() {
                return Err(diagnostic(
                    "unit_required",
                    offset + number_end,
                    offset + raw.len(),
                ));
            }
            let multiplier = byte_multiplier(unit).ok_or_else(|| {
                diagnostic("invalid_unit", offset + number_end, offset + raw.len())
            })?;
            let scaled = coefficient
                .checked_mul(multiplier)
                .ok_or_else(|| diagnostic("out_of_range", offset, offset + raw.len()))?;
            if scaled % scale != 0 {
                return Err(diagnostic(
                    "non_integral_base_value",
                    offset,
                    offset + raw.len(),
                ));
            }
            (scaled / scale, 1)
        }
        QuantityKind::Duration => {
            if unit.is_empty() {
                return Err(diagnostic(
                    "unit_required",
                    offset + number_end,
                    offset + raw.len(),
                ));
            }
            let (numerator, denominator) = duration_factors(unit).ok_or_else(|| {
                diagnostic("invalid_unit", offset + number_end, offset + raw.len())
            })?;
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
        let end = next_space(input, start);
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
    if value
        .bytes()
        .all(|byte| !byte.is_ascii_whitespace() && !matches!(byte, b':' | b'"' | b'\\'))
    {
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
    for character in input.chars() {
        if escaped {
            escaped = false;
        } else if quoted && character == '\\' {
            escaped = true;
        } else if character == '"' {
            quoted = !quoted;
        } else if !quoted && matches!(character, ':' | '<' | '>' | '!' | '=') {
            return true;
        }
    }
    false
}

fn reserved_at(input: &str, start: usize) -> Option<(usize, usize)> {
    if matches!(input.as_bytes().get(start), Some(b'(' | b')')) {
        return Some((start, start + 1));
    }
    for token in ["NOT", "OR"] {
        let end = start + token.len();
        if input
            .get(start..end)
            .is_some_and(|value| value.eq_ignore_ascii_case(token))
            && (start == 0
                || input
                    .as_bytes()
                    .get(start - 1)
                    .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b'('))
            && input
                .as_bytes()
                .get(end)
                .is_none_or(|byte| byte.is_ascii_whitespace() || matches!(byte, b'(' | b')'))
        {
            return Some((start, end));
        }
    }
    None
}

#[expect(
    clippy::string_slice,
    reason = "candidate boundaries are accepted only around the three ASCII AND bytes"
)]
fn standalone_and(input: &str) -> Option<(usize, usize)> {
    let bytes = input.as_bytes();
    let mut start = 0;
    while start + 3 <= input.len() {
        let end = start + 3;
        if input[start..end].eq_ignore_ascii_case("AND")
            && (start == 0 || bytes[start - 1].is_ascii_whitespace())
            && bytes.get(end).is_none_or(u8::is_ascii_whitespace)
        {
            return Some((start, end));
        }
        start += 1;
    }
    None
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

fn next_space(input: &str, mut cursor: usize) -> usize {
    while input
        .as_bytes()
        .get(cursor)
        .is_some_and(|byte| !byte.is_ascii_whitespace())
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

const STATEMENT_SEARCH_FIELDS: &[SearchField] = &[
    search_string("text", &["q"], &["query", "datname", "usename"]),
    search_id("query_id", &[], &["queryid"], true),
    search_string("database", &["db"], &["datname"]),
    search_string("role", &["user"], &["usename"]),
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
