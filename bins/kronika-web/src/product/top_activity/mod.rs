//! Semantic registry and typed contract for ranked activity retrieval.

mod execute;
mod registry;
mod result;

use std::fmt;

use serde::{Deserialize, Serialize, Serializer};

pub(crate) use execute::{TopActivityError, execute_top_activity};
pub(crate) use registry::{
    ConversionContext, ConversionContextBuilder, EntityShape, ExecutionRecipe, MetricClass,
    MetricUnit, Ranking, ResolvedMetric, metric_definitions, surface_definitions,
};
pub(crate) use result::{
    Band, Entity, FiniteValue, I64String, Interval, Row, TopActivityResult, U64String,
    ValueDefinition,
};

#[cfg(test)]
mod tests;

const MICROS_PER_HOUR: i64 = 3_600_000_000;
const LAST_MICROSECOND: i64 = MICROS_PER_HOUR - 1;

/// Unvalidated transport arguments for `kronika_find_top_activity`.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawQuery {
    pub(crate) hour: String,
    pub(crate) surface: String,
    #[serde(default, deserialize_with = "present_string")]
    pub(crate) metric: Option<String>,
    #[serde(default, deserialize_with = "present_string")]
    pub(crate) level: Option<String>,
    #[serde(default, deserialize_with = "present_i64")]
    pub(crate) top: Option<i64>,
}

fn present_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    String::deserialize(deserializer).map(Some)
}

fn present_i64<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    i64::deserialize(deserializer).map(Some)
}

/// A normalized, valid top-activity query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Query {
    hour: UtcHour,
    selection: Selection,
    top: Top,
}

impl Query {
    /// Exact requested UTC hour.
    #[must_use]
    pub(crate) const fn hour(self) -> UtcHour {
        self.hour
    }

    /// Valid surface, metric, and grouping selection.
    #[must_use]
    pub(crate) const fn selection(self) -> Selection {
        self.selection
    }

    /// Shipped top-K choice.
    #[must_use]
    pub(crate) const fn top(self) -> Top {
        self.top
    }

    /// Physical execution recipe derived from the semantic selection.
    #[must_use]
    pub(crate) fn recipe(self) -> Option<ExecutionRecipe> {
        ExecutionRecipe::for_selection(self.selection)
    }
}

/// Exact UTC-calendar-hour start in Unix microseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct UtcHour(i64);

impl UtcHour {
    /// Start of the hour, inclusive.
    #[must_use]
    pub(crate) const fn start(self) -> i64 {
        self.0
    }

    /// End of the hour, inclusive.
    #[must_use]
    pub(crate) const fn end(self) -> i64 {
        self.0 + LAST_MICROSECOND
    }
}

/// One of the eight shipped activity ledgers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Surface {
    PostgreSqlStatements,
    PostgreSqlPlans,
    PostgreSqlTables,
    PostgreSqlIndexes,
    Processes,
    PostgreSqlDatabases,
    CgroupCpu,
    CgroupIo,
}

impl Serialize for Surface {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl Surface {
    /// Every shipped semantic surface, in public descriptor order.
    pub(crate) const ALL: [Self; 8] = [
        Self::PostgreSqlStatements,
        Self::PostgreSqlPlans,
        Self::PostgreSqlTables,
        Self::PostgreSqlIndexes,
        Self::Processes,
        Self::PostgreSqlDatabases,
        Self::CgroupCpu,
        Self::CgroupIo,
    ];

    /// Stable public identifier.
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::PostgreSqlStatements => "postgresql_statements",
            Self::PostgreSqlPlans => "postgresql_plans",
            Self::PostgreSqlTables => "postgresql_tables",
            Self::PostgreSqlIndexes => "postgresql_indexes",
            Self::Processes => "processes",
            Self::PostgreSqlDatabases => "postgresql_databases",
            Self::CgroupCpu => "cgroup_cpu",
            Self::CgroupIo => "cgroup_io",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|surface| surface.as_str() == value)
    }
}

impl fmt::Display for Surface {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Stable semantic metric identifier shared by request and result contracts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Metric {
    ExecTime,
    Calls,
    Rows,
    SharedRead,
    SharedDirtied,
    TempWritten,
    WalBytes,
    Writes,
    SeqRead,
    HeapRead,
    DeadTuples,
    AutovacuumTime,
    IdxScan,
    IdxTupRead,
    IdxBlksRead,
    Cpu,
    Rss,
    IoRead,
    IoWrite,
    Majflt,
    RunDelay,
    Commits,
    Rollbacks,
    DbRead,
    TempBytes,
    Deadlocks,
    CgCpu,
    CgThrottled,
    CgRead,
    CgWrite,
    CgRios,
    CgWios,
}

impl Serialize for Metric {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl Metric {
    /// Every distinct metric identifier, in descriptor order.
    #[cfg(test)]
    pub(crate) const ALL: [Self; 32] = [
        Self::ExecTime,
        Self::Calls,
        Self::Rows,
        Self::SharedRead,
        Self::SharedDirtied,
        Self::TempWritten,
        Self::WalBytes,
        Self::Writes,
        Self::SeqRead,
        Self::HeapRead,
        Self::DeadTuples,
        Self::AutovacuumTime,
        Self::IdxScan,
        Self::IdxTupRead,
        Self::IdxBlksRead,
        Self::Cpu,
        Self::Rss,
        Self::IoRead,
        Self::IoWrite,
        Self::Majflt,
        Self::RunDelay,
        Self::Commits,
        Self::Rollbacks,
        Self::DbRead,
        Self::TempBytes,
        Self::Deadlocks,
        Self::CgCpu,
        Self::CgThrottled,
        Self::CgRead,
        Self::CgWrite,
        Self::CgRios,
        Self::CgWios,
    ];

    /// Stable public identifier.
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ExecTime => "exec_time",
            Self::Calls => "calls",
            Self::Rows => "rows",
            Self::SharedRead => "shared_read",
            Self::SharedDirtied => "shared_dirtied",
            Self::TempWritten => "temp_written",
            Self::WalBytes => "wal_bytes",
            Self::Writes => "writes",
            Self::SeqRead => "seq_read",
            Self::HeapRead => "heap_read",
            Self::DeadTuples => "dead_tuples",
            Self::AutovacuumTime => "autovacuum_time",
            Self::IdxScan => "idx_scan",
            Self::IdxTupRead => "idx_tup_read",
            Self::IdxBlksRead => "idx_blks_read",
            Self::Cpu => "cpu",
            Self::Rss => "rss",
            Self::IoRead => "io_read",
            Self::IoWrite => "io_write",
            Self::Majflt => "majflt",
            Self::RunDelay => "run_delay",
            Self::Commits => "commits",
            Self::Rollbacks => "rollbacks",
            Self::DbRead => "db_read",
            Self::TempBytes => "temp_bytes",
            Self::Deadlocks => "deadlocks",
            Self::CgCpu => "cg_cpu",
            Self::CgThrottled => "cg_throttled",
            Self::CgRead => "cg_read",
            Self::CgWrite => "cg_write",
            Self::CgRios => "cg_rios",
            Self::CgWios => "cg_wios",
        }
    }
}

impl fmt::Display for Metric {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Relation aggregation level for table and index surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum RelationLevel {
    Object,
    Schema,
    Database,
    Tablespace,
}

impl Serialize for RelationLevel {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl RelationLevel {
    /// Every shipped relation level, in descriptor order.
    pub(crate) const ALL: [Self; 4] =
        [Self::Object, Self::Schema, Self::Database, Self::Tablespace];

    /// Stable public identifier.
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Object => "object",
            Self::Schema => "schema",
            Self::Database => "database",
            Self::Tablespace => "tablespace",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|level| level.as_str() == value)
    }
}

/// One of the four shipped top-K choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Top {
    Ten,
    TwentyFive,
    Fifty,
    OneHundred,
}

impl Top {
    /// Every shipped choice.
    #[cfg(test)]
    pub(crate) const ALL: [Self; 4] = [Self::Ten, Self::TwentyFive, Self::Fifty, Self::OneHundred];

    /// Numeric top-K limit.
    #[must_use]
    pub(crate) const fn get(self) -> usize {
        match self {
            Self::Ten => 10,
            Self::TwentyFive => 25,
            Self::Fifty => 50,
            Self::OneHundred => 100,
        }
    }

    fn parse(value: i64) -> Option<Self> {
        match value {
            10 => Some(Self::Ten),
            25 => Some(Self::TwentyFive),
            50 => Some(Self::Fifty),
            100 => Some(Self::OneHundred),
            _ => None,
        }
    }
}

/// Valid combination of a surface, its metric, and its optional relation level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Selection {
    surface: Surface,
    metric: Metric,
    level: Option<RelationLevel>,
}

impl Selection {
    /// Selected surface.
    #[must_use]
    pub(crate) const fn surface(self) -> Surface {
        self.surface
    }

    /// Effective metric after default resolution.
    #[must_use]
    pub(crate) const fn metric(self) -> Metric {
        self.metric
    }

    /// Effective relation level, or `None` for non-relation surfaces.
    #[must_use]
    pub(crate) const fn level(self) -> Option<RelationLevel> {
        self.level
    }
}

/// Stable invalid-argument detail from semantic normalization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NormalizeError {
    parameter: &'static str,
    message: String,
}

impl NormalizeError {
    /// Stable client-facing error message.
    #[must_use]
    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    /// Public argument whose value was rejected.
    #[must_use]
    pub(crate) const fn parameter(&self) -> &'static str {
        self.parameter
    }
}

impl fmt::Display for NormalizeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for NormalizeError {}

/// Normalize transport arguments into the sole product query form.
pub(crate) fn normalize(raw: RawQuery) -> Result<Query, NormalizeError> {
    let hour = parse_hour(&raw.hour)?;
    let Some(surface) = Surface::parse(&raw.surface) else {
        return Err(invalid(
            "surface",
            format!("unknown surface {}", raw.surface),
        ));
    };
    let surface_definition = registry::surface_definition(surface);
    let metric_name = raw
        .metric
        .as_deref()
        .unwrap_or(surface_definition.default_metric.as_str());
    let Some(metric_definition) = registry::metric_definition(surface, metric_name) else {
        return Err(invalid(
            "metric",
            format!("metric {metric_name} is not valid for surface {surface}"),
        ));
    };
    let level = normalize_level(surface, raw.level.as_deref())?;
    let top_value = raw.top.unwrap_or(25);
    let Some(top) = Top::parse(top_value) else {
        return Err(invalid(
            "top",
            format!("top {top_value} is not one of 10, 25, 50, 100"),
        ));
    };
    Ok(Query {
        hour,
        selection: Selection {
            surface,
            metric: metric_definition.metric,
            level,
        },
        top,
    })
}

fn parse_hour(value: &str) -> Result<UtcHour, NormalizeError> {
    if !canonical_i64(value) {
        return Err(invalid(
            "hour",
            "hour must be a canonical signed i64 Unix-microsecond value",
        ));
    }
    let Ok(hour) = value.parse::<i64>() else {
        return Err(invalid("hour", "hour must fit signed i64"));
    };
    if hour % MICROS_PER_HOUR != 0 {
        return Err(invalid(
            "hour",
            "hour must be an exact UTC-hour start divisible by 3600000000",
        ));
    }
    if hour.checked_add(LAST_MICROSECOND).is_none() {
        return Err(invalid("hour", "hour end is outside signed i64"));
    }
    Ok(UtcHour(hour))
}

fn canonical_i64(value: &str) -> bool {
    if value == "0" {
        return true;
    }
    let digits = value.strip_prefix('-').unwrap_or(value);
    !digits.is_empty()
        && !digits.starts_with('0')
        && digits.bytes().all(|byte| byte.is_ascii_digit())
}

fn normalize_level(
    surface: Surface,
    level: Option<&str>,
) -> Result<Option<RelationLevel>, NormalizeError> {
    if matches!(
        surface,
        Surface::PostgreSqlTables | Surface::PostgreSqlIndexes
    ) {
        let value = level.unwrap_or("object");
        return RelationLevel::parse(value).map(Some).ok_or_else(|| {
            invalid(
                "level",
                format!("level {value} is not one of object, schema, database, tablespace"),
            )
        });
    }
    if level.is_some() {
        return Err(invalid(
            "level",
            format!("level is not valid for surface {surface}"),
        ));
    }
    Ok(None)
}

fn invalid(parameter: &'static str, message: impl Into<String>) -> NormalizeError {
    NormalizeError {
        parameter,
        message: message.into(),
    }
}
