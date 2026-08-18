//! Fixed relation views for `PostgreSQL` table and index snapshots.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};

#[cfg(test)]
use std::cell::Cell as Counter;

use kronika_reader::{Cell, Dictionary, Reader, Row, Segment, SegmentRef};
use kronika_registry::{contract, logical_section_name};
use serde_json::{Map, Value, json};

use super::{
    ApiError, CounterReadings, Order, OrderedNumber, PageContext, Plan, PreparedSnapshot,
    RelationGroup, SectionPlans, SnapshotCursor, add_ordered, compare_page_order_values,
    counter_delta, identity_of, plans, rate_columns, record, resolved_dictionary, search_matches,
    stored_bytes,
};
use crate::api::query;
use crate::route::{DataRequest, Filter, SegmentRequest, SeriesRequest, SnapshotRequest, Window};

const TABLES: &str = "pg_stat_user_tables";
const INDEXES: &str = "pg_stat_user_indexes";

#[cfg(test)]
thread_local! {
    static HISTORY_SELECTION_VISITS: Counter<usize> = const { Counter::new(0) };
    static HISTORY_SOURCE_VISITS: Counter<usize> = const { Counter::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_history_operations() {
    HISTORY_SELECTION_VISITS.set(0);
    HISTORY_SOURCE_VISITS.set(0);
}

#[cfg(test)]
pub(crate) fn history_operations() -> (usize, usize) {
    (HISTORY_SELECTION_VISITS.get(), HISTORY_SOURCE_VISITS.get())
}

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
    count_field("table_count"),
    rate_field("seq_scan"),
    rate_field("seq_tup_read"),
    rate_field("idx_scan"),
    rate_field("idx_tup_fetch"),
    percent_field("sequential_share_pct"),
    rate_field("tuple_throughput"),
    number_field("seq_tuples_per_scan"),
    number_field("idx_tuples_per_scan"),
    field("last_seq_scan_never", Kind::Boolean, None),
    field("last_idx_scan_never", Kind::Boolean, None),
    rate_field("n_tup_ins"),
    rate_field("n_tup_upd"),
    rate_field("n_tup_del"),
    rate_field("dml_total"),
    percent_field("insert_share_pct"),
    percent_field("update_share_pct"),
    percent_field("delete_share_pct"),
    rate_field("n_tup_hot_upd"),
    rate_field("n_tup_newpage_upd"),
    count_field("n_live_tup"),
    count_field("n_dead_tup"),
    count_field("n_mod_since_analyze"),
    count_field("n_ins_since_vacuum"),
    count_field("reltuples"),
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
    integer_field("displayed_storage_bytes", Some("bytes")),
    percent_field("toast_share_pct"),
    count_field("toast_n_live_tup"),
    count_field("toast_n_dead_tup"),
    percent_field("toast_dead_pct"),
    rate_field("heap_blks_read"),
    rate_field("heap_blks_hit"),
    rate_field("idx_blks_read"),
    rate_field("idx_blks_hit"),
    rate_field("toast_blks_read"),
    rate_field("toast_blks_hit"),
    rate_field("tidx_blks_read"),
    rate_field("tidx_blks_hit"),
    percent_field("heap_buffer_hit_pct"),
    percent_field("index_buffer_hit_pct"),
    percent_field("toast_buffer_hit_pct"),
    percent_field("tidx_buffer_hit_pct"),
    percent_field("buffer_hit_pct"),
    integer_field("xid_age", None),
    integer_field("mxid_age", None),
];

const TABLE_AGGREGATE_FIELDS: &[FieldSpec] = &[
    count_field("table_count"),
    rate_field("seq_scan"),
    rate_field("seq_tup_read"),
    rate_field("idx_scan"),
    rate_field("idx_tup_fetch"),
    percent_field("sequential_share_pct"),
    rate_field("tuple_throughput"),
    number_field("seq_tuples_per_scan"),
    number_field("idx_tuples_per_scan"),
    rate_field("n_tup_ins"),
    rate_field("n_tup_upd"),
    rate_field("n_tup_del"),
    rate_field("dml_total"),
    percent_field("insert_share_pct"),
    percent_field("update_share_pct"),
    percent_field("delete_share_pct"),
    rate_field("n_tup_hot_upd"),
    rate_field("n_tup_newpage_upd"),
    count_field("n_live_tup"),
    count_field("n_dead_tup"),
    count_field("n_mod_since_analyze"),
    count_field("n_ins_since_vacuum"),
    count_field("reltuples"),
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
    count_field("last_vacuum_never_count"),
    timestamp_field("last_autovacuum_oldest"),
    timestamp_field("last_autovacuum_latest"),
    count_field("last_autovacuum_never_count"),
    timestamp_field("last_analyze_oldest"),
    timestamp_field("last_analyze_latest"),
    count_field("last_analyze_never_count"),
    timestamp_field("last_autoanalyze_oldest"),
    timestamp_field("last_autoanalyze_latest"),
    count_field("last_autoanalyze_never_count"),
    timestamp_field("last_seq_scan_oldest"),
    timestamp_field("last_seq_scan_latest"),
    count_field("last_seq_scan_never_count"),
    timestamp_field("last_idx_scan_oldest"),
    timestamp_field("last_idx_scan_latest"),
    count_field("last_idx_scan_never_count"),
    timestamp_field("toast_last_autovacuum_oldest"),
    timestamp_field("toast_last_autovacuum_latest"),
    count_field("toast_last_autovacuum_never_count"),
    integer_field("main_fork_bytes", Some("bytes")),
    integer_field("toast_bytes", Some("bytes")),
    integer_field("displayed_storage_bytes", Some("bytes")),
    percent_field("toast_share_pct"),
    count_field("toast_n_live_tup"),
    count_field("toast_n_dead_tup"),
    percent_field("toast_dead_pct"),
    rate_field("heap_blks_read"),
    rate_field("heap_blks_hit"),
    rate_field("idx_blks_read"),
    rate_field("idx_blks_hit"),
    rate_field("toast_blks_read"),
    rate_field("toast_blks_hit"),
    rate_field("tidx_blks_read"),
    rate_field("tidx_blks_hit"),
    percent_field("heap_buffer_hit_pct"),
    percent_field("index_buffer_hit_pct"),
    percent_field("toast_buffer_hit_pct"),
    percent_field("tidx_buffer_hit_pct"),
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
    count_field("index_count"),
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
    field("last_idx_scan_never", Kind::Boolean, None),
    field("no_scans", Kind::Boolean, None),
    field("indisunique", Kind::Boolean, None),
    field("indisprimary", Kind::Boolean, None),
    field("indisvalid", Kind::Boolean, None),
    field("indisexclusion", Kind::Boolean, None),
    field("indisready", Kind::Boolean, None),
    integer_field("state_severity", None),
];

const INDEX_AGGREGATE_FIELDS: &[FieldSpec] = &[
    count_field("index_count"),
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
    count_field("last_idx_scan_never_count"),
    count_field("no_scan_count"),
    count_field("known_scan_count"),
    count_field("invalid_count"),
    count_field("unready_count"),
    count_field("unique_count"),
    count_field("primary_count"),
    count_field("exclusion_count"),
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

    const fn logical_name(self) -> &'static str {
        match self {
            Self::Tables => TABLES,
            Self::Indexes => INDEXES,
        }
    }

    fn fields(self, group: RelationGroup) -> Vec<FieldSpec> {
        let base = match (self, group) {
            (Self::Tables, RelationGroup::Object) => TABLE_OBJECT_FIELDS,
            (
                Self::Tables,
                RelationGroup::Database | RelationGroup::Schema | RelationGroup::Tablespace,
            ) => TABLE_AGGREGATE_FIELDS,
            (Self::Indexes, RelationGroup::Object) => INDEX_OBJECT_FIELDS,
            (
                Self::Indexes,
                RelationGroup::Database | RelationGroup::Schema | RelationGroup::Tablespace,
            ) => INDEX_AGGREGATE_FIELDS,
        };
        if group != RelationGroup::Tablespace {
            return base.to_vec();
        }
        let mut fields = Vec::with_capacity(base.len().saturating_add(1));
        fields.push(field("tablespace", Kind::Text, None));
        fields.extend_from_slice(base);
        fields
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

const fn count_field(name: &'static str) -> FieldSpec {
    integer_field(name, Some("count"))
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
        if filter.column == "tablespace_oid"
            && filter.value.parse::<u32>().ok().is_none_or(|oid| oid == 0)
        {
            return Err(ApiError::BadFilter(filter.column.clone()));
        }
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
enum GroupKey {
    Database {
        datid: u32,
        datname: String,
    },
    Schema {
        datid: u32,
        datname: String,
        schemaname: String,
    },
    Tablespace {
        tablespace_oid: u32,
    },
    Table {
        datid: u32,
        datname: String,
        schemaname: String,
        relid: u32,
        relname: String,
    },
    Index {
        datid: u32,
        datname: String,
        schemaname: String,
        relid: u32,
        relname: String,
        indexrelid: u32,
        indexrelname: String,
    },
}

impl GroupKey {
    fn from_row(
        kind: RelationKind,
        group: RelationGroup,
        row: &Row,
        dictionary: &Dictionary,
    ) -> Result<Option<Self>, ApiError> {
        if group == RelationGroup::Tablespace {
            return Ok(unsigned_cell(row.get("tablespace_oid"))
                .map(|tablespace_oid| Self::Tablespace { tablespace_oid }));
        }
        let (Some(datid), Some(datname)) = (
            unsigned_cell(row.get("datid")),
            text_cell(row.get("datname"), dictionary)?,
        ) else {
            return Ok(None);
        };
        if group == RelationGroup::Database {
            return Ok(Some(Self::Database { datid, datname }));
        }
        let Some(schemaname) = text_cell(row.get("schemaname"), dictionary)? else {
            return Ok(None);
        };
        if group == RelationGroup::Schema {
            return Ok(Some(Self::Schema {
                datid,
                datname,
                schemaname,
            }));
        }
        let (Some(relid), Some(relname)) = (
            unsigned_cell(row.get("relid")),
            text_cell(row.get("relname"), dictionary)?,
        ) else {
            return Ok(None);
        };
        if kind == RelationKind::Tables {
            return Ok(Some(Self::Table {
                datid,
                datname,
                schemaname,
                relid,
                relname,
            }));
        }
        let (Some(indexrelid), Some(indexrelname)) = (
            unsigned_cell(row.get("indexrelid")),
            text_cell(row.get("indexrelname"), dictionary)?,
        ) else {
            return Ok(None);
        };
        Ok(Some(Self::Index {
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
        match self.clone().for_group(kind, group) {
            Self::Database { datid, datname } => json!({
                "datid": datid.to_string(),
                "datname": datname,
            }),
            Self::Schema {
                datid,
                datname,
                schemaname,
            } => json!({
                "datid": datid.to_string(),
                "datname": datname,
                "schemaname": schemaname,
            }),
            Self::Tablespace { tablespace_oid } => {
                json!({ "tablespace_oid": tablespace_oid.to_string() })
            }
            Self::Table {
                datid,
                datname,
                schemaname,
                relid,
                relname,
            } => json!({
                "datid": datid.to_string(),
                "datname": datname,
                "schemaname": schemaname,
                "relid": relid.to_string(),
                "relname": relname,
            }),
            Self::Index {
                datid,
                datname,
                schemaname,
                relid,
                relname,
                indexrelid,
                indexrelname,
            } => json!({
                "datid": datid.to_string(),
                "datname": datname,
                "schemaname": schemaname,
                "relid": relid.to_string(),
                "relname": relname,
                "indexrelid": indexrelid.to_string(),
                "indexrelname": indexrelname,
            }),
        }
    }

    fn for_group(self, _kind: RelationKind, group: RelationGroup) -> Self {
        match group {
            RelationGroup::Object => self,
            RelationGroup::Database => match self {
                Self::Table { datid, datname, .. } | Self::Index { datid, datname, .. } => {
                    Self::Database { datid, datname }
                }
                key => key,
            },
            RelationGroup::Schema => match self {
                Self::Table {
                    datid,
                    datname,
                    schemaname,
                    ..
                }
                | Self::Index {
                    datid,
                    datname,
                    schemaname,
                    ..
                } => Self::Schema {
                    datid,
                    datname,
                    schemaname,
                },
                key => key,
            },
            RelationGroup::Tablespace => match self {
                key @ Self::Tablespace { .. } => key,
                _ => unreachable!("tablespace keys are formed directly from physical rows"),
            },
        }
    }

    #[allow(
        clippy::unnested_or_patterns,
        reason = "each tuple arm keeps the requested field name attached to its variants"
    )]
    fn metric(&self, name: &str) -> Option<Metric> {
        match (self, name) {
            (Self::Database { datid, .. } | Self::Schema { datid, .. }, "datid")
            | (Self::Table { datid, .. } | Self::Index { datid, .. }, "datid") => {
                Some(Metric::Integer(i128::from(*datid)))
            }
            (Self::Database { datname, .. } | Self::Schema { datname, .. }, "datname")
            | (Self::Table { datname, .. } | Self::Index { datname, .. }, "datname") => {
                Some(Metric::Text(datname.clone()))
            }
            (Self::Schema { schemaname, .. }, "schemaname")
            | (Self::Table { schemaname, .. } | Self::Index { schemaname, .. }, "schemaname") => {
                Some(Metric::Text(schemaname.clone()))
            }
            (Self::Tablespace { tablespace_oid }, "tablespace_oid") => {
                Some(Metric::Integer(i128::from(*tablespace_oid)))
            }
            (Self::Table { relid, .. } | Self::Index { relid, .. }, "relid") => {
                Some(Metric::Integer(i128::from(*relid)))
            }
            (Self::Table { relname, .. } | Self::Index { relname, .. }, "relname") => {
                Some(Metric::Text(relname.clone()))
            }
            (Self::Index { indexrelid, .. }, "indexrelid") => {
                Some(Metric::Integer(i128::from(*indexrelid)))
            }
            (Self::Index { indexrelname, .. }, "indexrelname") => {
                Some(Metric::Text(indexrelname.clone()))
            }
            _ => None,
        }
    }

    const fn is_tablespace(&self) -> bool {
        matches!(self, Self::Tablespace { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Source {
    segment_id: i64,
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

    fn merge(&mut self, other: Self) {
        self.add(match other {
            Self::Empty => Input::Neutral,
            Self::Value(value) => Input::Value(value),
            Self::Unavailable => Input::Unavailable,
        });
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

    fn merge(&mut self, other: Self) {
        if self.unavailable || other.unavailable {
            self.unavailable = true;
            self.value = None;
            return;
        }
        if let Some(value) = other.value {
            self.value = self.value.map_or(Some(value), |known| known.add(value));
            if self.value.is_none() {
                self.unavailable = true;
            }
        }
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

    fn merge(&mut self, other: Self) {
        self.unavailable |= other.unavailable;
        if let Some(value) = other.maximum {
            self.maximum = Some(self.maximum.map_or(value, |known| known.max(value)));
        }
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

    fn merge(&mut self, other: Self) {
        self.applicable = self.applicable.saturating_add(other.applicable);
        self.unavailable |= other.unavailable;
        self.oldest = match (self.oldest, other.oldest) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (left, right) => left.or(right),
        };
        self.latest = match (self.latest, other.latest) {
            (Some(left), Some(right)) => Some(left.max(right)),
            (left, right) => left.or(right),
        };
        self.never = self.never.saturating_add(other.never);
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

    const fn merge(&mut self, other: Self) {
        self.known = self.known.saturating_add(other.known);
        self.truthy = self.truthy.saturating_add(other.truthy);
        self.unavailable |= other.unavailable;
    }
}

#[derive(Clone)]
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
    tablespace_label_timestamp: Option<i64>,
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
            tablespace_label_timestamp: None,
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
        if self.key.is_tablespace()
            && let Some(label) = text_cell(row.get("tablespace"), dictionary)?
        {
            let replace = self.tablespace_label_timestamp.is_none_or(|timestamp| {
                source.timestamp > timestamp
                    || source.timestamp == timestamp
                        && self
                            .texts
                            .get("tablespace")
                            .is_none_or(|current| label.as_bytes() < current.as_bytes())
            });
            if replace {
                self.tablespace_label_timestamp = Some(source.timestamp);
                self.texts.insert("tablespace", label);
            }
        }
        match kind {
            RelationKind::Tables => self.add_table(plan, row, before, elapsed, dictionary)?,
            RelationKind::Indexes => self.add_index(plan, row, before, elapsed, dictionary)?,
        }
        Ok(())
    }

    fn merge(&mut self, other: &Self) {
        self.count = self.count.saturating_add(other.count);
        self.source = self.source.min(other.source);
        self.from = match (self.from, other.from) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (left, right) => left.or(right),
        };
        self.to = match (self.to, other.to) {
            (Some(left), Some(right)) => Some(left.max(right)),
            (left, right) => left.or(right),
        };
        for (&name, rate) in &other.rates {
            self.rates.entry(name).or_default().merge(*rate);
        }
        for (&name, gauge) in &other.gauges {
            self.gauges.entry(name).or_default().merge(*gauge);
        }
        for (&name, maximum) in &other.maxima {
            self.maxima.entry(name).or_default().merge(*maximum);
        }
        for (&name, timestamp) in &other.timestamps {
            self.timestamps.entry(name).or_default().merge(*timestamp);
        }
        for (&name, flag) in &other.flags {
            self.flags.entry(name).or_default().merge(*flag);
        }
        self.no_scans.merge(other.no_scans);
        self.state_severity = match (self.state_severity, other.state_severity) {
            (Some(left), Some(right)) => Some(left.max(right)),
            _ => None,
        };
        if let Some(timestamp) = other.tablespace_label_timestamp
            && let Some(label) = other.texts.get("tablespace")
        {
            self.update_tablespace_label(timestamp, label.clone());
        }
    }

    fn update_tablespace_label(&mut self, timestamp: i64, label: String) {
        let replace = self.tablespace_label_timestamp.is_none_or(|known| {
            timestamp > known
                || timestamp == known
                    && self
                        .texts
                        .get("tablespace")
                        .is_none_or(|current| label.as_bytes() < current.as_bytes())
        });
        if replace {
            self.tablespace_label_timestamp = Some(timestamp);
            self.texts.insert("tablespace", label);
        }
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
            let structural_na = structural && matches!(row.get("toast_bytes"), Some(Cell::Null));
            let mut input = gauge_input(plan, row, name, structural_na);
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
            return (group == RelationGroup::Object
                || group == RelationGroup::Tablespace && name == "tablespace")
                .then(|| Metric::Text(value.clone()));
        }
        if let Some(value) = self.identifiers.get(name) {
            return (group == RelationGroup::Object).then_some(Metric::Integer(*value));
        }
        match (kind, name) {
            (RelationKind::Tables, "sequential_share_pct") => {
                self.ratio_neutral(&["seq_scan"], &["seq_scan", "idx_scan"], 100.0)
            }
            (RelationKind::Tables, "tuple_throughput") => self
                .rate_sum_neutral(&["seq_tup_read", "idx_tup_fetch"])
                .map(Metric::Rate),
            (RelationKind::Tables, "seq_tuples_per_scan") => {
                self.ratio(&["seq_tup_read"], &["seq_scan"], 1.0)
            }
            (RelationKind::Tables, "idx_tuples_per_scan")
            | (RelationKind::Indexes, "fetches_per_scan") => {
                self.ratio(&["idx_tup_fetch"], &["idx_scan"], 1.0)
            }
            (RelationKind::Tables, "last_seq_scan_never") if group == RelationGroup::Object => {
                self.never("last_seq_scan")
            }
            (RelationKind::Tables | RelationKind::Indexes, "last_idx_scan_never")
                if group == RelationGroup::Object =>
            {
                self.never("last_idx_scan")
            }
            (RelationKind::Tables, "dml_total") => self
                .rate_sum(&["n_tup_ins", "n_tup_upd", "n_tup_del"])
                .map(Metric::Rate),
            (RelationKind::Tables, "insert_share_pct") => self.ratio(
                &["n_tup_ins"],
                &["n_tup_ins", "n_tup_upd", "n_tup_del"],
                100.0,
            ),
            (RelationKind::Tables, "update_share_pct") => self.ratio(
                &["n_tup_upd"],
                &["n_tup_ins", "n_tup_upd", "n_tup_del"],
                100.0,
            ),
            (RelationKind::Tables, "delete_share_pct") => self.ratio(
                &["n_tup_del"],
                &["n_tup_ins", "n_tup_upd", "n_tup_del"],
                100.0,
            ),
            (RelationKind::Tables, "dead_pct") => {
                self.gauge_ratio(&["n_dead_tup"], &["n_live_tup", "n_dead_tup"], 100.0)
            }
            (RelationKind::Tables, "hot_pct") => {
                self.ratio(&["n_tup_hot_upd"], &["n_tup_upd"], 100.0)
            }
            (RelationKind::Tables, "new_page_pct") => {
                self.ratio(&["n_tup_newpage_upd"], &["n_tup_upd"], 100.0)
            }
            (RelationKind::Tables, "displayed_storage_bytes") => self
                .gauge_sum_neutral(&["main_fork_bytes", "toast_bytes"])
                .map(Metric::Number),
            (RelationKind::Tables, "toast_share_pct") => self.gauge_ratio_neutral(
                &["toast_bytes"],
                &["main_fork_bytes", "toast_bytes"],
                100.0,
            ),
            (RelationKind::Tables, "toast_dead_pct") => self.gauge_ratio(
                &["toast_n_dead_tup"],
                &["toast_n_live_tup", "toast_n_dead_tup"],
                100.0,
            ),
            (RelationKind::Tables, "heap_buffer_hit_pct") => self.ratio(
                &["heap_blks_hit"],
                &["heap_blks_read", "heap_blks_hit"],
                100.0,
            ),
            (RelationKind::Tables, "index_buffer_hit_pct")
            | (RelationKind::Indexes, "buffer_hit_pct") => {
                self.ratio(&["idx_blks_hit"], &["idx_blks_read", "idx_blks_hit"], 100.0)
            }
            (RelationKind::Tables, "toast_buffer_hit_pct") => self.ratio(
                &["toast_blks_hit"],
                &["toast_blks_read", "toast_blks_hit"],
                100.0,
            ),
            (RelationKind::Tables, "tidx_buffer_hit_pct") => self.ratio(
                &["tidx_blks_hit"],
                &["tidx_blks_read", "tidx_blks_hit"],
                100.0,
            ),
            (RelationKind::Tables, "buffer_hit_pct") => self.ratio_neutral(
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
                .then(|| Metric::Integer(i128::from(self.no_scans.known - self.no_scans.truthy))),
            (RelationKind::Indexes, "state_severity") => self.state_severity.map(Metric::Integer),
            (RelationKind::Indexes, "invalid_count") => self.flag_count("indisvalid", false),
            (RelationKind::Indexes, "unready_count") => self.flag_count("indisready", false),
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
            numerator: self.rate_sum(numerator)?,
            denominator: self.rate_sum(denominator)?,
            scale,
        })
    }

    fn ratio_neutral(
        &self,
        numerator: &[&str],
        denominator: &[&str],
        scale: f64,
    ) -> Option<Metric> {
        Some(Metric::RateRatio {
            numerator: self.rate_sum_neutral(numerator)?,
            denominator: self.rate_sum_neutral(denominator)?,
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

    fn gauge_ratio_neutral(
        &self,
        numerator: &[&str],
        denominator: &[&str],
        scale: f64,
    ) -> Option<Metric> {
        Some(Metric::Ratio {
            numerator: self.gauge_sum_neutral(numerator)?,
            denominator: self.gauge_sum_neutral(denominator)?,
            scale,
        })
    }

    fn rate_sum(&self, names: &[&str]) -> Option<RateValue> {
        sum_rates(&self.rates, names)
    }

    fn rate_sum_neutral(&self, names: &[&str]) -> Option<RateValue> {
        sum_rates_neutral(&self.rates, names)
    }

    fn gauge_sum_neutral(&self, names: &[&str]) -> Option<OrderedNumber> {
        sum_values_neutral(&self.gauges, names)
    }

    fn never(&self, name: &str) -> Option<Metric> {
        let timestamp = self.timestamps.get(name).copied()?;
        timestamp
            .exact()
            .then_some(Metric::Boolean(timestamp.never == 1))
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

fn sum_rates_neutral(
    rates: &BTreeMap<&'static str, RateAggregate>,
    names: &[&str],
) -> Option<RateValue> {
    let mut sum: Option<RateValue> = None;
    for name in names {
        let rate = rates.get(name)?;
        if rate.unavailable {
            return None;
        }
        let Some(value) = rate.value else {
            continue;
        };
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

fn sum_values_neutral(
    values: &BTreeMap<&'static str, Availability>,
    names: &[&str],
) -> Option<OrderedNumber> {
    names.iter().try_fold(None, |sum, name| {
        let value = match values.get(name)? {
            Availability::Value(value) => *value,
            Availability::Empty => OrderedNumber::Integer(0),
            Availability::Unavailable => return None,
        };
        add_ordered(sum, value).map(Some)
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

struct HistorySegment {
    segment: SegmentRef,
    plans: Vec<Plan>,
}

struct HistoryPrevious {
    timestamp: i64,
    readings: CounterReadings,
}

#[derive(Clone, Copy)]
struct HistoryMoment {
    type_id: u32,
    previous: Option<i64>,
}

#[expect(
    clippy::too_many_lines,
    reason = "one grouped stream keeps validation, the fixed two passes, and ordered emission together"
)]
pub(super) fn stream_history(
    reader: &Reader,
    listed: &[SegmentRef],
    window: Window,
    request: &SeriesRequest,
    emit: &mut impl FnMut(Vec<u8>) -> bool,
    cancelled: &impl Fn() -> bool,
) -> Result<(), ApiError> {
    let group = request
        .group
        .filter(|group| *group != RelationGroup::Object)
        .ok_or_else(|| ApiError::BadFilter("group".to_owned()))?;
    let kind = RelationKind::from_name(&request.section)?;
    let fields = output_fields(
        std::slice::from_ref(&request.section),
        group,
        &request.fields,
    )?;
    if fields.is_empty() || request.type_id.is_some() {
        return Err(ApiError::BadFilter("group".to_owned()));
    }
    if group == RelationGroup::Tablespace {
        return stream_tablespace_history(
            reader, listed, window, request, kind, &fields, emit, cancelled,
        );
    }
    let datid = history_datid(group, &request.filters)?;
    let Some((from, to)) = window.from.zip(window.to) else {
        return Ok(());
    };
    let refs = history_segments(reader, listed, &request.section, datid, from, to, cancelled)?;
    let physical_fields = history_physical_fields(kind, group, &fields);
    let mut sources = Vec::with_capacity(refs.len());
    for segment_ref in refs {
        if cancelled() {
            return Ok(());
        }
        let segment = reader.open_segment(&segment_ref)?;
        let fields = physical_fields
            .iter()
            .filter(|name| {
                segment
                    .layouts(&request.section)
                    .filter_map(|(type_id, _section)| contract(type_id))
                    .any(|layout| layout.column(name).is_some())
            })
            .cloned()
            .collect();
        let data = DataRequest {
            segment: SegmentRequest {
                segment_id: segment_ref.id(),
                section: request.section.clone(),
            },
            fields,
            filters: request.filters.clone(),
            type_id: None,
            after: None,
        };
        match plans(&segment, &data, true) {
            Ok(plans) => sources.push(HistorySegment {
                segment: segment_ref,
                plans,
            }),
            Err(ApiError::NoSuchSection) => {}
            Err(error) => return Err(error),
        }
    }
    let section = SectionPlans {
        logical_name: request.section.clone(),
        plans: Vec::new(),
    };
    if cancelled() || !emit(relation_layout(&section, kind, group, &fields)?) {
        return Ok(());
    }
    let selected = selected_history_layouts(reader, &sources, datid, from, to, cancelled)?;
    if cancelled() {
        return Ok(());
    }
    let mut previous = BTreeMap::<(u32, Vec<super::IdentityCell>), HistoryPrevious>::new();
    let mut aggregates = BTreeMap::<(i64, GroupKey), Aggregate>::new();
    for source in &sources {
        if cancelled() {
            return Ok(());
        }
        let segment = reader.open_segment(&source.segment)?;
        for plan in &source.plans {
            scan_history_plan(
                &segment,
                plan,
                kind,
                group,
                datid,
                from,
                to,
                &selected,
                &mut previous,
                &mut aggregates,
                cancelled,
            )?;
            if cancelled() {
                return Ok(());
            }
        }
    }
    let mut segment_id = None;
    for ((_timestamp, _key), aggregate) in aggregates {
        if segment_id != Some(aggregate.source.segment_id) {
            segment_id = Some(aggregate.source.segment_id);
            if !emit(record(json!({
                "record": "series_segment",
                "segment": { "id": aggregate.source.segment_id.to_string() },
            }))?) {
                return Ok(());
            }
        }
        let metrics = fields
            .iter()
            .map(|name| (name.clone(), aggregate.metric(kind, group, name)))
            .collect();
        let row = RelationRow {
            key: aggregate.key,
            metrics,
            sort: None,
            source: aggregate.source,
            from: aggregate.from,
            to: aggregate.to,
        };
        if cancelled() || !emit(relation_record(&section, kind, group, &row, false)?) {
            return Ok(());
        }
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "the exact cluster-wide stream keeps its source window and output explicit"
)]
#[allow(
    clippy::too_many_lines,
    reason = "the cross-database as-of reducer keeps scan and emission state adjacent"
)]
fn stream_tablespace_history(
    reader: &Reader,
    listed: &[SegmentRef],
    window: Window,
    request: &SeriesRequest,
    kind: RelationKind,
    fields: &[String],
    emit: &mut impl FnMut(Vec<u8>) -> bool,
    cancelled: &impl Fn() -> bool,
) -> Result<(), ApiError> {
    let tablespace_oid = history_tablespace_oid(&request.filters)?;
    let section = SectionPlans {
        logical_name: request.section.clone(),
        plans: Vec::new(),
    };
    if cancelled()
        || !emit(relation_layout(
            &section,
            kind,
            RelationGroup::Tablespace,
            fields,
        )?)
    {
        return Ok(());
    }
    let Some((from, to)) = window.from.zip(window.to) else {
        return Ok(());
    };
    let (refs, datids) =
        tablespace_history_segments(reader, listed, &request.section, from, to, cancelled)?;
    if datids.is_empty() || cancelled() {
        return Ok(());
    }
    let physical_fields = history_physical_fields(kind, RelationGroup::Tablespace, fields);
    let mut sources = Vec::with_capacity(refs.len());
    for segment_ref in refs {
        if cancelled() {
            return Ok(());
        }
        let segment = reader.open_segment(&segment_ref)?;
        let projected = physical_fields
            .iter()
            .filter(|name| {
                segment
                    .layouts(&request.section)
                    .filter_map(|(type_id, _section)| contract(type_id))
                    .any(|layout| layout.column(name).is_some())
            })
            .cloned()
            .collect();
        let data = DataRequest {
            segment: SegmentRequest {
                segment_id: segment_ref.id(),
                section: request.section.clone(),
            },
            fields: projected,
            filters: Vec::new(),
            type_id: None,
            after: None,
        };
        match plans(&segment, &data, true) {
            Ok(plans) => sources.push(HistorySegment {
                segment: segment_ref,
                plans,
            }),
            Err(ApiError::NoSuchSection) => {}
            Err(error) => return Err(error),
        }
    }
    let selected =
        selected_tablespace_history_layouts(reader, &sources, &datids, from, to, cancelled)?;
    let mut previous = BTreeMap::<(u32, Vec<super::IdentityCell>), HistoryPrevious>::new();
    let mut contributions = BTreeMap::<(i64, u32), Aggregate>::new();
    let mut event_sources = BTreeMap::<(i64, u32), Source>::new();
    for source in &sources {
        if cancelled() {
            return Ok(());
        }
        let segment = reader.open_segment(&source.segment)?;
        for plan in &source.plans {
            scan_tablespace_history_plan(
                &segment,
                plan,
                kind,
                tablespace_oid,
                to,
                &datids,
                &selected,
                &mut previous,
                &mut contributions,
                &mut event_sources,
                cancelled,
            )?;
            if cancelled() {
                return Ok(());
            }
        }
    }
    let mut current = BTreeMap::<u32, Aggregate>::new();
    let events = selected.keys().copied().collect::<Vec<_>>();
    let mut cursor = 0;
    let mut emitted_segment = None;
    while cursor < events.len() {
        let timestamp = events[cursor].0;
        let mut event_source = None;
        while cursor < events.len() && events[cursor].0 == timestamp {
            let event = events[cursor];
            if let Some(aggregate) = contributions.remove(&event) {
                current.insert(event.1, aggregate);
            } else {
                current.remove(&event.1);
            }
            if let Some(source) = event_sources.get(&event).copied() {
                event_source = Some(event_source.map_or(source, |known: Source| known.min(source)));
            }
            cursor += 1;
        }
        if timestamp < from || timestamp > to {
            continue;
        }
        let Some(mut aggregate) = current.values().next().cloned() else {
            continue;
        };
        for member in current.values().skip(1) {
            aggregate.merge(member);
        }
        let Some(mut source) = event_source else {
            continue;
        };
        source.timestamp = timestamp;
        aggregate.source = source;
        aggregate.to = Some(timestamp);
        if emitted_segment != Some(source.segment_id) {
            emitted_segment = Some(source.segment_id);
            if !emit(record(json!({
                "record": "series_segment",
                "segment": { "id": source.segment_id.to_string() },
            }))?) {
                return Ok(());
            }
        }
        let metrics = fields
            .iter()
            .map(|name| {
                (
                    name.clone(),
                    aggregate.metric(kind, RelationGroup::Tablespace, name),
                )
            })
            .collect();
        let row = RelationRow {
            key: aggregate.key,
            metrics,
            sort: None,
            source,
            from: aggregate.from,
            to: aggregate.to,
        };
        if cancelled()
            || !emit(relation_record(
                &section,
                kind,
                RelationGroup::Tablespace,
                &row,
                false,
            )?)
        {
            return Ok(());
        }
    }
    Ok(())
}

fn history_tablespace_oid(filters: &[Filter]) -> Result<u32, ApiError> {
    if filters.len() != 1 || filters[0].column != "tablespace_oid" {
        return Err(ApiError::BadFilter("where".to_owned()));
    }
    filters[0]
        .value
        .parse::<u32>()
        .ok()
        .filter(|oid| *oid != 0)
        .ok_or_else(|| ApiError::BadFilter("tablespace_oid".to_owned()))
}

fn tablespace_history_segments(
    reader: &Reader,
    listed: &[SegmentRef],
    logical_name: &str,
    from: i64,
    to: i64,
    cancelled: &impl Fn() -> bool,
) -> Result<(Vec<SegmentRef>, HashSet<u32>), ApiError> {
    let has_section = |segment: &SegmentRef| {
        segment
            .sections()
            .iter()
            .any(|section| logical_section_name(section.type_id) == Some(logical_name))
    };
    let mut selected = listed
        .iter()
        .filter(|segment| {
            has_section(segment) && segment.max_ts() >= from && segment.min_ts() <= to
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut predecessors = BTreeMap::<(u32, u32), Vec<i64>>::new();
    for segment_ref in &selected {
        collect_tablespace_moments(
            reader,
            segment_ref,
            logical_name,
            from,
            to,
            true,
            &mut predecessors,
            cancelled,
        )?;
    }
    for segment_ref in &selected {
        collect_tablespace_moments(
            reader,
            segment_ref,
            logical_name,
            from,
            to,
            false,
            &mut predecessors,
            cancelled,
        )?;
    }
    let required = predecessors.keys().copied().collect::<Vec<_>>();
    let selected_ids = selected.iter().map(SegmentRef::id).collect::<HashSet<_>>();
    let mut candidates = listed
        .iter()
        .filter(|segment| {
            has_section(segment) && segment.min_ts() < from && !selected_ids.contains(&segment.id())
        })
        .collect::<Vec<_>>();
    candidates.sort_unstable_by_key(|segment| (segment.max_ts(), segment.id()));
    for candidate in candidates.into_iter().rev() {
        if cancelled()
            || required.iter().all(|pair| {
                predecessors
                    .get(pair)
                    .is_some_and(|moments| moments.len() >= 2)
            })
        {
            break;
        }
        let changed = collect_tablespace_moments(
            reader,
            candidate,
            logical_name,
            from,
            to,
            false,
            &mut predecessors,
            cancelled,
        )?;
        if changed {
            selected.push((*candidate).clone());
        }
    }
    selected.sort_unstable_by_key(SegmentRef::id);
    selected.dedup_by_key(|segment| segment.id());
    let datids = required
        .into_iter()
        .map(|(datid, _type_id)| datid)
        .collect();
    Ok((selected, datids))
}

#[expect(
    clippy::too_many_arguments,
    reason = "moment discovery keeps its exact time bounds and predecessor state explicit"
)]
fn collect_tablespace_moments(
    reader: &Reader,
    segment_ref: &SegmentRef,
    logical_name: &str,
    from: i64,
    to: i64,
    discover: bool,
    moments: &mut BTreeMap<(u32, u32), Vec<i64>>,
    cancelled: &impl Fn() -> bool,
) -> Result<bool, ApiError> {
    let segment = reader.open_segment(segment_ref)?;
    let mut changed = false;
    for (type_id, _section) in segment.layouts(logical_name) {
        let Some(timestamp) = contract(type_id).and_then(|layout| {
            layout
                .columns
                .iter()
                .find(|column| column.class == kronika_registry::ColumnClass::Timestamp)
                .map(|column| column.name)
        }) else {
            continue;
        };
        segment.visit_rows(
            type_id,
            &[timestamp, "datid"],
            0,
            usize::MAX,
            |_ordinal, row| {
                if cancelled() {
                    return false;
                }
                let (Some(datid), Some(stored)) = (
                    unsigned_cell(row.get("datid")),
                    timestamp_cell(row.get(timestamp)),
                ) else {
                    return true;
                };
                let key = (datid, type_id);
                if discover && (from..=to).contains(&stored) {
                    moments.entry(key).or_default();
                }
                if stored < from
                    && let Some(known) = moments.get_mut(&key)
                    && !known.contains(&stored)
                {
                    known.push(stored);
                    known.sort_unstable_by(|left, right| right.cmp(left));
                    known.truncate(2);
                    changed = true;
                }
                true
            },
        )?;
    }
    Ok(changed)
}

fn selected_tablespace_history_layouts(
    reader: &Reader,
    sources: &[HistorySegment],
    datids: &HashSet<u32>,
    from: i64,
    to: i64,
    cancelled: &impl Fn() -> bool,
) -> Result<BTreeMap<(i64, u32), HistoryMoment>, ApiError> {
    let mut by_layout = BTreeMap::<(u32, u32), std::collections::BTreeSet<i64>>::new();
    for source in sources {
        if cancelled() {
            break;
        }
        let segment = reader.open_segment(&source.segment)?;
        for plan in &source.plans {
            let Some(timestamp) = plan.timestamp else {
                continue;
            };
            #[cfg(test)]
            HISTORY_SELECTION_VISITS.set(HISTORY_SELECTION_VISITS.get().saturating_add(1));
            segment.visit_rows(
                plan.type_id,
                &[timestamp, "datid"],
                0,
                usize::MAX,
                |_ordinal, row| {
                    let (Some(datid), Some(stored)) = (
                        unsigned_cell(row.get("datid")),
                        timestamp_cell(row.get(timestamp)),
                    ) else {
                        return !cancelled();
                    };
                    if datids.contains(&datid) && stored <= to {
                        by_layout
                            .entry((datid, plan.type_id))
                            .or_default()
                            .insert(stored);
                    }
                    !cancelled()
                },
            )?;
        }
    }
    let mut selected = BTreeMap::<(i64, u32), HistoryMoment>::new();
    let mut seeds = BTreeMap::<u32, (i64, HistoryMoment)>::new();
    for ((datid, type_id), moments) in &by_layout {
        let mut previous = None;
        for timestamp in moments {
            let candidate = HistoryMoment {
                type_id: *type_id,
                previous,
            };
            if (from..=to).contains(timestamp) {
                selected
                    .entry((*timestamp, *datid))
                    .and_modify(|chosen| {
                        if candidate.type_id > chosen.type_id {
                            *chosen = candidate;
                        }
                    })
                    .or_insert(candidate);
            } else if *timestamp < from {
                seeds
                    .entry(*datid)
                    .and_modify(|chosen| {
                        if *timestamp > chosen.0
                            || *timestamp == chosen.0 && candidate.type_id > chosen.1.type_id
                        {
                            *chosen = (*timestamp, candidate);
                        }
                    })
                    .or_insert((*timestamp, candidate));
            }
            previous = Some(*timestamp);
        }
    }
    for (datid, (timestamp, moment)) in seeds {
        selected.insert((timestamp, datid), moment);
    }
    Ok(selected)
}

#[expect(
    clippy::too_many_arguments,
    reason = "one scan keeps exact object predecessors, event membership, and source coordinates"
)]
fn scan_tablespace_history_plan(
    segment: &Segment,
    plan: &Plan,
    kind: RelationKind,
    tablespace_oid: u32,
    to: i64,
    datids: &HashSet<u32>,
    selected: &BTreeMap<(i64, u32), HistoryMoment>,
    previous: &mut BTreeMap<(u32, Vec<super::IdentityCell>), HistoryPrevious>,
    contributions: &mut BTreeMap<(i64, u32), Aggregate>,
    event_sources: &mut BTreeMap<(i64, u32), Source>,
    cancelled: &impl Fn() -> bool,
) -> Result<(), ApiError> {
    if plan.timestamp.is_none() {
        return Ok(());
    }
    #[cfg(test)]
    HISTORY_SOURCE_VISITS.set(HISTORY_SOURCE_VISITS.get().saturating_add(1));
    let counters = rate_columns(plan);
    let mut chunk = Vec::with_capacity(super::SNAPSHOT_CHUNK_ROWS);
    let mut failure = None;
    segment.visit_rows(
        plan.type_id,
        &plan.projection,
        0,
        usize::MAX,
        |ordinal, row| {
            chunk.push((ordinal, row));
            if chunk.len() == super::SNAPSHOT_CHUNK_ROWS
                && let Err(error) = process_tablespace_history_chunk(
                    segment,
                    plan,
                    kind,
                    tablespace_oid,
                    to,
                    datids,
                    selected,
                    &counters,
                    previous,
                    contributions,
                    event_sources,
                    &mut chunk,
                )
            {
                failure = Some(error);
                return false;
            }
            !cancelled()
        },
    )?;
    if let Some(error) = failure {
        return Err(error);
    }
    if !cancelled() && !chunk.is_empty() {
        process_tablespace_history_chunk(
            segment,
            plan,
            kind,
            tablespace_oid,
            to,
            datids,
            selected,
            &counters,
            previous,
            contributions,
            event_sources,
            &mut chunk,
        )?;
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "the bounded chunk shares exact predecessor and per-database aggregate state"
)]
fn process_tablespace_history_chunk(
    segment: &Segment,
    plan: &Plan,
    kind: RelationKind,
    tablespace_oid: u32,
    to: i64,
    datids: &HashSet<u32>,
    selected: &BTreeMap<(i64, u32), HistoryMoment>,
    counters: &[&'static str],
    previous: &mut BTreeMap<(u32, Vec<super::IdentityCell>), HistoryPrevious>,
    contributions: &mut BTreeMap<(i64, u32), Aggregate>,
    event_sources: &mut BTreeMap<(i64, u32), Source>,
    chunk: &mut Vec<(u64, Row)>,
) -> Result<(), ApiError> {
    let dictionary = query::chunk_dictionary(segment, chunk)?;
    for (ordinal, row) in chunk.drain(..) {
        let (Some(datid), Some(timestamp), Some(identity)) = (
            unsigned_cell(row.get("datid")),
            plan.timestamp
                .and_then(|name| timestamp_cell(row.get(name))),
            identity_of(plan, &row),
        ) else {
            continue;
        };
        if !datids.contains(&datid) || timestamp > to {
            continue;
        }
        let history_key = (plan.type_id, identity);
        let before = previous.get(&history_key);
        if before.is_some_and(|stored| stored.timestamp >= timestamp) {
            continue;
        }
        let event = (timestamp, datid);
        let moment = selected.get(&event).copied();
        let required_previous = moment
            .filter(|moment| moment.type_id == plan.type_id)
            .and_then(|moment| moment.previous);
        let elapsed = required_previous
            .and_then(|stored| timestamp.checked_sub(stored))
            .filter(|elapsed| *elapsed > 0);
        let exact_before = before.filter(|stored| Some(stored.timestamp) == required_previous);
        if moment.is_some_and(|moment| moment.type_id == plan.type_id) {
            let source = Source {
                segment_id: segment.id(),
                context_index: 0,
                ordinal,
                type_id: plan.type_id,
                timestamp,
            };
            event_sources
                .entry(event)
                .and_modify(|known| *known = (*known).min(source))
                .or_insert(source);
            if unsigned_cell(row.get("tablespace_oid")) == Some(tablespace_oid) {
                let key = GroupKey::Tablespace { tablespace_oid };
                contributions
                    .entry(event)
                    .or_insert_with(|| Aggregate::new(key, source))
                    .add(
                        kind,
                        plan,
                        &row,
                        exact_before.map(|stored| &stored.readings),
                        elapsed,
                        &dictionary,
                        source,
                    )?;
            }
        }
        let readings = counters
            .iter()
            .filter_map(|name| row.get(name).cloned().map(|value| (*name, value)))
            .collect();
        previous.insert(
            history_key,
            HistoryPrevious {
                timestamp,
                readings,
            },
        );
    }
    Ok(())
}

fn history_physical_fields(
    kind: RelationKind,
    group: RelationGroup,
    fields: &[String],
) -> Vec<String> {
    let mut names = std::collections::BTreeSet::new();
    let keys: &[&str] = match group {
        RelationGroup::Database => &["datid", "datname"],
        RelationGroup::Schema => &["datid", "datname", "schemaname"],
        RelationGroup::Tablespace => &["datid", "tablespace_oid", "tablespace"],
        RelationGroup::Object => &[],
    };
    names.extend(keys.iter().copied());
    for field in fields {
        let dependencies: &[&str] = match (kind, field.as_str()) {
            (RelationKind::Tables, "sequential_share_pct") => &["seq_scan", "idx_scan"],
            (RelationKind::Tables, "tuple_throughput") => &["seq_tup_read", "idx_tup_fetch"],
            (RelationKind::Tables, "seq_tuples_per_scan") => &["seq_tup_read", "seq_scan"],
            (RelationKind::Tables, "idx_tuples_per_scan")
            | (RelationKind::Indexes, "fetches_per_scan") => &["idx_tup_fetch", "idx_scan"],
            (
                RelationKind::Tables,
                "dml_total" | "insert_share_pct" | "update_share_pct" | "delete_share_pct",
            ) => &["n_tup_ins", "n_tup_upd", "n_tup_del"],
            (RelationKind::Tables, "dead_pct") => &["n_live_tup", "n_dead_tup"],
            (RelationKind::Tables, "hot_pct") => &["n_tup_hot_upd", "n_tup_upd"],
            (RelationKind::Tables, "new_page_pct") => &["n_tup_newpage_upd", "n_tup_upd"],
            (RelationKind::Tables, "displayed_storage_bytes" | "toast_share_pct") => {
                &["main_fork_bytes", "toast_bytes"]
            }
            (RelationKind::Tables, "toast_dead_pct") => {
                &["toast_bytes", "toast_n_live_tup", "toast_n_dead_tup"]
            }
            (RelationKind::Tables, "heap_buffer_hit_pct") => &["heap_blks_read", "heap_blks_hit"],
            (RelationKind::Tables, "index_buffer_hit_pct")
            | (RelationKind::Indexes, "buffer_hit_pct") => &["idx_blks_read", "idx_blks_hit"],
            (RelationKind::Tables, "toast_buffer_hit_pct") => {
                &["toast_blks_read", "toast_blks_hit"]
            }
            (RelationKind::Tables, "tidx_buffer_hit_pct") => &["tidx_blks_read", "tidx_blks_hit"],
            (RelationKind::Tables, "buffer_hit_pct") => &[
                "heap_blks_read",
                "heap_blks_hit",
                "idx_blks_read",
                "idx_blks_hit",
                "toast_blks_read",
                "toast_blks_hit",
                "tidx_blks_read",
                "tidx_blks_hit",
            ],
            (RelationKind::Tables, "vacuum_mean_ms") => &["total_vacuum_time", "vacuum_count"],
            (RelationKind::Tables, "autovacuum_mean_ms") => {
                &["total_autovacuum_time", "autovacuum_count"]
            }
            (RelationKind::Tables, "analyze_mean_ms") => &["total_analyze_time", "analyze_count"],
            (RelationKind::Tables, "autoanalyze_mean_ms") => {
                &["total_autoanalyze_time", "autoanalyze_count"]
            }
            (RelationKind::Indexes, "tuples_per_scan") => &["idx_tup_read", "idx_scan"],
            (RelationKind::Indexes, "no_scan_count" | "known_scan_count") => &["idx_scan"],
            (RelationKind::Indexes, "state_severity") => &["indisvalid", "indisready"],
            (RelationKind::Indexes, "invalid_count") => &["indisvalid"],
            (RelationKind::Indexes, "unready_count") => &["indisready"],
            (RelationKind::Indexes, "unique_count") => &["indisunique"],
            (RelationKind::Indexes, "primary_count") => &["indisprimary"],
            (RelationKind::Indexes, "exclusion_count") => &["indisexclusion"],
            _ => {
                let timestamp = ["_oldest", "_latest", "_never_count"]
                    .iter()
                    .find_map(|suffix| field.strip_suffix(suffix));
                if let Some(timestamp) = timestamp {
                    names.insert(timestamp);
                    if timestamp == "toast_last_autovacuum" {
                        names.insert("toast_bytes");
                    }
                } else if (kind == RelationKind::Tables
                    && (TABLE_RATES.contains(&field.as_str())
                        || TABLE_GAUGES.contains(&field.as_str())
                        || TABLE_MAXIMA.contains(&field.as_str())))
                    || (kind == RelationKind::Indexes
                        && (INDEX_RATES.contains(&field.as_str())
                            || INDEX_GAUGES.contains(&field.as_str())
                            || INDEX_FLAGS.contains(&field.as_str())))
                {
                    names.insert(field);
                }
                &[]
            }
        };
        names.extend(dependencies.iter().copied());
    }
    names.into_iter().map(ToOwned::to_owned).collect()
}

fn history_datid(group: RelationGroup, filters: &[Filter]) -> Result<u32, ApiError> {
    let required: &[&str] = match group {
        RelationGroup::Database => &["datid"],
        RelationGroup::Schema => &["datid", "schemaname"],
        RelationGroup::Tablespace | RelationGroup::Object => {
            return Err(ApiError::BadFilter("group".to_owned()));
        }
    };
    if filters.len() != required.len()
        || required
            .iter()
            .any(|name| !filters.iter().any(|filter| filter.column == *name))
    {
        return Err(ApiError::BadFilter("where".to_owned()));
    }
    filters
        .iter()
        .find(|filter| filter.column == "datid")
        .and_then(|filter| filter.value.parse().ok())
        .ok_or_else(|| ApiError::BadFilter("datid".to_owned()))
}

fn history_segments(
    reader: &Reader,
    listed: &[SegmentRef],
    logical_name: &str,
    datid: u32,
    from: i64,
    to: i64,
    cancelled: &impl Fn() -> bool,
) -> Result<Vec<SegmentRef>, ApiError> {
    let has_section = |segment: &SegmentRef| {
        segment
            .sections()
            .iter()
            .any(|section| logical_section_name(section.type_id) == Some(logical_name))
    };
    let mut selected = listed
        .iter()
        .filter(|segment| {
            has_section(segment) && segment.max_ts() >= from && segment.min_ts() <= to
        })
        .cloned()
        .collect::<Vec<_>>();
    let type_ids = selected
        .iter()
        .flat_map(SegmentRef::sections)
        .filter(|section| logical_section_name(section.type_id) == Some(logical_name))
        .map(|section| section.type_id)
        .collect::<HashSet<_>>();
    let selected_ids = selected.iter().map(SegmentRef::id).collect::<HashSet<_>>();
    let mut candidates = listed
        .iter()
        .filter(|segment| segment.min_ts() < from && !selected_ids.contains(&segment.id()))
        .filter(|segment| {
            segment
                .sections()
                .iter()
                .any(|section| type_ids.contains(&section.type_id))
        })
        .collect::<Vec<_>>();
    candidates.sort_unstable_by_key(|segment| (segment.max_ts(), segment.id()));
    let mut predecessors = BTreeMap::<u32, (i64, Vec<SegmentRef>)>::new();
    for candidate in candidates.into_iter().rev() {
        if cancelled() {
            break;
        }
        if type_ids.iter().all(|type_id| {
            predecessors
                .get(type_id)
                .is_some_and(|(timestamp, _segments)| candidate.max_ts() < *timestamp)
        }) {
            break;
        }
        let segment = reader.open_segment(candidate)?;
        let carried = candidate
            .sections()
            .iter()
            .map(|section| section.type_id)
            .filter(|type_id| type_ids.contains(type_id))
            .collect::<Vec<_>>();
        for type_id in carried {
            let Some(timestamp) =
                segment_datid_predecessor(&segment, type_id, datid, from, cancelled)?
            else {
                continue;
            };
            match predecessors.entry(type_id) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert((timestamp, vec![(*candidate).clone()]));
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    let (chosen, segments) = entry.get_mut();
                    match timestamp.cmp(chosen) {
                        Ordering::Greater => {
                            *chosen = timestamp;
                            segments.clear();
                            segments.push((*candidate).clone());
                        }
                        Ordering::Equal => segments.push((*candidate).clone()),
                        Ordering::Less => {}
                    }
                }
            }
        }
    }
    selected.extend(
        predecessors
            .into_values()
            .flat_map(|(_timestamp, segments)| segments),
    );
    selected.sort_unstable_by_key(SegmentRef::id);
    selected.dedup_by_key(|segment| segment.id());
    Ok(selected)
}

fn segment_datid_predecessor(
    segment: &Segment,
    type_id: u32,
    datid: u32,
    before: i64,
    cancelled: &impl Fn() -> bool,
) -> Result<Option<i64>, ApiError> {
    if segment.rows_of(type_id).is_none() {
        return Ok(None);
    }
    let Some(timestamp) = contract(type_id).and_then(|layout| {
        layout
            .columns
            .iter()
            .find(|column| column.class == kronika_registry::ColumnClass::Timestamp)
            .map(|column| column.name)
    }) else {
        return Ok(None);
    };
    let mut found = None;
    segment.visit_rows(
        type_id,
        &[timestamp, "datid"],
        0,
        usize::MAX,
        |_ordinal, row| {
            if unsigned_cell(row.get("datid")) == Some(datid)
                && let Some(stored) = timestamp_cell(row.get(timestamp))
                && stored < before
                && found.is_none_or(|chosen| stored > chosen)
            {
                found = Some(stored);
            }
            !cancelled()
        },
    )?;
    Ok(found)
}

fn selected_history_layouts(
    reader: &Reader,
    sources: &[HistorySegment],
    datid: u32,
    from: i64,
    to: i64,
    cancelled: &impl Fn() -> bool,
) -> Result<BTreeMap<i64, HistoryMoment>, ApiError> {
    let mut by_layout = BTreeMap::<u32, std::collections::BTreeSet<i64>>::new();
    for source in sources {
        if cancelled() {
            break;
        }
        let segment = reader.open_segment(&source.segment)?;
        for plan in &source.plans {
            let Some(timestamp) = plan.timestamp else {
                continue;
            };
            #[cfg(test)]
            HISTORY_SELECTION_VISITS.set(HISTORY_SELECTION_VISITS.get().saturating_add(1));
            segment.visit_rows(
                plan.type_id,
                &[timestamp, "datid"],
                0,
                usize::MAX,
                |_ordinal, row| {
                    if cancelled() {
                        return false;
                    }
                    let (Some(stored_datid), Some(stored)) = (
                        unsigned_cell(row.get("datid")),
                        timestamp_cell(row.get(timestamp)),
                    ) else {
                        return true;
                    };
                    if stored_datid == datid && stored <= to {
                        by_layout.entry(plan.type_id).or_default().insert(stored);
                    }
                    true
                },
            )?;
        }
    }
    let mut selected = BTreeMap::<i64, HistoryMoment>::new();
    for (type_id, moments) in &by_layout {
        let mut previous = None;
        for timestamp in moments {
            if (from..=to).contains(timestamp) {
                let candidate = HistoryMoment {
                    type_id: *type_id,
                    previous,
                };
                selected
                    .entry(*timestamp)
                    .and_modify(|chosen| {
                        if candidate.type_id > chosen.type_id {
                            *chosen = candidate;
                        }
                    })
                    .or_insert(candidate);
            }
            previous = Some(*timestamp);
        }
    }
    Ok(selected)
}

#[expect(
    clippy::too_many_arguments,
    reason = "one history scan keeps its exact target, window, and reducer state explicit"
)]
fn scan_history_plan(
    segment: &Segment,
    plan: &Plan,
    kind: RelationKind,
    group: RelationGroup,
    datid: u32,
    from: i64,
    to: i64,
    selected: &BTreeMap<i64, HistoryMoment>,
    previous: &mut BTreeMap<(u32, Vec<super::IdentityCell>), HistoryPrevious>,
    aggregates: &mut BTreeMap<(i64, GroupKey), Aggregate>,
    cancelled: &impl Fn() -> bool,
) -> Result<(), ApiError> {
    if plan.timestamp.is_none() {
        return Ok(());
    }
    let counters = rate_columns(plan);
    let mut chunk = Vec::with_capacity(super::SNAPSHOT_CHUNK_ROWS);
    let mut failure = None;
    #[cfg(test)]
    HISTORY_SOURCE_VISITS.set(HISTORY_SOURCE_VISITS.get().saturating_add(1));
    segment.visit_rows(
        plan.type_id,
        &plan.projection,
        0,
        usize::MAX,
        |ordinal, row| {
            chunk.push((ordinal, row));
            if chunk.len() == super::SNAPSHOT_CHUNK_ROWS
                && let Err(error) = process_history_chunk(
                    segment, plan, kind, group, datid, from, to, selected, &counters, previous,
                    aggregates, &mut chunk,
                )
            {
                failure = Some(error);
                return false;
            }
            !cancelled()
        },
    )?;
    if let Some(error) = failure {
        return Err(error);
    }
    if !cancelled() && !chunk.is_empty() {
        process_history_chunk(
            segment, plan, kind, group, datid, from, to, selected, &counters, previous, aggregates,
            &mut chunk,
        )?;
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "the bounded chunk shares request-scoped predecessor and aggregate state"
)]
fn process_history_chunk(
    segment: &Segment,
    plan: &Plan,
    kind: RelationKind,
    group: RelationGroup,
    datid: u32,
    from: i64,
    to: i64,
    selected: &BTreeMap<i64, HistoryMoment>,
    counters: &[&'static str],
    previous: &mut BTreeMap<(u32, Vec<super::IdentityCell>), HistoryPrevious>,
    aggregates: &mut BTreeMap<(i64, GroupKey), Aggregate>,
    chunk: &mut Vec<(u64, Row)>,
) -> Result<(), ApiError> {
    let dictionary = query::chunk_dictionary(segment, chunk)?;
    for (ordinal, row) in chunk.drain(..) {
        if unsigned_cell(row.get("datid")) != Some(datid) || !plan.matches(&row, &dictionary) {
            continue;
        }
        let (Some(timestamp), Some(identity)) = (
            plan.timestamp
                .and_then(|name| timestamp_cell(row.get(name))),
            identity_of(plan, &row),
        ) else {
            continue;
        };
        let history_key = (plan.type_id, identity);
        let before = previous.get(&history_key);
        if before.is_some_and(|stored| stored.timestamp >= timestamp) {
            continue;
        }
        let moment = selected.get(&timestamp).copied();
        let required_previous = moment
            .filter(|moment| moment.type_id == plan.type_id)
            .and_then(|moment| moment.previous);
        let elapsed = required_previous
            .and_then(|stored| timestamp.checked_sub(stored))
            .filter(|elapsed| *elapsed > 0);
        let exact_before = before.filter(|stored| Some(stored.timestamp) == required_previous);
        if (from..=to).contains(&timestamp)
            && moment.is_some_and(|moment| moment.type_id == plan.type_id)
            && let Some(key) = GroupKey::from_row(kind, group, &row, &dictionary)?
        {
            let source_row = Source {
                segment_id: segment.id(),
                context_index: 0,
                ordinal,
                type_id: plan.type_id,
                timestamp,
            };
            aggregates
                .entry((timestamp, key.clone()))
                .or_insert_with(|| Aggregate::new(key, source_row))
                .add(
                    kind,
                    plan,
                    &row,
                    exact_before.map(|stored| &stored.readings),
                    elapsed,
                    &dictionary,
                    source_row,
                )?;
        }
        let readings = counters
            .iter()
            .filter_map(|name| row.get(name).cloned().map(|value| (*name, value)))
            .collect();
        previous.insert(
            history_key,
            HistoryPrevious {
                timestamp,
                readings,
            },
        );
    }
    Ok(())
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
                segment_id: self.anchor.id(),
                active_position: self.anchor.active_position().unwrap_or(0),
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
    let source_segment = prepared.reader.open_segment(context.source)?;
    let mut offset = 0_u64;
    while offset < context.rows {
        let mut chunk = Vec::new();
        source_segment.visit_rows(
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
        let dictionary = resolved_dictionary(&source_segment, &ids)?;
        for (ordinal, row) in chunk {
            if !context.window.matches(&row)
                || !context.plan.matches(&row, &dictionary)
                || prepared.search.as_ref().is_some_and(|search| {
                    !search_matches(
                        kind.logical_name(),
                        context.plan,
                        &row,
                        &dictionary,
                        None,
                        search,
                    )
                })
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
                segment_id: context.source.id(),
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
            let key = if group == RelationGroup::Tablespace {
                let Some(key) = GroupKey::from_row(kind, group, &row, &dictionary)? else {
                    continue;
                };
                key
            } else {
                object_key.for_group(kind, group)
            };
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
            "segment_id": row.source.segment_id.to_string(),
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
        RelationGroup::Tablespace => "tablespace",
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
        (_, RelationGroup::Tablespace) => &["tablespace_oid"],
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
        GroupKey::Index {
            datid,
            datname: format!("db{datid}"),
            schemaname: "public".to_owned(),
            relid: 10 + datid,
            relname: format!("table{datid}"),
            indexrelid: 20 + datid,
            indexrelname: format!("index{datid}"),
        }
    }

    fn table_key(datid: u32) -> GroupKey {
        GroupKey::Table {
            datid,
            datname: format!("db{datid}"),
            schemaname: "public".to_owned(),
            relid: 10 + datid,
            relname: format!("table{datid}"),
        }
    }

    const fn source(context_index: usize, ordinal: u64) -> Source {
        Source {
            segment_id: 7,
            context_index,
            ordinal,
            type_id: 1_014_004,
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
            search: None,
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
    fn tablespace_oid_filter_requires_a_nonzero_u32() {
        let mut request = SnapshotRequest {
            segment_id: 1,
            at: 2,
            sections: vec![TABLES.to_owned()],
            fields: Vec::new(),
            by: Vec::new(),
            direction: Order::Asc,
            group: Some(RelationGroup::Object),
            page_size: Some(200),
            cursor: None,
            search: None,
            text: None,
            filters: vec![Filter {
                column: "tablespace_oid".to_owned(),
                value: u32::MAX.to_string(),
            }],
            type_id: None,
            row_ordinal: None,
        };
        assert_eq!(split_filters(&request).unwrap().0, request.filters);
        for value in ["0", "4294967296", "not-an-oid"] {
            request.filters[0].value = value.to_owned();
            assert!(matches!(
                split_filters(&request),
                Err(ApiError::BadFilter(name)) if name == "tablespace_oid"
            ));
        }
    }

    #[test]
    fn object_keys_are_minimal_and_display_identity_is_a_value() {
        let table_key = table_key(7).json(RelationKind::Tables, RelationGroup::Object);
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
        assert_eq!(
            first.metric("schemaname").unwrap().json(),
            second.metric("schemaname").unwrap().json()
        );
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
        assert_eq!(
            table
                .iter()
                .find(|field| field.name == "table_count")
                .map(|field| field.unit),
            Some(Some("count"))
        );
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
        assert_eq!(
            RelationKind::Tables.fields(RelationGroup::Tablespace)[0].name,
            "tablespace"
        );

        let index = RelationKind::Indexes.fields(RelationGroup::Database);
        for name in [
            "index_count",
            "last_idx_scan_never_count",
            "no_scan_count",
            "known_scan_count",
            "invalid_count",
            "unready_count",
            "unique_count",
            "primary_count",
            "exclusion_count",
        ] {
            assert_eq!(
                index
                    .iter()
                    .find(|field| field.name == name)
                    .map(|field| field.unit),
                Some(Some("count")),
                "{name}"
            );
        }
        for name in [
            "idx_scan",
            "idx_tup_read",
            "idx_tup_fetch",
            "idx_blks_read",
            "idx_blks_hit",
        ] {
            assert_eq!(
                index
                    .iter()
                    .find(|field| field.name == name)
                    .map(|field| field.unit),
                Some(Some("per_second")),
                "{name}"
            );
        }
        assert!(!index.iter().any(|field| field.name == "indisvalid"));
        assert_eq!(
            RelationKind::Indexes.fields(RelationGroup::Tablespace)[0].name,
            "tablespace"
        );
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
        assert_eq!(object["source"]["segment_id"], "7");
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
    fn table_cuts_recompute_from_exact_operands() {
        let mut aggregate = aggregate();
        for (name, delta) in [
            ("seq_scan", 2),
            ("idx_scan", 6),
            ("seq_tup_read", 20),
            ("idx_tup_fetch", 60),
            ("n_tup_ins", 1),
            ("n_tup_upd", 3),
            ("n_tup_del", 6),
            ("n_tup_hot_upd", 2),
            ("n_tup_newpage_upd", 1),
            ("heap_blks_read", 1),
            ("heap_blks_hit", 9),
            ("idx_blks_read", 2),
            ("idx_blks_hit", 8),
        ] {
            let mut rate = RateAggregate::default();
            add_rate(&mut rate, delta);
            aggregate.rates.insert(name, rate);
        }
        for name in [
            "toast_blks_read",
            "toast_blks_hit",
            "tidx_blks_read",
            "tidx_blks_hit",
        ] {
            aggregate.rates.insert(name, RateAggregate::default());
        }
        for (name, value) in [
            ("main_fork_bytes", 800),
            ("toast_bytes", 200),
            ("toast_n_live_tup", 90),
            ("toast_n_dead_tup", 10),
        ] {
            aggregate
                .gauges
                .insert(name, Availability::Value(OrderedNumber::Integer(value)));
        }

        let table = |name: &'static str| {
            aggregate
                .metric(RelationKind::Tables, RelationGroup::Database, name)
                .expect("available table cut")
                .json()
        };
        assert_eq!(table("tuple_throughput"), json!(80.0));
        assert_eq!(table("dml_total"), json!(10.0));
        assert_eq!(table("insert_share_pct"), json!(10.0));
        assert_eq!(table("update_share_pct"), json!(30.0));
        assert_eq!(table("delete_share_pct"), json!(60.0));
        assert_eq!(table("displayed_storage_bytes"), json!("1000"));
        assert_eq!(table("toast_share_pct"), json!(20.0));
        assert_eq!(table("toast_dead_pct"), json!(10.0));
        let assert_close = |name: &'static str, expected: f64| {
            let actual = table(name).as_f64().expect("numeric table cut");
            assert!((actual - expected).abs() < 1e-12, "{name}: {actual}");
        };
        assert_close("heap_buffer_hit_pct", 90.0);
        assert_close("index_buffer_hit_pct", 80.0);
        assert_close("buffer_hit_pct", 85.0);
        assert!(
            aggregate
                .metric(
                    RelationKind::Tables,
                    RelationGroup::Database,
                    "toast_buffer_hit_pct"
                )
                .is_none()
        );
    }

    #[test]
    fn structural_absence_is_zero_only_inside_valid_sums() {
        let mut stored = aggregate();
        stored.gauges.insert(
            "main_fork_bytes",
            Availability::Value(OrderedNumber::Integer(800)),
        );
        stored.gauges.insert("toast_bytes", Availability::Empty);
        assert_eq!(
            metric_number(stored.metric(
                RelationKind::Tables,
                RelationGroup::Object,
                "displayed_storage_bytes"
            )),
            json!("800")
        );
        assert_eq!(
            metric_number(stored.metric(
                RelationKind::Tables,
                RelationGroup::Object,
                "toast_share_pct"
            )),
            json!(0.0)
        );

        for name in ["heap_blks_read", "heap_blks_hit"] {
            let mut rate = RateAggregate::default();
            add_rate(&mut rate, 0);
            stored.rates.insert(name, rate);
        }
        for name in [
            "idx_blks_read",
            "idx_blks_hit",
            "toast_blks_read",
            "toast_blks_hit",
            "tidx_blks_read",
            "tidx_blks_hit",
        ] {
            stored.rates.insert(name, RateAggregate::default());
        }
        let ratio = stored
            .metric(
                RelationKind::Tables,
                RelationGroup::Object,
                "buffer_hit_pct",
            )
            .expect("exact zero-access ratio");
        assert_eq!(ratio.json(), Value::Null);
        assert!(ratio.order_value().is_none());

        let mut scans = aggregate();
        let mut sequential = RateAggregate::default();
        add_rate(&mut sequential, 4);
        scans.rates.insert("seq_scan", sequential);
        scans.rates.insert("idx_scan", RateAggregate::default());
        assert_eq!(
            metric_number(scans.metric(
                RelationKind::Tables,
                RelationGroup::Object,
                "sequential_share_pct"
            )),
            json!(100.0)
        );

        stored
            .gauges
            .insert("toast_bytes", Availability::Unavailable);
        assert!(
            stored
                .metric(
                    RelationKind::Tables,
                    RelationGroup::Object,
                    "displayed_storage_bytes"
                )
                .is_none()
        );
    }

    #[test]
    fn zero_denominators_are_unavailable_for_table_cuts() {
        let mut aggregate = aggregate();
        for name in [
            "seq_scan",
            "seq_tup_read",
            "idx_scan",
            "idx_tup_fetch",
            "n_tup_ins",
            "n_tup_upd",
            "n_tup_del",
            "n_tup_hot_upd",
            "n_tup_newpage_upd",
            "heap_blks_read",
            "heap_blks_hit",
        ] {
            let mut rate = RateAggregate::default();
            add_rate(&mut rate, 0);
            aggregate.rates.insert(name, rate);
        }
        for name in [
            "n_live_tup",
            "n_dead_tup",
            "main_fork_bytes",
            "toast_bytes",
            "toast_n_live_tup",
            "toast_n_dead_tup",
        ] {
            aggregate
                .gauges
                .insert(name, Availability::Value(OrderedNumber::Integer(0)));
        }

        assert_eq!(
            metric_number(aggregate.metric(
                RelationKind::Tables,
                RelationGroup::Object,
                "tuple_throughput"
            )),
            json!(0.0)
        );
        assert_eq!(
            metric_number(aggregate.metric(
                RelationKind::Tables,
                RelationGroup::Object,
                "dml_total"
            )),
            json!(0.0)
        );
        for name in [
            "sequential_share_pct",
            "seq_tuples_per_scan",
            "idx_tuples_per_scan",
            "insert_share_pct",
            "update_share_pct",
            "delete_share_pct",
            "hot_pct",
            "new_page_pct",
            "dead_pct",
            "toast_share_pct",
            "toast_dead_pct",
            "heap_buffer_hit_pct",
        ] {
            let metric = aggregate
                .metric(RelationKind::Tables, RelationGroup::Object, name)
                .unwrap_or_else(|| panic!("exact zero-denominator metric {name}"));
            assert_eq!(metric.json(), Value::Null, "{name}");
            assert!(metric.order_value().is_none(), "{name}");
        }
    }

    #[test]
    fn last_scan_never_is_distinct_from_layout_absence() {
        let mut aggregate = aggregate();
        let mut never = TimestampAggregate::default();
        never.add(true, Some(&Cell::Null), false);
        aggregate.timestamps.insert("last_seq_scan", never);
        assert!(matches!(
            aggregate.metric(
                RelationKind::Tables,
                RelationGroup::Object,
                "last_seq_scan_never"
            ),
            Some(Metric::Boolean(true))
        ));

        let mut seen = TimestampAggregate::default();
        seen.add(true, Some(&Cell::Ts(30)), false);
        aggregate.timestamps.insert("last_idx_scan", seen);
        assert!(matches!(
            aggregate.metric(
                RelationKind::Tables,
                RelationGroup::Object,
                "last_idx_scan_never"
            ),
            Some(Metric::Boolean(false))
        ));
        assert!(matches!(
            aggregate.metric(
                RelationKind::Indexes,
                RelationGroup::Object,
                "last_idx_scan_never"
            ),
            Some(Metric::Boolean(false))
        ));

        let mut absent = TimestampAggregate::default();
        absent.add(false, None, false);
        aggregate.timestamps.insert("last_seq_scan", absent);
        assert!(
            aggregate
                .metric(
                    RelationKind::Tables,
                    RelationGroup::Object,
                    "last_seq_scan_never"
                )
                .is_none()
        );
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
