//! Fixed relation views for `PostgreSQL` table and index snapshots.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};

use kronika_reader::{Cell, Dictionary, Row};
use serde_json::{Map, Value, json};

use super::{
    ApiError, CounterReadings, Order, OrderedNumber, PageContext, Plan, PreparedSnapshot,
    RelationGroup, SectionPlans, SnapshotCursor, add_ordered, compare_page_order_values,
    counter_delta, identity_of, record, resolved_dictionary, search_matches, stored_bytes,
};
use crate::route::{Filter, SnapshotRequest};

const TABLES: &str = "pg_stat_user_tables";
const INDEXES: &str = "pg_stat_user_indexes";

const TABLE_RATES: &[&str] = &[
    "seq_scan",
    "seq_tup_read",
    "idx_scan",
    "idx_tup_fetch",
    "n_tup_ins",
    "n_tup_upd",
    "n_tup_del",
    "n_tup_hot_upd",
    "n_tup_newpage_upd",
    "vacuum_count",
    "autovacuum_count",
    "analyze_count",
    "autoanalyze_count",
    "total_vacuum_time",
    "total_autovacuum_time",
    "total_analyze_time",
    "total_autoanalyze_time",
    "heap_blks_read",
    "heap_blks_hit",
    "idx_blks_read",
    "idx_blks_hit",
    "toast_blks_read",
    "toast_blks_hit",
    "tidx_blks_read",
    "tidx_blks_hit",
];

const TABLE_STRUCTURAL_RATES: &[&str] = &[
    "idx_scan",
    "idx_tup_fetch",
    "idx_blks_read",
    "idx_blks_hit",
    "toast_blks_read",
    "toast_blks_hit",
    "tidx_blks_read",
    "tidx_blks_hit",
];

const TABLE_GAUGES: &[&str] = &[
    "n_live_tup",
    "n_dead_tup",
    "n_mod_since_analyze",
    "n_ins_since_vacuum",
    "main_fork_bytes",
    "toast_bytes",
    "toast_n_live_tup",
    "toast_n_dead_tup",
    "reltuples",
];

const TABLE_STRUCTURAL_GAUGES: &[&str] = &["toast_bytes", "toast_n_live_tup", "toast_n_dead_tup"];
const TABLE_MAXIMA: &[&str] = &["xid_age", "mxid_age"];
const TABLE_TIMESTAMPS: &[&str] = &[
    "last_vacuum",
    "last_autovacuum",
    "last_analyze",
    "last_autoanalyze",
    "last_seq_scan",
    "last_idx_scan",
    "toast_last_autovacuum",
];

const INDEX_RATES: &[&str] = &[
    "idx_scan",
    "idx_tup_read",
    "idx_tup_fetch",
    "idx_blks_read",
    "idx_blks_hit",
];
const INDEX_GAUGES: &[&str] = &["main_fork_bytes"];
const INDEX_TIMESTAMPS: &[&str] = &["last_idx_scan"];
const INDEX_FLAGS: &[&str] = &[
    "indisunique",
    "indisprimary",
    "indisvalid",
    "indisexclusion",
    "indisready",
];

const TABLE_OBJECT_FIELDS: &[FieldSpec] = &[
    field("relname", Kind::Text, None),
    field("tablespace", Kind::Text, None),
    field("table_count", Kind::Integer, None),
    rate_field("seq_scan"),
    rate_field("seq_tup_read"),
    rate_field("idx_scan"),
    rate_field("idx_tup_fetch"),
    percent_field("sequential_share_pct"),
    number_field("seq_tuples_per_scan"),
    number_field("idx_tuples_per_scan"),
    rate_field("n_tup_ins"),
    rate_field("n_tup_upd"),
    rate_field("n_tup_del"),
    rate_field("n_tup_hot_upd"),
    rate_field("n_tup_newpage_upd"),
    integer_field("n_live_tup", None),
    integer_field("n_dead_tup", None),
    integer_field("n_mod_since_analyze", None),
    integer_field("n_ins_since_vacuum", None),
    integer_field("reltuples", None),
    percent_field("dead_pct"),
    percent_field("hot_pct"),
    percent_field("new_page_pct"),
    rate_field("vacuum_count"),
    rate_field("autovacuum_count"),
    rate_field("analyze_count"),
    rate_field("autoanalyze_count"),
    number_field_with_unit("vacuum_mean_ms", "milliseconds"),
    number_field_with_unit("autovacuum_mean_ms", "milliseconds"),
    number_field_with_unit("analyze_mean_ms", "milliseconds"),
    number_field_with_unit("autoanalyze_mean_ms", "milliseconds"),
    timestamp_field("last_vacuum"),
    timestamp_field("last_autovacuum"),
    timestamp_field("last_analyze"),
    timestamp_field("last_autoanalyze"),
    timestamp_field("last_seq_scan"),
    timestamp_field("last_idx_scan"),
    timestamp_field("toast_last_autovacuum"),
    integer_field("main_fork_bytes", Some("bytes")),
    integer_field("toast_bytes", Some("bytes")),
    integer_field("toast_n_live_tup", None),
    integer_field("toast_n_dead_tup", None),
    rate_field("heap_blks_read"),
    rate_field("heap_blks_hit"),
    rate_field("idx_blks_read"),
    rate_field("idx_blks_hit"),
    rate_field("toast_blks_read"),
    rate_field("toast_blks_hit"),
    rate_field("tidx_blks_read"),
    rate_field("tidx_blks_hit"),
    percent_field("buffer_hit_pct"),
    integer_field("xid_age", None),
    integer_field("mxid_age", None),
];

const TABLE_AGGREGATE_FIELDS: &[FieldSpec] = &[
    field("table_count", Kind::Integer, None),
    rate_field("seq_scan"),
    rate_field("seq_tup_read"),
    rate_field("idx_scan"),
    rate_field("idx_tup_fetch"),
    percent_field("sequential_share_pct"),
    number_field("seq_tuples_per_scan"),
    number_field("idx_tuples_per_scan"),
    rate_field("n_tup_ins"),
    rate_field("n_tup_upd"),
    rate_field("n_tup_del"),
    rate_field("n_tup_hot_upd"),
    rate_field("n_tup_newpage_upd"),
    integer_field("n_live_tup", None),
    integer_field("n_dead_tup", None),
    integer_field("n_mod_since_analyze", None),
    integer_field("n_ins_since_vacuum", None),
    integer_field("reltuples", None),
    percent_field("dead_pct"),
    percent_field("hot_pct"),
    percent_field("new_page_pct"),
    rate_field("vacuum_count"),
    rate_field("autovacuum_count"),
    rate_field("analyze_count"),
    rate_field("autoanalyze_count"),
    number_field_with_unit("vacuum_mean_ms", "milliseconds"),
    number_field_with_unit("autovacuum_mean_ms", "milliseconds"),
    number_field_with_unit("analyze_mean_ms", "milliseconds"),
    number_field_with_unit("autoanalyze_mean_ms", "milliseconds"),
    timestamp_field("last_vacuum_oldest"),
    timestamp_field("last_vacuum_latest"),
    integer_field("last_vacuum_never_count", None),
    timestamp_field("last_autovacuum_oldest"),
    timestamp_field("last_autovacuum_latest"),
    integer_field("last_autovacuum_never_count", None),
    timestamp_field("last_analyze_oldest"),
    timestamp_field("last_analyze_latest"),
    integer_field("last_analyze_never_count", None),
    timestamp_field("last_autoanalyze_oldest"),
    timestamp_field("last_autoanalyze_latest"),
    integer_field("last_autoanalyze_never_count", None),
    timestamp_field("last_seq_scan_oldest"),
    timestamp_field("last_seq_scan_latest"),
    integer_field("last_seq_scan_never_count", None),
    timestamp_field("last_idx_scan_oldest"),
    timestamp_field("last_idx_scan_latest"),
    integer_field("last_idx_scan_never_count", None),
    timestamp_field("toast_last_autovacuum_oldest"),
    timestamp_field("toast_last_autovacuum_latest"),
    integer_field("toast_last_autovacuum_never_count", None),
    integer_field("main_fork_bytes", Some("bytes")),
    integer_field("toast_bytes", Some("bytes")),
    integer_field("toast_n_live_tup", None),
    integer_field("toast_n_dead_tup", None),
    rate_field("heap_blks_read"),
    rate_field("heap_blks_hit"),
    rate_field("idx_blks_read"),
    rate_field("idx_blks_hit"),
    rate_field("toast_blks_read"),
    rate_field("toast_blks_hit"),
    rate_field("tidx_blks_read"),
    rate_field("tidx_blks_hit"),
    percent_field("buffer_hit_pct"),
    integer_field("xid_age", None),
    integer_field("mxid_age", None),
];

const INDEX_OBJECT_FIELDS: &[FieldSpec] = &[
    field("indexrelname", Kind::Text, None),
    field("relname", Kind::Text, None),
    field("relid", Kind::Id, None),
    field("tablespace", Kind::Text, None),
    field("amname", Kind::Text, None),
    field("index_count", Kind::Integer, None),
    rate_field("idx_scan"),
    rate_field("idx_tup_read"),
    rate_field("idx_tup_fetch"),
    number_field("tuples_per_scan"),
    number_field("fetches_per_scan"),
    integer_field("main_fork_bytes", Some("bytes")),
    rate_field("idx_blks_read"),
    rate_field("idx_blks_hit"),
    percent_field("buffer_hit_pct"),
    timestamp_field("last_idx_scan"),
    field("no_scans", Kind::Boolean, None),
    field("indisunique", Kind::Boolean, None),
    field("indisprimary", Kind::Boolean, None),
    field("indisvalid", Kind::Boolean, None),
    field("indisexclusion", Kind::Boolean, None),
    field("indisready", Kind::Boolean, None),
    integer_field("state_severity", None),
];

const INDEX_AGGREGATE_FIELDS: &[FieldSpec] = &[
    field("index_count", Kind::Integer, None),
    rate_field("idx_scan"),
    rate_field("idx_tup_read"),
    rate_field("idx_tup_fetch"),
    number_field("tuples_per_scan"),
    number_field("fetches_per_scan"),
    integer_field("main_fork_bytes", Some("bytes")),
    rate_field("idx_blks_read"),
    rate_field("idx_blks_hit"),
    percent_field("buffer_hit_pct"),
    timestamp_field("last_idx_scan_oldest"),
    timestamp_field("last_idx_scan_latest"),
    integer_field("last_idx_scan_never_count", None),
    integer_field("no_scan_count", None),
    integer_field("known_scan_count", None),
    integer_field("invalid_count", None),
    integer_field("not_ready_count", None),
    integer_field("unique_count", None),
    integer_field("primary_count", None),
    integer_field("exclusion_count", None),
    integer_field("state_severity", None),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum RelationKind {
    Tables,
    Indexes,
}

impl RelationKind {
    fn from_name(name: &str) -> Result<Self, ApiError> {
        match name {
            TABLES => Ok(Self::Tables),
            INDEXES => Ok(Self::Indexes),
            _ => Err(ApiError::BadFilter("group".to_owned())),
        }
    }

    const fn count_field(self) -> &'static str {
        match self {
            Self::Tables => "table_count",
            Self::Indexes => "index_count",
        }
    }

    const fn fields(self, group: RelationGroup) -> &'static [FieldSpec] {
        match (self, group) {
            (Self::Tables, RelationGroup::Object) => TABLE_OBJECT_FIELDS,
            (Self::Tables, RelationGroup::Database | RelationGroup::Schema) => {
                TABLE_AGGREGATE_FIELDS
            }
            (Self::Indexes, RelationGroup::Object) => INDEX_OBJECT_FIELDS,
            (Self::Indexes, RelationGroup::Database | RelationGroup::Schema) => {
                INDEX_AGGREGATE_FIELDS
            }
        }
    }
}

#[derive(Clone, Copy)]
struct FieldSpec {
    name: &'static str,
    kind: Kind,
    unit: Option<&'static str>,
}

#[derive(Clone, Copy)]
enum Kind {
    Number,
    Id,
    Integer,
    Timestamp,
    Boolean,
    Text,
}

const fn field(name: &'static str, kind: Kind, unit: Option<&'static str>) -> FieldSpec {
    FieldSpec { name, kind, unit }
}

const fn rate_field(name: &'static str) -> FieldSpec {
    field(name, Kind::Number, Some("per_second"))
}

const fn percent_field(name: &'static str) -> FieldSpec {
    field(name, Kind::Number, Some("percent"))
}

const fn number_field(name: &'static str) -> FieldSpec {
    field(name, Kind::Number, None)
}

const fn number_field_with_unit(name: &'static str, unit: &'static str) -> FieldSpec {
    field(name, Kind::Number, Some(unit))
}

const fn integer_field(name: &'static str, unit: Option<&'static str>) -> FieldSpec {
    field(name, Kind::Integer, unit)
}

const fn timestamp_field(name: &'static str) -> FieldSpec {
    field(name, Kind::Timestamp, None)
}

pub(super) fn output_fields(
    sections: &[String],
    group: RelationGroup,
    requested: &[String],
) -> Result<Vec<String>, ApiError> {
    let [logical_name] = sections else {
        return Err(ApiError::BadFilter("group".to_owned()));
    };
    let kind = RelationKind::from_name(logical_name)?;
    let available = kind.fields(group);
    if requested.is_empty() {
        return Ok(available
            .iter()
            .map(|field| field.name.to_owned())
            .collect());
    }
    let keys = key_fields(kind, group);
    for name in requested {
        if !available.iter().any(|field| field.name == name) && !keys.contains(&name.as_str()) {
            return Err(ApiError::NoSuchColumn(name.clone()));
        }
    }
    let mut output = Vec::with_capacity(requested.len());
    for name in requested {
        if !keys.contains(&name.as_str()) && !output.contains(name) {
            output.push(name.clone());
        }
    }
    Ok(output)
}

pub(super) fn split_filters(
    request: &SnapshotRequest,
) -> Result<(Vec<Filter>, Vec<Filter>), ApiError> {
    if request.group.is_none() {
        return Ok((request.filters.clone(), Vec::new()));
    }
    let [section] = request.sections.as_slice() else {
        return Err(ApiError::BadFilter("where".to_owned()));
    };
    let mut physical = Vec::new();
    let mut derived = Vec::new();
    for filter in &request.filters {
        if filter.column == "no_scans" {
            if section != INDEXES || filter.value != "true" {
                return Err(ApiError::BadFilter(filter.column.clone()));
            }
            derived.push(filter.clone());
        } else {
            physical.push(filter.clone());
        }
    }
    Ok((physical, derived))
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct GroupKey {
    datid: u32,
    datname: String,
    schemaname: Option<String>,
    relid: Option<u32>,
    relname: Option<String>,
    indexrelid: Option<u32>,
    indexrelname: Option<String>,
}

impl GroupKey {
    fn from_row(
        kind: RelationKind,
        group: RelationGroup,
        row: &Row,
        dictionary: &Dictionary,
    ) -> Result<Option<Self>, ApiError> {
        let (Some(datid), Some(datname)) = (
            unsigned_cell(row.get("datid")),
            text_cell(row.get("datname"), dictionary)?,
        ) else {
            return Ok(None);
        };
        let schemaname = if group == RelationGroup::Database {
            None
        } else {
            text_cell(row.get("schemaname"), dictionary)?
        };
        if group != RelationGroup::Database && schemaname.is_none() {
            return Ok(None);
        }
        let object = group == RelationGroup::Object;
        let relid = object.then(|| unsigned_cell(row.get("relid"))).flatten();
        let relname = if object {
            text_cell(row.get("relname"), dictionary)?
        } else {
            None
        };
        if object && (relid.is_none() || relname.is_none()) {
            return Ok(None);
        }
        let (indexrelid, indexrelname) = if kind == RelationKind::Indexes && object {
            (
                unsigned_cell(row.get("indexrelid")),
                text_cell(row.get("indexrelname"), dictionary)?,
            )
        } else {
            (None, None)
        };
        if kind == RelationKind::Indexes
            && object
            && (indexrelid.is_none() || indexrelname.is_none())
        {
            return Ok(None);
        }
        Ok(Some(Self {
            datid,
            datname,
            schemaname,
            relid,
            relname,
            indexrelid,
            indexrelname,
        }))
    }

    fn json(&self, kind: RelationKind, group: RelationGroup) -> Value {
        let mut key = Map::new();
        key.insert("datid".to_owned(), Value::String(self.datid.to_string()));
        key.insert("datname".to_owned(), Value::String(self.datname.clone()));
        if group != RelationGroup::Database
            && let Some(value) = &self.schemaname
        {
            key.insert("schemaname".to_owned(), Value::String(value.clone()));
        }
        if group == RelationGroup::Object {
            if let Some(value) = self.relid {
                key.insert("relid".to_owned(), Value::String(value.to_string()));
            }
            if let Some(value) = &self.relname {
                key.insert("relname".to_owned(), Value::String(value.clone()));
            }
            if kind == RelationKind::Indexes {
                if let Some(value) = self.indexrelid {
                    key.insert("indexrelid".to_owned(), Value::String(value.to_string()));
                }
                if let Some(value) = &self.indexrelname {
                    key.insert("indexrelname".to_owned(), Value::String(value.clone()));
                }
            }
        }
        Value::Object(key)
    }

    fn for_group(mut self, kind: RelationKind, group: RelationGroup) -> Self {
        match group {
            RelationGroup::Database => {
                self.schemaname = None;
                self.relid = None;
                self.relname = None;
                self.indexrelid = None;
                self.indexrelname = None;
            }
            RelationGroup::Schema => {
                self.relid = None;
                self.relname = None;
                self.indexrelid = None;
                self.indexrelname = None;
            }
            RelationGroup::Object if kind == RelationKind::Tables => {
                self.indexrelid = None;
                self.indexrelname = None;
            }
            RelationGroup::Object => {}
        }
        self
    }

    fn metric(&self, name: &str) -> Option<Metric> {
        match name {
            "datid" => Some(Metric::Integer(i128::from(self.datid))),
            "datname" => Some(Metric::Text(self.datname.clone())),
            "schemaname" => self.schemaname.clone().map(Metric::Text),
            "relid" => self.relid.map(|value| Metric::Integer(i128::from(value))),
            "relname" => self.relname.clone().map(Metric::Text),
            "indexrelid" => self
                .indexrelid
                .map(|value| Metric::Integer(i128::from(value))),
            "indexrelname" => self.indexrelname.clone().map(Metric::Text),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Source {
    context_index: usize,
    ordinal: u64,
    type_id: u32,
    timestamp: i64,
}

#[derive(Clone, Copy, Default)]
enum Availability {
    #[default]
    Empty,
    Value(OrderedNumber),
    Unavailable,
}

impl Availability {
    fn add(&mut self, input: Input) {
        match (*self, input) {
            (Self::Unavailable, _) | (_, Input::Unavailable) => *self = Self::Unavailable,
            (Self::Empty | Self::Value(_), Input::Neutral) => {}
            (Self::Empty, Input::Value(value)) => *self = Self::Value(value),
            (Self::Value(left), Input::Value(right)) => {
                *self = add_ordered(Some(left), right).map_or(Self::Unavailable, Self::Value);
            }
        }
    }

    const fn value(self) -> Option<OrderedNumber> {
        match self {
            Self::Value(value) => Some(value),
            Self::Empty | Self::Unavailable => None,
        }
    }
}

#[derive(Clone, Copy)]
enum Input {
    Value(OrderedNumber),
    Neutral,
    Unavailable,
}

#[expect(
    variant_size_differences,
    reason = "exact rational rates avoid heap allocation and preserve ordering"
)]
#[derive(Clone, Copy, Debug)]
enum RateValue {
    /// A non-negative rate represented exactly as units per microsecond.
    Exact { numerator: u128, denominator: u128 },
    /// Units per microsecond when the input counter is floating point or exact
    /// rational accumulation exceeds `u128`.
    Float(f64),
}

impl RateValue {
    #[expect(
        clippy::cast_precision_loss,
        reason = "an interval of 2^52 microseconds is 142 years"
    )]
    fn from_delta(delta: OrderedNumber, elapsed: i64) -> Option<Self> {
        let denominator = u128::try_from(elapsed).ok().filter(|value| *value > 0)?;
        match delta {
            OrderedNumber::Integer(value) => {
                let numerator = u128::try_from(value).ok()?;
                Some(Self::exact(numerator, denominator))
            }
            OrderedNumber::Float(value) => {
                let rate = value / elapsed as f64;
                rate.is_finite().then_some(Self::Float(rate))
            }
        }
    }

    const fn exact(numerator: u128, denominator: u128) -> Self {
        let divisor = greatest_common_divisor(numerator, denominator);
        Self::Exact {
            numerator: numerator / divisor,
            denominator: denominator / divisor,
        }
    }

    fn add(self, other: Self) -> Option<Self> {
        match (self, other) {
            (
                Self::Exact {
                    numerator: left_numerator,
                    denominator: left_denominator,
                },
                Self::Exact {
                    numerator: right_numerator,
                    denominator: right_denominator,
                },
            ) => {
                let common = greatest_common_divisor(left_denominator, right_denominator);
                let left_scale = right_denominator / common;
                let right_scale = left_denominator / common;
                let exact = left_numerator
                    .checked_mul(left_scale)
                    .and_then(|left| {
                        right_numerator
                            .checked_mul(right_scale)
                            .and_then(|right| left.checked_add(right))
                    })
                    .zip(left_denominator.checked_mul(left_scale))
                    .map(|(numerator, denominator)| Self::exact(numerator, denominator));
                exact.or_else(|| Self::float_sum(self, other))
            }
            _ => Self::float_sum(self, other),
        }
    }

    fn float_sum(self, other: Self) -> Option<Self> {
        let value = self.per_microsecond() + other.per_microsecond();
        value.is_finite().then_some(Self::Float(value))
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "floating output is produced only after exact rational accumulation"
    )]
    fn per_microsecond(self) -> f64 {
        match self {
            Self::Exact {
                numerator,
                denominator,
            } => numerator as f64 / denominator as f64,
            Self::Float(value) => value,
        }
    }

    fn per_second(self) -> f64 {
        self.per_microsecond() * 1_000_000.0
    }

    fn order_value(self) -> Option<super::PageOrderValue> {
        match self {
            Self::Exact {
                numerator,
                denominator,
            } => Some(super::PageOrderValue::IntegerRatio {
                numerator,
                denominator,
            }),
            Self::Float(value) => value
                .is_finite()
                .then_some(super::PageOrderValue::FloatRatio(value)),
        }
    }

    fn ratio_order_value(self, other: Self) -> Option<super::PageOrderValue> {
        if let (
            Self::Exact {
                numerator,
                denominator,
            },
            Self::Exact {
                numerator: other_numerator,
                denominator: other_denominator,
            },
        ) = (self, other)
        {
            if other_numerator == 0 {
                return None;
            }
            if let Some((numerator, denominator)) = numerator
                .checked_mul(other_denominator)
                .zip(denominator.checked_mul(other_numerator))
            {
                return Self::exact(numerator, denominator).order_value();
            }
        }
        let denominator = other.per_microsecond();
        let value = self.per_microsecond() / denominator;
        (denominator > 0.0 && value.is_finite()).then_some(super::PageOrderValue::FloatRatio(value))
    }
}

const fn greatest_common_divisor(mut left: u128, mut right: u128) -> u128 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    if left == 0 { 1 } else { left }
}

#[derive(Clone, Copy, Default)]
struct RateAggregate {
    value: Option<RateValue>,
    unavailable: bool,
}

impl RateAggregate {
    fn add(&mut self, input: Input, elapsed: Option<i64>) {
        if self.unavailable {
            return;
        }
        match input {
            Input::Neutral => {}
            Input::Unavailable => self.unavailable = true,
            Input::Value(delta) => {
                let Some(value) = elapsed.and_then(|elapsed| RateValue::from_delta(delta, elapsed))
                else {
                    self.unavailable = true;
                    return;
                };
                self.value = self.value.map_or(Some(value), |known| known.add(value));
                if self.value.is_none() {
                    self.unavailable = true;
                }
            }
        }
    }

    fn metric(self) -> Option<Metric> {
        if self.unavailable {
            None
        } else {
            self.value.map(Metric::Rate)
        }
    }

    const fn value(self) -> Option<RateValue> {
        if self.unavailable { None } else { self.value }
    }
}

#[derive(Clone, Copy, Default)]
struct MaximumAggregate {
    maximum: Option<i128>,
    unavailable: bool,
}

impl MaximumAggregate {
    fn add(&mut self, present: bool, stored: Option<&Cell>) {
        if !present {
            self.unavailable = true;
            return;
        }
        match stored {
            Some(Cell::Null) => {}
            Some(cell) => match integer_cell(Some(cell)) {
                Some(value) => {
                    self.maximum = Some(self.maximum.map_or(value, |known| known.max(value)));
                }
                None => self.unavailable = true,
            },
            None => self.unavailable = true,
        }
    }

    const fn exact(self) -> Option<i128> {
        if self.unavailable { None } else { self.maximum }
    }
}

#[derive(Clone, Copy, Default)]
struct TimestampAggregate {
    applicable: u64,
    unavailable: bool,
    oldest: Option<i64>,
    latest: Option<i64>,
    never: u64,
}

impl TimestampAggregate {
    fn add(&mut self, present: bool, stored: Option<&Cell>, structural_na: bool) {
        if !present {
            self.unavailable = true;
            return;
        }
        if structural_na {
            return;
        }
        self.applicable = self.applicable.saturating_add(1);
        match stored {
            Some(Cell::Ts(value)) => {
                self.oldest = Some(self.oldest.map_or(*value, |oldest| oldest.min(*value)));
                self.latest = Some(self.latest.map_or(*value, |latest| latest.max(*value)));
            }
            Some(Cell::Null) => self.never = self.never.saturating_add(1),
            _ => self.unavailable = true,
        }
    }

    const fn exact(self) -> bool {
        self.applicable > 0 && !self.unavailable
    }
}

#[derive(Clone, Copy, Default)]
struct BoolAggregate {
    known: u64,
    truthy: u64,
    unavailable: bool,
}

impl BoolAggregate {
    fn add(&mut self, stored: Option<&Cell>) {
        match stored {
            Some(Cell::Bool(value)) => {
                self.known = self.known.saturating_add(1);
                self.truthy = self.truthy.saturating_add(u64::from(*value));
            }
            _ => self.unavailable = true,
        }
    }

    fn add_scan(&mut self, input: Input) {
        match input {
            Input::Value(value) => self.add(Some(&Cell::Bool(number_is_zero(value)))),
            Input::Neutral | Input::Unavailable => self.unavailable = true,
        }
    }

    const fn exact(self) -> bool {
        self.known > 0 && !self.unavailable
    }
}

struct Aggregate {
    key: GroupKey,
    count: u64,
    source: Source,
    from: Option<i64>,
    to: Option<i64>,
    rates: BTreeMap<&'static str, RateAggregate>,
    gauges: BTreeMap<&'static str, Availability>,
    maxima: BTreeMap<&'static str, MaximumAggregate>,
    timestamps: BTreeMap<&'static str, TimestampAggregate>,
    flags: BTreeMap<&'static str, BoolAggregate>,
    texts: BTreeMap<&'static str, String>,
    identifiers: BTreeMap<&'static str, i128>,
    no_scans: BoolAggregate,
    state_severity: Option<i128>,
}

impl Aggregate {
    fn new(key: GroupKey, source: Source) -> Self {
        Self {
            key,
            count: 0,
            source,
            from: None,
            to: None,
            rates: BTreeMap::new(),
            gauges: BTreeMap::new(),
            maxima: BTreeMap::new(),
            timestamps: BTreeMap::new(),
            flags: BTreeMap::new(),
            texts: BTreeMap::new(),
            identifiers: BTreeMap::new(),
            no_scans: BoolAggregate::default(),
            state_severity: None,
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "one physical object carries its validated interval and source coordinates"
    )]
    fn add(
        &mut self,
        kind: RelationKind,
        plan: &Plan,
        row: &Row,
        before: Option<&CounterReadings>,
        elapsed: Option<i64>,
        dictionary: &Dictionary,
        source: Source,
    ) -> Result<(), ApiError> {
        self.count = self.count.saturating_add(1);
        self.source = self.source.min(source);
        self.to = Some(
            self.to
                .map_or(source.timestamp, |to| to.max(source.timestamp)),
        );
        if let Some(from) = elapsed.and_then(|elapsed| source.timestamp.checked_sub(elapsed)) {
            self.from = Some(self.from.map_or(from, |oldest| oldest.min(from)));
        }
        match kind {
            RelationKind::Tables => self.add_table(plan, row, before, elapsed, dictionary)?,
            RelationKind::Indexes => self.add_index(plan, row, before, elapsed, dictionary)?,
        }
        Ok(())
    }

    fn add_table(
        &mut self,
        plan: &Plan,
        row: &Row,
        before: Option<&CounterReadings>,
        elapsed: Option<i64>,
        dictionary: &Dictionary,
    ) -> Result<(), ApiError> {
        for &name in TABLE_RATES {
            let structural = TABLE_STRUCTURAL_RATES.contains(&name);
            self.rates
                .entry(name)
                .or_default()
                .add(counter_input(plan, row, before, name, structural), elapsed);
        }
        for &name in TABLE_GAUGES {
            let structural = TABLE_STRUCTURAL_GAUGES.contains(&name);
            let mut input = gauge_input(plan, row, name, structural);
            if name == "reltuples" && matches!(input, Input::Value(OrderedNumber::Integer(-1))) {
                input = Input::Unavailable;
            }
            self.gauges.entry(name).or_default().add(input);
        }
        for &name in TABLE_MAXIMA {
            self.maxima
                .entry(name)
                .or_default()
                .add(plan.contract.column(name).is_some(), row.get(name));
        }
        for &name in TABLE_TIMESTAMPS {
            let structural_na = name == "toast_last_autovacuum"
                && matches!(row.get("toast_bytes"), Some(Cell::Null));
            self.timestamps.entry(name).or_default().add(
                plan.contract.column(name).is_some(),
                row.get(name),
                structural_na,
            );
        }
        for name in ["relname", "tablespace"] {
            if let Some(value) = text_cell(row.get(name), dictionary)? {
                self.texts.entry(name).or_insert(value);
            }
        }
        Ok(())
    }

    fn add_index(
        &mut self,
        plan: &Plan,
        row: &Row,
        before: Option<&CounterReadings>,
        elapsed: Option<i64>,
        dictionary: &Dictionary,
    ) -> Result<(), ApiError> {
        let scan_input = counter_input(plan, row, before, "idx_scan", false);
        for &name in INDEX_RATES {
            let input = if name == "idx_scan" {
                scan_input
            } else {
                counter_input(plan, row, before, name, false)
            };
            self.rates.entry(name).or_default().add(input, elapsed);
        }
        for &name in INDEX_GAUGES {
            self.gauges
                .entry(name)
                .or_default()
                .add(gauge_input(plan, row, name, false));
        }
        for &name in INDEX_TIMESTAMPS {
            self.timestamps.entry(name).or_default().add(
                plan.contract.column(name).is_some(),
                row.get(name),
                false,
            );
        }
        for &name in INDEX_FLAGS {
            self.flags.entry(name).or_default().add(row.get(name));
        }
        self.no_scans.add_scan(scan_input);
        let valid = bool_cell(row.get("indisvalid"));
        let ready = bool_cell(row.get("indisready"));
        let severity = match (valid, ready) {
            (Some(false), _) => Some(2),
            (Some(true), Some(false)) => Some(1),
            (Some(true), Some(true)) => Some(0),
            _ => None,
        };
        self.state_severity = match (self.state_severity, severity) {
            (Some(left), Some(right)) => Some(left.max(right)),
            (None, Some(value)) if self.count == 1 => Some(value),
            _ => None,
        };
        for name in ["indexrelname", "relname", "tablespace", "amname"] {
            if let Some(value) = text_cell(row.get(name), dictionary)? {
                self.texts.entry(name).or_insert(value);
            }
        }
        if let Some(value) = integer_cell(row.get("relid")) {
            self.identifiers.entry("relid").or_insert(value);
        }
        Ok(())
    }

    fn matches_derived_filters(&self, kind: RelationKind, filters: &[Filter]) -> bool {
        filters
            .iter()
            .all(|filter| match (kind, filter.column.as_str()) {
                (RelationKind::Indexes, "no_scans") => self
                    .rates
                    .get("idx_scan")
                    .and_then(|rate| rate.metric())
                    .is_some_and(|metric| metric_is_zero(&metric)),
                _ => false,
            })
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the fixed relation contract keeps every audited reducer in one exhaustive match"
    )]
    fn metric(&self, kind: RelationKind, group: RelationGroup, name: &str) -> Option<Metric> {
        if name == kind.count_field() {
            return Some(Metric::Integer(i128::from(self.count)));
        }
        if let Some(rate) = self.rates.get(name).copied() {
            return rate.metric();
        }
        if let Some(gauge) = self.gauges.get(name).copied() {
            return gauge.value().map(Metric::Number);
        }
        if let Some(maximum) = self
            .maxima
            .get(name)
            .copied()
            .and_then(MaximumAggregate::exact)
        {
            return Some(Metric::Integer(maximum));
        }
        if let Some(value) = self.texts.get(name) {
            return (group == RelationGroup::Object).then(|| Metric::Text(value.clone()));
        }
        if let Some(value) = self.identifiers.get(name) {
            return (group == RelationGroup::Object).then_some(Metric::Integer(*value));
        }
        match (kind, name) {
            (RelationKind::Tables, "sequential_share_pct") => {
                self.ratio(&["seq_scan"], &["seq_scan", "idx_scan"], 100.0)
            }
            (RelationKind::Tables, "seq_tuples_per_scan") => {
                self.ratio(&["seq_tup_read"], &["seq_scan"], 1.0)
            }
            (RelationKind::Tables, "idx_tuples_per_scan")
            | (RelationKind::Indexes, "fetches_per_scan") => {
                self.ratio(&["idx_tup_fetch"], &["idx_scan"], 1.0)
            }
            (RelationKind::Tables, "dead_pct") => {
                self.gauge_ratio(&["n_dead_tup"], &["n_live_tup", "n_dead_tup"], 100.0)
            }
            (RelationKind::Tables, "hot_pct") => {
                self.ratio(&["n_tup_hot_upd"], &["n_tup_upd"], 100.0)
            }
            (RelationKind::Tables, "new_page_pct") => {
                self.ratio(&["n_tup_newpage_upd"], &["n_tup_upd"], 100.0)
            }
            (RelationKind::Tables, "buffer_hit_pct") => self.ratio(
                &[
                    "heap_blks_hit",
                    "idx_blks_hit",
                    "toast_blks_hit",
                    "tidx_blks_hit",
                ],
                &[
                    "heap_blks_read",
                    "heap_blks_hit",
                    "idx_blks_read",
                    "idx_blks_hit",
                    "toast_blks_read",
                    "toast_blks_hit",
                    "tidx_blks_read",
                    "tidx_blks_hit",
                ],
                100.0,
            ),
            (RelationKind::Tables, "vacuum_mean_ms") => {
                self.ratio(&["total_vacuum_time"], &["vacuum_count"], 1.0)
            }
            (RelationKind::Tables, "autovacuum_mean_ms") => {
                self.ratio(&["total_autovacuum_time"], &["autovacuum_count"], 1.0)
            }
            (RelationKind::Tables, "analyze_mean_ms") => {
                self.ratio(&["total_analyze_time"], &["analyze_count"], 1.0)
            }
            (RelationKind::Tables, "autoanalyze_mean_ms") => {
                self.ratio(&["total_autoanalyze_time"], &["autoanalyze_count"], 1.0)
            }
            (RelationKind::Indexes, "tuples_per_scan") => {
                self.ratio(&["idx_tup_read"], &["idx_scan"], 1.0)
            }
            (RelationKind::Indexes, "buffer_hit_pct") => {
                self.ratio(&["idx_blks_hit"], &["idx_blks_read", "idx_blks_hit"], 100.0)
            }
            (RelationKind::Indexes, "no_scans") if group == RelationGroup::Object => self
                .no_scans
                .exact()
                .then_some(Metric::Boolean(self.no_scans.truthy == 1)),
            (RelationKind::Indexes, "no_scan_count") => self
                .no_scans
                .exact()
                .then(|| Metric::Integer(i128::from(self.no_scans.truthy))),
            (RelationKind::Indexes, "known_scan_count") => self
                .no_scans
                .exact()
                .then(|| Metric::Integer(i128::from(self.no_scans.known))),
            (RelationKind::Indexes, "state_severity") => self.state_severity.map(Metric::Integer),
            (RelationKind::Indexes, "invalid_count") => self.flag_count("indisvalid", false),
            (RelationKind::Indexes, "not_ready_count") => self.flag_count("indisready", false),
            (RelationKind::Indexes, "unique_count") => self.flag_count("indisunique", true),
            (RelationKind::Indexes, "primary_count") => self.flag_count("indisprimary", true),
            (RelationKind::Indexes, "exclusion_count") => self.flag_count("indisexclusion", true),
            (RelationKind::Indexes, name) if INDEX_FLAGS.contains(&name) => self
                .flags
                .get(name)
                .copied()
                .filter(|flag| flag.exact() && flag.known == 1)
                .map(|flag| Metric::Boolean(flag.truthy == 1)),
            _ => timestamp_metric(self, group, name),
        }
    }

    fn ratio(&self, numerator: &[&str], denominator: &[&str], scale: f64) -> Option<Metric> {
        Some(Metric::RateRatio {
            numerator: sum_rates(&self.rates, numerator)?,
            denominator: sum_rates(&self.rates, denominator)?,
            scale,
        })
    }

    fn gauge_ratio(&self, numerator: &[&str], denominator: &[&str], scale: f64) -> Option<Metric> {
        Some(Metric::Ratio {
            numerator: sum_values(&self.gauges, numerator)?,
            denominator: sum_values(&self.gauges, denominator)?,
            scale,
        })
    }

    fn flag_count(&self, name: &str, truthy: bool) -> Option<Metric> {
        let flag = self.flags.get(name).copied()?;
        flag.exact().then(|| {
            let count = if truthy {
                flag.truthy
            } else {
                flag.known - flag.truthy
            };
            Metric::Integer(i128::from(count))
        })
    }
}

fn metric_is_zero(metric: &Metric) -> bool {
    match metric {
        Metric::Rate(RateValue::Exact { numerator, .. }) => *numerator == 0,
        Metric::Rate(RateValue::Float(value)) => *value == 0.0,
        _ => false,
    }
}

fn timestamp_metric(aggregate: &Aggregate, group: RelationGroup, name: &str) -> Option<Metric> {
    if group == RelationGroup::Object {
        let timestamp = aggregate.timestamps.get(name).copied()?;
        return (timestamp.exact() && timestamp.never == 0)
            .then(|| timestamp.latest.map(Metric::Timestamp))
            .flatten();
    }
    for suffix in ["_oldest", "_latest", "_never_count"] {
        let Some(base) = name.strip_suffix(suffix) else {
            continue;
        };
        let timestamp = aggregate.timestamps.get(base).copied()?;
        if !timestamp.exact() {
            return None;
        }
        return match suffix {
            "_oldest" => timestamp.oldest.map(Metric::Timestamp),
            "_latest" => timestamp.latest.map(Metric::Timestamp),
            "_never_count" => Some(Metric::Integer(i128::from(timestamp.never))),
            _ => None,
        };
    }
    None
}

fn sum_rates(rates: &BTreeMap<&'static str, RateAggregate>, names: &[&str]) -> Option<RateValue> {
    let mut sum: Option<RateValue> = None;
    for name in names {
        let value = rates.get(name)?.value()?;
        sum = Some(match sum {
            Some(known) => known.add(value)?,
            None => value,
        });
    }
    sum
}

fn sum_values(
    values: &BTreeMap<&'static str, Availability>,
    names: &[&str],
) -> Option<OrderedNumber> {
    names.iter().try_fold(None, |sum, name| {
        add_ordered(sum, values.get(name)?.value()?).map(Some)
    })?
}

#[derive(Clone)]
enum Metric {
    Number(OrderedNumber),
    Rate(RateValue),
    RateRatio {
        numerator: RateValue,
        denominator: RateValue,
        scale: f64,
    },
    Ratio {
        numerator: OrderedNumber,
        denominator: OrderedNumber,
        scale: f64,
    },
    Integer(i128),
    Timestamp(i64),
    Boolean(bool),
    Text(String),
}

impl Metric {
    fn json(&self) -> Value {
        match self {
            Self::Number(OrderedNumber::Integer(value)) | Self::Integer(value) => {
                Value::String(value.to_string())
            }
            Self::Number(OrderedNumber::Float(value)) => finite_json(*value),
            Self::Rate(value) => finite_json(value.per_second()),
            Self::RateRatio {
                numerator,
                denominator,
                scale,
            } => finite_json(numerator.per_microsecond() / denominator.per_microsecond() * scale),
            Self::Ratio {
                numerator,
                denominator,
                scale,
            } => finite_json(numerator.as_f64() / denominator.as_f64() * scale),
            Self::Timestamp(value) => Value::String(value.to_string()),
            Self::Boolean(value) => Value::Bool(*value),
            Self::Text(value) => Value::String(value.clone()),
        }
    }

    fn order_value(&self) -> Option<super::PageOrderValue> {
        match self {
            Self::Number(OrderedNumber::Integer(value)) | Self::Integer(value) => {
                Some(super::PageOrderValue::Integer(*value))
            }
            Self::Number(OrderedNumber::Float(value)) => value
                .is_finite()
                .then_some(super::PageOrderValue::Float(*value)),
            Self::Rate(value) => value.order_value(),
            Self::RateRatio {
                numerator,
                denominator,
                ..
            } => numerator.ratio_order_value(*denominator),
            Self::Ratio {
                numerator: OrderedNumber::Integer(numerator),
                denominator: OrderedNumber::Integer(denominator),
                ..
            } if *numerator >= 0 && *denominator > 0 => Some(super::PageOrderValue::IntegerRatio {
                numerator: u128::try_from(*numerator).ok()?,
                denominator: u128::try_from(*denominator).ok()?,
            }),
            Self::Ratio {
                numerator,
                denominator,
                ..
            } => {
                let ratio = numerator.as_f64() / denominator.as_f64();
                (denominator.as_f64() > 0.0 && ratio.is_finite())
                    .then_some(super::PageOrderValue::FloatRatio(ratio))
            }
            Self::Timestamp(value) => Some(super::PageOrderValue::Integer(i128::from(*value))),
            Self::Boolean(value) => Some(super::PageOrderValue::Integer(i128::from(*value))),
            Self::Text(value) => Some(super::PageOrderValue::Text(value.as_bytes().to_vec())),
        }
    }
}

fn finite_json(value: f64) -> Value {
    if value.is_finite() {
        json!(value)
    } else {
        Value::Null
    }
}

struct RelationRow {
    key: GroupKey,
    metrics: BTreeMap<String, Option<Metric>>,
    sort: Option<Metric>,
    source: Source,
    from: Option<i64>,
    to: Option<i64>,
}

impl PreparedSnapshot {
    #[expect(
        clippy::too_many_lines,
        reason = "one streaming routine preserves the exact aggregate-sort-page-cursor pipeline"
    )]
    pub(super) fn emit_relation_page(
        &self,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), ApiError> {
        let [section] = self.sections.as_slice() else {
            return Err(ApiError::BadCursor);
        };
        let group = self.group.ok_or(ApiError::BadCursor)?;
        let kind = RelationKind::from_name(&section.logical_name)?;
        let fields = kind.fields(group);
        let keys = key_fields(kind, group);
        for name in &self.by {
            let semantic = sort_name(name);
            if !fields.iter().any(|field| field.name == semantic) && !keys.contains(&semantic) {
                return Err(ApiError::NoSuchColumn(name.clone()));
            }
        }
        if cancelled()
            || !emit(relation_layout(
                section,
                kind,
                group,
                &self.relation_fields,
            )?)
        {
            return Ok(());
        }
        let contexts = self.partitioned_contexts(section, cancelled)?;
        if cancelled() {
            return Ok(());
        }
        let mut aggregates = BTreeMap::<GroupKey, Aggregate>::new();
        for context in &contexts {
            scan_context(self, kind, group, context, &mut aggregates, cancelled)?;
            if cancelled() {
                return Ok(());
            }
        }
        let eligible = u64::try_from(aggregates.len()).unwrap_or(u64::MAX);
        let order_by = self.by.first().map(|name| sort_name(name));
        let mut rows = aggregates
            .into_values()
            .map(|aggregate| {
                let metrics = self
                    .relation_fields
                    .iter()
                    .map(|name| (name.clone(), aggregate.metric(kind, group, name)))
                    .collect();
                let sort = order_by.and_then(|name| {
                    aggregate
                        .metric(kind, group, name)
                        .or_else(|| aggregate.key.metric(name))
                });
                RelationRow {
                    key: aggregate.key,
                    metrics,
                    sort,
                    source: aggregate.source,
                    from: aggregate.from,
                    to: aggregate.to,
                }
            })
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| compare_rows(left, right, self.direction));
        let start = match self.cursor {
            Some(cursor) => rows
                .iter()
                .position(|row| {
                    row.source.context_index == cursor.context_index
                        && row.source.ordinal == cursor.ordinal
                })
                .ok_or(ApiError::BadCursor)?,
            None => 0,
        };
        let page_size = self.page_size.ok_or(ApiError::BadCursor)?;
        let end = start.saturating_add(page_size).min(rows.len());
        let has_more = end < rows.len();
        let next_cursor = has_more.then(|| {
            let source = rows[end].source;
            SnapshotCursor {
                segment_id: self.segment.id(),
                active_position: self.segment.active_position().unwrap_or(0),
                context_index: source.context_index,
                ordinal: source.ordinal,
                binding: self.binding,
            }
            .encode()
        });
        let returned = end.saturating_sub(start);
        for row in &rows[start..end] {
            if cancelled()
                || !emit(relation_record(
                    section,
                    kind,
                    group,
                    row,
                    group == RelationGroup::Object,
                )?)
            {
                return Ok(());
            }
        }
        let from = rows.iter().filter_map(|row| row.from).min();
        let to = rows.iter().filter_map(|row| row.to).max();
        let _connected = emit(record(json!({
            "record": "snapshot_page",
            "logical_name": section.logical_name,
            "group": group_name(group),
            "eligible": eligible.to_string(),
            "returned": returned.to_string(),
            "has_more": has_more,
            "truncated": eligible > returned as u64,
            "next_cursor": next_cursor,
            "page_size": page_size,
            "order_by": order_by.into_iter().collect::<Vec<_>>(),
            "order_direction": order_name(self.direction),
            "from": from.map(|value| value.to_string()),
            "to": to.map(|value| value.to_string()),
        }))?);
        Ok(())
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "one bounded scan preserves filter-search-delta-group ordering"
)]
fn scan_context(
    prepared: &PreparedSnapshot,
    kind: RelationKind,
    group: RelationGroup,
    context: &PageContext<'_>,
    aggregates: &mut BTreeMap<GroupKey, Aggregate>,
    cancelled: &impl Fn() -> bool,
) -> Result<(), ApiError> {
    let mut offset = 0_u64;
    while offset < context.rows {
        let mut chunk = Vec::new();
        context.source.visit_rows(
            context.plan.type_id,
            &context.plan.projection,
            offset,
            super::SNAPSHOT_CHUNK_ROWS,
            |ordinal, row| {
                if cancelled() {
                    return false;
                }
                chunk.push((ordinal, row));
                true
            },
        )?;
        if cancelled() || chunk.is_empty() {
            break;
        }
        offset = chunk
            .last()
            .map_or(context.rows, |(ordinal, _row)| ordinal.saturating_add(1));
        let mut ids = HashSet::new();
        for (_ordinal, row) in &chunk {
            if !context.window.matches(row) {
                continue;
            }
            for (_name, cell) in row.iter() {
                if let Cell::StrId(id) = cell {
                    ids.insert(*id);
                }
            }
        }
        let dictionary = resolved_dictionary(context.source, &ids)?;
        for (ordinal, row) in chunk {
            if !context.window.matches(&row)
                || !context.plan.matches(&row, &dictionary)
                || (!prepared.search.is_empty()
                    && !search_matches(
                        &row,
                        &dictionary,
                        &context.search_columns,
                        &prepared.search,
                    ))
            {
                continue;
            }
            let Some(identity) = identity_of(context.plan, &row) else {
                continue;
            };
            let Some(object_key) =
                GroupKey::from_row(kind, RelationGroup::Object, &row, &dictionary)?
            else {
                continue;
            };
            let Some(timestamp) = context
                .plan
                .timestamp
                .and_then(|name| timestamp_cell(row.get(name)))
            else {
                continue;
            };
            let source = Source {
                context_index: context.context_index,
                ordinal,
                type_id: context.plan.type_id,
                timestamp,
            };
            let before = context
                .previous
                .as_ref()
                .and_then(|previous| previous.get(&identity));
            if !prepared.relation_filters.is_empty() {
                let mut object = Aggregate::new(object_key.clone(), source);
                object.add(
                    kind,
                    context.plan,
                    &row,
                    before,
                    context.elapsed_for(&row),
                    &dictionary,
                    source,
                )?;
                if !object.matches_derived_filters(kind, &prepared.relation_filters) {
                    continue;
                }
            }
            let key = object_key.for_group(kind, group);
            aggregates
                .entry(key.clone())
                .or_insert_with(|| Aggregate::new(key, source))
                .add(
                    kind,
                    context.plan,
                    &row,
                    before,
                    context.elapsed_for(&row),
                    &dictionary,
                    source,
                )?;
        }
    }
    Ok(())
}

fn relation_layout(
    section: &SectionPlans,
    kind: RelationKind,
    group: RelationGroup,
    selected: &[String],
) -> Result<Vec<u8>, ApiError> {
    let available = kind.fields(group);
    let columns = selected
        .iter()
        .filter_map(|name| available.iter().find(|field| field.name == name))
        .map(|field| {
            json!({
                "name": field.name,
                "kind": kind_name(field.kind),
                "unit": field.unit.unwrap_or("none"),
                "nullable": true,
            })
        })
        .collect::<Vec<_>>();
    record(json!({
        "record": "relation_layout",
        "logical_name": section.logical_name,
        "group": group_name(group),
        "columns": columns,
    }))
}

fn relation_record(
    section: &SectionPlans,
    kind: RelationKind,
    group: RelationGroup,
    row: &RelationRow,
    physical_source: bool,
) -> Result<Vec<u8>, ApiError> {
    let values = relation_values(&row.metrics);
    let source = physical_source.then(|| {
        json!({
            "type_id": row.source.type_id.to_string(),
            "ordinal": row.source.ordinal.to_string(),
            "timestamp": row.source.timestamp.to_string(),
        })
    });
    record(json!({
        "record": "relation",
        "logical_name": section.logical_name,
        "group": group_name(group),
        "key": row.key.json(kind, group),
        "values": values,
        "sample_from": row.from.map(|value| value.to_string()),
        "sample_to": row.to.map(|value| value.to_string()),
        "source": source,
    }))
}

fn relation_values(metrics: &BTreeMap<String, Option<Metric>>) -> Map<String, Value> {
    metrics
        .iter()
        .map(|(name, metric)| {
            (
                name.clone(),
                metric.as_ref().map_or(Value::Null, Metric::json),
            )
        })
        .collect()
}

fn compare_rows(left: &RelationRow, right: &RelationRow, direction: Order) -> Ordering {
    let left_value = left.sort.as_ref().and_then(Metric::order_value);
    let right_value = right.sort.as_ref().and_then(Metric::order_value);
    // The physical helper ranks greatest first and keeps nulls last.
    let ordered =
        compare_page_order_values(left_value.as_ref(), right_value.as_ref(), direction).reverse();
    ordered.then_with(|| left.key.cmp(&right.key))
}

fn counter_input(
    plan: &Plan,
    row: &Row,
    before: Option<&CounterReadings>,
    name: &'static str,
    structural: bool,
) -> Input {
    if plan.contract.column(name).is_none() {
        return Input::Unavailable;
    }
    let (Some(now), Some(earlier)) = (row.get(name), before.and_then(|values| values.get(name)))
    else {
        return Input::Unavailable;
    };
    if structural && matches!((now, earlier), (Cell::Null, Cell::Null)) {
        return Input::Neutral;
    }
    counter_delta(now, earlier).map_or(Input::Unavailable, Input::Value)
}

fn gauge_input(plan: &Plan, row: &Row, name: &'static str, structural: bool) -> Input {
    if plan.contract.column(name).is_none() {
        return Input::Unavailable;
    }
    match row.get(name) {
        Some(Cell::Null) if structural => Input::Neutral,
        Some(cell) => super::ordered_cell(cell).map_or(Input::Unavailable, Input::Value),
        None => Input::Unavailable,
    }
}

fn text_cell(stored: Option<&Cell>, dictionary: &Dictionary) -> Result<Option<String>, ApiError> {
    let Some(Cell::StrId(id)) = stored else {
        return Ok(None);
    };
    let bytes = dictionary
        .resolve(*id)
        .map(stored_bytes)
        .ok_or(ApiError::BadCursor)?;
    String::from_utf8(bytes.to_vec())
        .map(Some)
        .map_err(|error| ApiError::Unreadable(Box::new(error)))
}

const fn unsigned_cell(stored: Option<&Cell>) -> Option<u32> {
    match stored {
        Some(Cell::U32(value)) => Some(*value),
        _ => None,
    }
}

fn integer_cell(stored: Option<&Cell>) -> Option<i128> {
    super::ordered_cell(stored?).and_then(|value| match value {
        OrderedNumber::Integer(value) => Some(value),
        OrderedNumber::Float(_) => None,
    })
}

const fn timestamp_cell(stored: Option<&Cell>) -> Option<i64> {
    match stored {
        Some(Cell::Ts(value)) => Some(*value),
        _ => None,
    }
}

const fn bool_cell(stored: Option<&Cell>) -> Option<bool> {
    match stored {
        Some(Cell::Bool(value)) => Some(*value),
        _ => None,
    }
}

fn number_is_zero(value: OrderedNumber) -> bool {
    match value {
        OrderedNumber::Integer(value) => value == 0,
        OrderedNumber::Float(value) => value == 0.0,
    }
}

const fn group_name(group: RelationGroup) -> &'static str {
    match group {
        RelationGroup::Database => "database",
        RelationGroup::Schema => "schema",
        RelationGroup::Object => "object",
    }
}

const fn order_name(order: Order) -> &'static str {
    match order {
        Order::Asc => "asc",
        Order::Desc => "desc",
    }
}

const fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Number | Kind::Integer => "number",
        Kind::Id => "id",
        Kind::Timestamp => "timestamp",
        Kind::Boolean => "bool",
        Kind::Text => "text",
    }
}

fn sort_name(name: &str) -> &str {
    name.strip_prefix("derived.").unwrap_or(name)
}

const fn key_fields(kind: RelationKind, group: RelationGroup) -> &'static [&'static str] {
    match (kind, group) {
        (_, RelationGroup::Database) => &["datid", "datname"],
        (_, RelationGroup::Schema) => &["datid", "datname", "schemaname"],
        (RelationKind::Tables, RelationGroup::Object) => {
            &["datid", "datname", "schemaname", "relid", "relname"]
        }
        (RelationKind::Indexes, RelationGroup::Object) => &[
            "datid",
            "datname",
            "schemaname",
            "relid",
            "relname",
            "indexrelid",
            "indexrelname",
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(datid: u32) -> GroupKey {
        GroupKey {
            datid,
            datname: format!("db{datid}"),
            schemaname: Some("public".to_owned()),
            relid: Some(10 + datid),
            relname: Some(format!("table{datid}")),
            indexrelid: Some(20 + datid),
            indexrelname: Some(format!("index{datid}")),
        }
    }

    const fn source(context_index: usize, ordinal: u64) -> Source {
        Source {
            context_index,
            ordinal,
            type_id: 1_014_002,
            timestamp: 42,
        }
    }

    fn aggregate() -> Aggregate {
        Aggregate::new(key(1), source(2, 3))
    }

    fn add_rate(rate: &mut RateAggregate, delta: i128) {
        rate.add(Input::Value(OrderedNumber::Integer(delta)), Some(1_000_000));
    }

    fn metric_number(metric: Option<Metric>) -> Value {
        metric.expect("available metric").json()
    }

    #[test]
    fn relation_contract_rejects_physical_and_cross_section_fields() {
        let sections = vec![TABLES.to_owned()];
        assert!(matches!(
            output_fields(
                &sections,
                RelationGroup::Database,
                &["relid".to_owned()]
            ),
            Err(ApiError::NoSuchColumn(name)) if name == "relid"
        ));
        assert!(matches!(
            output_fields(
                &[TABLES.to_owned(), INDEXES.to_owned()],
                RelationGroup::Database,
                &[]
            ),
            Err(ApiError::BadFilter(name)) if name == "group"
        ));
    }

    #[test]
    fn low_activity_filter_is_one_fixed_true_indexes_predicate() {
        let mut request = SnapshotRequest {
            segment_id: 1,
            at: 2,
            sections: vec![INDEXES.to_owned()],
            fields: Vec::new(),
            by: Vec::new(),
            direction: Order::Asc,
            group: Some(RelationGroup::Object),
            page_size: Some(200),
            cursor: None,
            search: Vec::new(),
            text: None,
            filters: vec![Filter {
                column: "no_scans".to_owned(),
                value: "true".to_owned(),
            }],
            type_id: None,
            row_ordinal: None,
        };
        let (physical, derived) = split_filters(&request).unwrap();
        assert!(physical.is_empty());
        assert_eq!(derived, request.filters);

        request.filters[0].value = "false".to_owned();
        assert!(matches!(
            split_filters(&request),
            Err(ApiError::BadFilter(name)) if name == "no_scans"
        ));
        request.filters[0].value = "true".to_owned();
        request.sections[0] = TABLES.to_owned();
        assert!(matches!(
            split_filters(&request),
            Err(ApiError::BadFilter(name)) if name == "no_scans"
        ));

        let mut zero = aggregate();
        let mut scan_rate = RateAggregate::default();
        add_rate(&mut scan_rate, 0);
        zero.rates.insert("idx_scan", scan_rate);
        assert!(zero.matches_derived_filters(
            RelationKind::Indexes,
            &[Filter {
                column: "no_scans".to_owned(),
                value: "true".to_owned()
            }]
        ));
        zero.rates
            .get_mut("idx_scan")
            .unwrap()
            .add(Input::Unavailable, Some(1_000_000));
        assert!(!zero.matches_derived_filters(
            RelationKind::Indexes,
            &[Filter {
                column: "no_scans".to_owned(),
                value: "true".to_owned()
            }]
        ));
    }

    #[test]
    fn object_keys_are_minimal_and_display_identity_is_a_value() {
        let table_key = key(7).json(RelationKind::Tables, RelationGroup::Object);
        assert_eq!(
            table_key
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            ["datid", "datname", "relid", "relname", "schemaname"]
        );
        let index_key = key(7).json(RelationKind::Indexes, RelationGroup::Object);
        assert_eq!(
            index_key
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            [
                "datid",
                "datname",
                "indexrelid",
                "indexrelname",
                "relid",
                "relname",
                "schemaname"
            ]
        );
        let output = output_fields(
            &[INDEXES.to_owned()],
            RelationGroup::Object,
            &[
                "datid".to_owned(),
                "indexrelid".to_owned(),
                "indexrelname".to_owned(),
                "relid".to_owned(),
                "relname".to_owned(),
            ],
        )
        .unwrap();
        assert!(output.is_empty(), "identity values are emitted in the key");

        assert!(matches!(
            output_fields(
                &[INDEXES.to_owned()],
                RelationGroup::Object,
                &["indexdef".to_owned()]
            ),
            Err(ApiError::NoSuchColumn(name)) if name == "indexdef"
        ));
    }

    #[test]
    fn database_schema_and_object_keys_keep_database_scope() {
        let first = key(1);
        let second = key(2);
        assert_eq!(first.schemaname, second.schemaname);
        assert_ne!(
            first, second,
            "the same schema name in two databases is distinct"
        );

        assert_eq!(
            first
                .json(RelationKind::Tables, RelationGroup::Database)
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            ["datid", "datname"]
        );
        assert_eq!(
            first
                .json(RelationKind::Tables, RelationGroup::Schema)
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            ["datid", "datname", "schemaname"]
        );
        assert_ne!(
            first.json(RelationKind::Tables, RelationGroup::Object),
            second.json(RelationKind::Tables, RelationGroup::Object)
        );
    }

    #[test]
    fn aggregate_contract_has_explicit_counts_and_timestamp_semantics() {
        let table = RelationKind::Tables.fields(RelationGroup::Schema);
        assert!(table.iter().any(|field| field.name == "table_count"));
        assert!(table.iter().any(|field| field.name == "last_vacuum_oldest"));
        assert!(
            table
                .iter()
                .any(|field| field.name == "last_vacuum_never_count")
        );
        assert!(
            table
                .iter()
                .any(|field| field.name == "toast_last_autovacuum_latest")
        );
        assert!(!table.iter().any(|field| field.name == "tablespace"));

        let index = RelationKind::Indexes.fields(RelationGroup::Database);
        assert!(index.iter().any(|field| field.name == "index_count"));
        assert!(index.iter().any(|field| field.name == "invalid_count"));
        assert!(!index.iter().any(|field| field.name == "indisvalid"));
    }

    #[test]
    fn unavailable_values_are_explicit_nulls() {
        let metrics = BTreeMap::from([
            ("available".to_owned(), Some(Metric::Integer(7))),
            ("unavailable".to_owned(), None),
        ]);
        let values = relation_values(&metrics);
        assert_eq!(
            values.get("available"),
            Some(&Value::String("7".to_owned()))
        );
        assert_eq!(values.get("unavailable"), Some(&Value::Null));
    }

    #[test]
    fn emitted_wire_distinguishes_groups_from_physical_objects() {
        let section = SectionPlans {
            logical_name: INDEXES.to_owned(),
            plans: Vec::new(),
        };
        let row = RelationRow {
            key: key(7),
            metrics: BTreeMap::from([("idx_scan".to_owned(), Some(Metric::Integer(3)))]),
            sort: None,
            source: source(4, 91),
            from: Some(10),
            to: Some(20),
        };
        let object: Value = serde_json::from_slice(
            &relation_record(
                &section,
                RelationKind::Indexes,
                RelationGroup::Object,
                &row,
                true,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(object["record"], "relation");
        assert_eq!(object["key"]["datid"], "7");
        assert_eq!(object["key"]["relid"], "17");
        assert_eq!(object["key"]["relname"], "table7");
        assert_eq!(object["key"]["indexrelid"], "27");
        assert_eq!(object["key"]["indexrelname"], "index7");
        assert_eq!(object["values"]["idx_scan"], "3");
        assert_eq!(object["source"]["ordinal"], "91");

        let aggregate: Value = serde_json::from_slice(
            &relation_record(
                &section,
                RelationKind::Indexes,
                RelationGroup::Database,
                &row,
                false,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(aggregate["source"].is_null());
        assert!(aggregate["key"].get("indexrelid").is_none());
        assert!(aggregate["key"].get("relid").is_none());
    }

    #[test]
    fn additive_rates_and_ratios_sum_each_objects_own_interval() {
        let mut aggregate = aggregate();
        let mut sequential = RateAggregate::default();
        sequential.add(Input::Value(OrderedNumber::Integer(10)), Some(10_000_000));
        sequential.add(Input::Value(OrderedNumber::Integer(20)), Some(5_000_000));
        aggregate.rates.insert("seq_scan", sequential);
        let mut indexed = RateAggregate::default();
        indexed.add(Input::Value(OrderedNumber::Integer(10)), Some(10_000_000));
        indexed.add(Input::Value(OrderedNumber::Integer(5)), Some(5_000_000));
        aggregate.rates.insert("idx_scan", indexed);
        assert_eq!(
            metric_number(sequential.metric()),
            json!(5.0),
            "10/10s + 20/5s is five scans per second"
        );
        assert_eq!(
            metric_number(aggregate.metric(
                RelationKind::Tables,
                RelationGroup::Database,
                "sequential_share_pct",
            )),
            json!(500.0 / 7.0),
            "the percentage is recomputed from the summed 5/s and 2/s operands"
        );

        let Some(order) = sequential.metric().and_then(|metric| metric.order_value()) else {
            panic!("exact rate order value");
        };
        assert!(matches!(
            order,
            super::super::PageOrderValue::IntegerRatio { .. }
        ));
    }

    #[test]
    fn reset_and_layout_unknown_poison_but_structural_null_is_neutral() {
        let mut sum = Availability::default();
        sum.add(Input::Neutral);
        assert!(sum.value().is_none(), "all structural N/A stays null");
        sum.add(Input::Value(OrderedNumber::Integer(8)));
        assert!(matches!(sum.value(), Some(OrderedNumber::Integer(8))));
        sum.add(Input::Unavailable);
        assert!(
            sum.value().is_none(),
            "reset/layout absence poisons the sum"
        );

        let mut rate = RateAggregate::default();
        rate.add(Input::Neutral, None);
        assert!(rate.metric().is_none(), "all structural N/A stays null");
        add_rate(&mut rate, 8);
        rate.add(Input::Unavailable, Some(1_000_000));
        assert!(
            rate.metric().is_none(),
            "one reset poisons an aggregate rate"
        );

        let mut missing_elapsed = RateAggregate::default();
        missing_elapsed.add(Input::Value(OrderedNumber::Integer(1)), None);
        assert!(missing_elapsed.metric().is_none());
    }

    #[test]
    fn ages_take_the_maximum_ignore_partition_na_and_reject_layout_absence() {
        let mut maximum = MaximumAggregate::default();
        maximum.add(true, Some(&Cell::Null));
        maximum.add(true, Some(&Cell::I64(41)));
        maximum.add(true, Some(&Cell::I64(17)));
        assert_eq!(maximum.exact(), Some(41));

        let mut all_na = MaximumAggregate::default();
        all_na.add(true, Some(&Cell::Null));
        assert_eq!(all_na.exact(), None);

        maximum.add(false, None);
        assert_eq!(maximum.exact(), None, "layout absence poisons the maximum");
    }

    #[test]
    fn timestamps_keep_oldest_latest_and_explicit_never() {
        let mut timestamp = TimestampAggregate::default();
        timestamp.add(true, Some(&Cell::Ts(30)), false);
        timestamp.add(true, Some(&Cell::Null), false);
        timestamp.add(true, Some(&Cell::Ts(10)), false);
        assert!(timestamp.exact());
        assert_eq!(timestamp.oldest, Some(10));
        assert_eq!(timestamp.latest, Some(30));
        assert_eq!(timestamp.never, 1);

        let mut toast = TimestampAggregate::default();
        toast.add(true, Some(&Cell::Null), true);
        assert!(!toast.exact(), "all no-TOAST rows stay unavailable");
        toast.add(true, Some(&Cell::Null), false);
        assert!(toast.exact());
        assert_eq!(toast.never, 1);

        let mut absent = TimestampAggregate::default();
        absent.add(false, None, false);
        assert!(!absent.exact());
    }

    #[test]
    fn index_flags_and_each_objects_scan_delta_are_counted() {
        let mut scans = BoolAggregate::default();
        scans.add_scan(Input::Value(OrderedNumber::Integer(0)));
        scans.add_scan(Input::Value(OrderedNumber::Integer(5)));
        scans.add_scan(Input::Value(OrderedNumber::Integer(0)));
        assert!(scans.exact());
        assert_eq!((scans.known, scans.truthy), (3, 2));

        let mut aggregate = aggregate();
        let mut valid = BoolAggregate::default();
        valid.add(Some(&Cell::Bool(true)));
        valid.add(Some(&Cell::Bool(false)));
        aggregate.flags.insert("indisvalid", valid);
        assert!(matches!(
            aggregate.flag_count("indisvalid", false),
            Some(Metric::Integer(1))
        ));

        aggregate.state_severity = Some(2);
        assert!(matches!(
            aggregate.metric(
                RelationKind::Indexes,
                RelationGroup::Database,
                "state_severity"
            ),
            Some(Metric::Integer(2))
        ));

        scans.add_scan(Input::Unavailable);
        assert!(!scans.exact(), "unknown scan delta poisons the count");
    }

    #[test]
    fn derived_hidden_sort_has_stable_key_ties_and_cursor_coordinates() {
        let row = |datid, sort| RelationRow {
            key: key(datid),
            metrics: BTreeMap::new(),
            sort,
            source: source(usize::try_from(datid).unwrap(), u64::from(datid)),
            from: Some(1),
            to: Some(2),
        };
        let high = row(2, Some(Metric::Integer(20)));
        let low = row(1, Some(Metric::Integer(10)));
        assert_eq!(compare_rows(&high, &low, Order::Desc), Ordering::Less);
        assert_eq!(compare_rows(&low, &high, Order::Asc), Ordering::Less);
        let tied_left = row(1, Some(Metric::Integer(10)));
        let tied_right = row(2, Some(Metric::Integer(10)));
        assert_eq!(
            compare_rows(&tied_left, &tied_right, Order::Desc),
            Ordering::Less
        );
        assert!(tied_left.metrics.is_empty(), "sort need not be projected");
        assert_eq!(sort_name("derived.state_severity"), "state_severity");

        let cursor = SnapshotCursor {
            segment_id: 7,
            active_position: 11,
            context_index: high.source.context_index,
            ordinal: high.source.ordinal,
            binding: 13,
        };
        assert_eq!(SnapshotCursor::parse(&cursor.encode()).unwrap(), cursor);
    }

    #[test]
    fn nulls_stay_last_in_both_directions() {
        let value = Some(super::super::PageOrderValue::Integer(1));
        assert_eq!(
            compare_page_order_values(value.as_ref(), None, Order::Asc),
            Ordering::Greater
        );
        assert_eq!(
            compare_page_order_values(value.as_ref(), None, Order::Desc),
            Ordering::Greater
        );
    }

    #[test]
    fn reltuples_unknown_poisons_the_sum_and_zero_remains_exact() {
        let mut aggregate = Availability::default();
        aggregate.add(Input::Value(OrderedNumber::Integer(0)));
        aggregate.add(Input::Value(OrderedNumber::Integer(12)));
        assert!(matches!(
            aggregate.value(),
            Some(OrderedNumber::Integer(12))
        ));
        aggregate.add(Input::Unavailable);
        assert!(aggregate.value().is_none());
    }
}
