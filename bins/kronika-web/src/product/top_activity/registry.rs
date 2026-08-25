//! Layout-aware semantic-to-physical activity registry.

use std::fmt;

#[cfg(test)]
use kronika_registry::{TypeContract, logical_section_name};
use serde::Serialize;

use super::{Metric, RelationLevel, Selection, Surface, UtcHour, ValueDefinition};

const NO_FIELDS: &[&str] = &[];
const STATEMENT_LABELS: &[&str] = &["datname", "usename"];
const TABLE_LABELS: &[&str] = &["datname", "schemaname", "relname"];
const INDEX_LABELS: &[&str] = &["datname", "schemaname", "relname", "indexrelname"];
const DATABASE_LABELS: &[&str] = &["datname"];
const PROCESS_GROUP: &[&str] = &["comm"];
const SCHEMA_GROUP: &[&str] = &["datname", "schemaname"];
const DATABASE_GROUP: &[&str] = &["datname"];
const TABLESPACE_GROUP: &[&str] = &["tablespace"];

/// Public counter/gauge calculation family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MetricClass {
    Cumulative,
    Gauge,
}

/// Public unit for every non-null cell or whole-hour value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MetricUnit {
    Count,
    CountPerSecond,
    Bytes,
    BytesPerSecond,
    Milliseconds,
    MillisecondsPerSecond,
    Seconds,
    SecondsPerSecond,
    Microseconds,
    MicrosecondsPerSecond,
    Nanoseconds,
    NanosecondsPerSecond,
}

/// Fixed descending whole-hour ranking formula.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Ranking {
    WholeWindowDeltaDesc,
    WholeWindowMaxDesc,
    SumMemberWindowDeltaDesc,
    SumMemberWindowMaxDesc,
}

/// Conversion metadata selected once from the pinned source view.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ConversionContext {
    block_size: Option<u128>,
    clock_ticks_per_sec: Option<i64>,
}

impl ConversionContext {
    /// Latest usable PostgreSQL block size at or before the selected hour end.
    #[must_use]
    #[cfg(test)]
    pub(crate) const fn block_size(self) -> Option<u128> {
        self.block_size
    }

    /// Latest usable OS clock rate at or before the selected hour end.
    #[must_use]
    #[cfg(test)]
    pub(crate) const fn clock_ticks_per_sec(self) -> Option<i64> {
        self.clock_ticks_per_sec
    }
}

/// Order-independent selector for one hour's conversion metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConversionContextBuilder {
    hour_end: i64,
    block_size: Option<(i64, u128)>,
    clock_ticks_per_sec: Option<(i64, i64)>,
}

impl ConversionContextBuilder {
    /// Start selection for an exact product hour.
    #[must_use]
    pub(crate) const fn new(hour: UtcHour) -> Self {
        Self {
            hour_end: hour.end(),
            block_size: None,
            clock_ticks_per_sec: None,
        }
    }

    /// Consider one recorded PostgreSQL block-size value.
    pub(crate) fn observe_block_size(&mut self, at: i64, value: Option<u128>) {
        let Some(value) = value.filter(|value| *value > 0) else {
            return;
        };
        observe_latest(&mut self.block_size, self.hour_end, at, value);
    }

    /// Consider one recorded OS clock-rate value.
    pub(crate) fn observe_clock_ticks_per_sec(&mut self, at: i64, value: Option<i64>) {
        let Some(value) = value.filter(|value| *value > 0) else {
            return;
        };
        observe_latest(&mut self.clock_ticks_per_sec, self.hour_end, at, value);
    }

    /// Complete the immutable conversion context used by the full result.
    #[must_use]
    pub(crate) const fn finish(self) -> ConversionContext {
        ConversionContext {
            block_size: match self.block_size {
                Some((_at, value)) => Some(value),
                None => None,
            },
            clock_ticks_per_sec: match self.clock_ticks_per_sec {
                Some((_at, value)) => Some(value),
                None => None,
            },
        }
    }
}

fn observe_latest<T: Copy>(slot: &mut Option<(i64, T)>, hour_end: i64, at: i64, value: T) {
    if at > hour_end || slot.is_some_and(|(stored_at, _value)| stored_at > at) {
        return;
    }
    *slot = Some((at, value));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Conversion {
    None,
    BlockSize,
    ClockTicks,
    Kibibytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolvedConversion {
    Native,
    BlockSize,
    RawBlocks,
    ClockTicks,
    RawTicks,
    Kibibytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntityShape {
    Statement,
    Plan,
    Table,
    Index,
    ProcessCommand,
    Database,
    CgroupCpu,
    CgroupIo,
    RelationSchema,
    RelationDatabase,
    Tablespace,
}

/// One public surface definition and its fixed display shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SurfaceDefinition {
    pub(crate) surface: Surface,
    pub(crate) default_metric: Metric,
    pub(crate) intervals: usize,
    pub(crate) description: &'static str,
    section: &'static str,
    labels: &'static [&'static str],
    entity: EntityShape,
}

/// One valid surface/metric pair and its physical projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MetricDefinition {
    pub(crate) surface: Surface,
    pub(crate) metric: Metric,
    pub(crate) fields: &'static [&'static str],
    pub(crate) class: MetricClass,
    conversion: Conversion,
    cell_unit: MetricUnit,
    total_unit: MetricUnit,
    pub(crate) description: &'static str,
}

/// Complete internal recipe for one normalized semantic selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExecutionRecipe {
    pub(crate) surface: Surface,
    pub(crate) metric: Metric,
    pub(crate) level: Option<RelationLevel>,
    pub(crate) section: &'static str,
    pub(crate) fields: &'static [&'static str],
    pub(crate) labels: &'static [&'static str],
    pub(crate) groups: &'static [&'static str],
    pub(crate) intervals: usize,
    pub(crate) class: MetricClass,
    conversion: Conversion,
    cell_unit: MetricUnit,
    total_unit: MetricUnit,
    pub(crate) metric_description: &'static str,
    pub(crate) entity: EntityShape,
}

impl ExecutionRecipe {
    /// Resolve one validated semantic selection to physical registry names.
    #[must_use]
    pub(crate) fn for_selection(selection: Selection) -> Option<Self> {
        let surface = surface_definition(selection.surface());
        let metric = metric_definition_by_id(selection.surface(), selection.metric())?;
        let (labels, groups, entity) = shape(surface, selection.level());
        Some(Self {
            surface: selection.surface(),
            metric: selection.metric(),
            level: selection.level(),
            section: surface.section,
            fields: metric.fields,
            labels,
            groups,
            intervals: surface.intervals,
            class: metric.class,
            conversion: metric.conversion,
            cell_unit: metric.cell_unit,
            total_unit: metric.total_unit,
            metric_description: metric.description,
            entity,
        })
    }

    /// Whether this physical registry layout can supply any selected value.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn supports_layout(self, contract: &TypeContract) -> bool {
        logical_section_name(contract.type_id.get()) == Some(self.section)
            && self
                .fields
                .iter()
                .any(|field| contract.column(field).is_some())
    }

    /// Resolve scale, units, ranking, and formulas from one immutable context.
    #[must_use]
    pub(crate) fn resolve(self, context: ConversionContext) -> ResolvedMetric {
        let (scale, cell_unit, total_unit, resolved_conversion) = match self.conversion {
            Conversion::None => (
                Scale::IDENTITY,
                self.cell_unit,
                self.total_unit,
                ResolvedConversion::Native,
            ),
            Conversion::Kibibytes => (
                Scale::multiply(1_024),
                self.cell_unit,
                self.total_unit,
                ResolvedConversion::Kibibytes,
            ),
            Conversion::BlockSize => match context.block_size {
                Some(block_size) => (
                    Scale::multiply(block_size),
                    self.cell_unit,
                    self.total_unit,
                    ResolvedConversion::BlockSize,
                ),
                None => (
                    Scale::IDENTITY,
                    MetricUnit::CountPerSecond,
                    MetricUnit::Count,
                    ResolvedConversion::RawBlocks,
                ),
            },
            Conversion::ClockTicks => match context.clock_ticks_per_sec {
                Some(clock_ticks) => (
                    Scale::divide(clock_ticks as u64),
                    self.cell_unit,
                    self.total_unit,
                    ResolvedConversion::ClockTicks,
                ),
                None => (
                    Scale::IDENTITY,
                    MetricUnit::CountPerSecond,
                    MetricUnit::Count,
                    ResolvedConversion::RawTicks,
                ),
            },
        };
        ResolvedMetric {
            scale,
            definition: ValueDefinition {
                class: self.class,
                cell_unit,
                total_unit,
                ranking: ranking(self.class, !self.groups.is_empty()),
                metric_description: self.metric_description,
                cell_formula: cell_formula(
                    self.class,
                    !self.groups.is_empty(),
                    resolved_conversion,
                ),
                total_formula: total_formula(
                    self.class,
                    !self.groups.is_empty(),
                    resolved_conversion,
                ),
            },
        }
    }

    /// Stable semantic entity shape produced by this selection.
    #[must_use]
    #[cfg(test)]
    pub(crate) const fn entity_kind(self) -> &'static str {
        match self.entity {
            EntityShape::Statement => "postgresql_statement",
            EntityShape::Plan => "postgresql_plan",
            EntityShape::Table => "postgresql_table",
            EntityShape::Index => "postgresql_index",
            EntityShape::ProcessCommand => "process_command",
            EntityShape::Database => "postgresql_database",
            EntityShape::CgroupCpu => "cgroup_cpu",
            EntityShape::CgroupIo => "cgroup_io_device",
            EntityShape::RelationSchema => "postgresql_relation_schema",
            EntityShape::RelationDatabase => "postgresql_relation_database",
            EntityShape::Tablespace => "postgresql_tablespace",
        }
    }
}

/// Metric scale and its exact public definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResolvedMetric {
    scale: Scale,
    pub(crate) definition: ValueDefinition,
}

/// A stored or converted numeric value cannot be published as finite JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NonFiniteValue;

impl fmt::Display for NonFiniteValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("top-activity numeric value is not finite")
    }
}

impl std::error::Error for NonFiniteValue {}

impl ResolvedMetric {
    /// Apply the selected metadata conversion to one finite raw value.
    pub(crate) fn scale(self, value: f64) -> Result<f64, NonFiniteValue> {
        self.scale.apply(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Scale {
    numerator: u128,
    denominator: u64,
}

impl Scale {
    const IDENTITY: Self = Self {
        numerator: 1,
        denominator: 1,
    };

    const fn multiply(value: u128) -> Self {
        Self {
            numerator: value,
            denominator: 1,
        }
    }

    const fn divide(value: u64) -> Self {
        Self {
            numerator: 1,
            denominator: value,
        }
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "recorded integer conversion constants scale floating product values"
    )]
    fn apply(self, value: f64) -> Result<f64, NonFiniteValue> {
        let scaled = value * self.numerator as f64 / self.denominator as f64;
        if scaled.is_finite() {
            Ok(scaled)
        } else {
            Err(NonFiniteValue)
        }
    }
}

fn cell_formula(class: MetricClass, grouped: bool, conversion: ResolvedConversion) -> &'static str {
    match (class, grouped, conversion) {
        (MetricClass::Cumulative, false, ResolvedConversion::Native) => {
            "Nonnegative endpoint delta divided by positive observed seconds; null without two usable endpoints."
        }
        (MetricClass::Cumulative, true, ResolvedConversion::Native) => {
            "Sum of usable member nonnegative endpoint deltas, each divided by that member's positive observed seconds; null when no member contributes a usable rate."
        }
        (MetricClass::Gauge, false, ResolvedConversion::Native) => {
            "The last usable reading assigned to the interval; null without a usable reading."
        }
        (MetricClass::Gauge, true, ResolvedConversion::Native) => {
            "Sum of each member's last usable reading assigned to the interval; null when no member contributes a usable reading."
        }
        (MetricClass::Gauge, false, ResolvedConversion::Kibibytes) => {
            "The last usable reading assigned to the interval, multiplied by 1024; null without a usable reading."
        }
        (MetricClass::Gauge, true, ResolvedConversion::Kibibytes) => {
            "Sum of each member's last usable reading assigned to the interval, each multiplied by 1024; null when no member contributes a usable reading."
        }
        (MetricClass::Cumulative, false, ResolvedConversion::BlockSize) => {
            "Nonnegative endpoint block delta multiplied by the latest usable recorded block size at or before hour_end, divided by positive observed seconds; null without two usable endpoints."
        }
        (MetricClass::Cumulative, true, ResolvedConversion::BlockSize) => {
            "Sum of usable member nonnegative endpoint block deltas, each multiplied by the latest usable recorded block size at or before hour_end and divided by that member's positive observed seconds; null when no member contributes a usable rate."
        }
        (MetricClass::Cumulative, false, ResolvedConversion::RawBlocks) => {
            "Raw nonnegative endpoint block delta divided by positive observed seconds; null without two usable endpoints."
        }
        (MetricClass::Cumulative, true, ResolvedConversion::RawBlocks) => {
            "Sum of usable member raw nonnegative endpoint block deltas, each divided by that member's positive observed seconds; null when no member contributes a usable rate."
        }
        (MetricClass::Cumulative, false, ResolvedConversion::ClockTicks) => {
            "Nonnegative endpoint tick delta divided by the latest usable recorded clock rate at or before hour_end and positive observed seconds; null without two usable endpoints."
        }
        (MetricClass::Cumulative, true, ResolvedConversion::ClockTicks) => {
            "Sum of usable member nonnegative endpoint tick deltas, each divided by the latest usable recorded clock rate at or before hour_end and that member's positive observed seconds; null when no member contributes a usable rate."
        }
        (MetricClass::Cumulative, false, ResolvedConversion::RawTicks) => {
            "Raw nonnegative endpoint tick delta divided by positive observed seconds; null without two usable endpoints."
        }
        (MetricClass::Cumulative, true, ResolvedConversion::RawTicks) => {
            "Sum of usable member raw nonnegative endpoint tick deltas, each divided by that member's positive observed seconds; null when no member contributes a usable rate."
        }
        (MetricClass::Cumulative, _, ResolvedConversion::Kibibytes)
        | (MetricClass::Gauge, _, ResolvedConversion::BlockSize)
        | (MetricClass::Gauge, _, ResolvedConversion::RawBlocks)
        | (MetricClass::Gauge, _, ResolvedConversion::ClockTicks)
        | (MetricClass::Gauge, _, ResolvedConversion::RawTicks) => {
            unreachable!("metric registry pairs class and conversion")
        }
    }
}

fn ranking(class: MetricClass, grouped: bool) -> Ranking {
    match (class, grouped) {
        (MetricClass::Cumulative, false) => Ranking::WholeWindowDeltaDesc,
        (MetricClass::Gauge, false) => Ranking::WholeWindowMaxDesc,
        (MetricClass::Cumulative, true) => Ranking::SumMemberWindowDeltaDesc,
        (MetricClass::Gauge, true) => Ranking::SumMemberWindowMaxDesc,
    }
}

fn total_formula(
    class: MetricClass,
    grouped: bool,
    conversion: ResolvedConversion,
) -> &'static str {
    match (class, grouped, conversion) {
        (MetricClass::Cumulative, false, ResolvedConversion::BlockSize) => {
            "The nonnegative whole-hour endpoint block delta multiplied by the latest usable recorded block size at or before hour_end; band totals sum contributing entity values."
        }
        (MetricClass::Cumulative, true, ResolvedConversion::BlockSize) => {
            "The sum of member nonnegative whole-hour endpoint block deltas multiplied by the latest usable recorded block size at or before hour_end; band totals sum contributing groups."
        }
        (MetricClass::Cumulative, false, ResolvedConversion::RawBlocks) => {
            "The raw nonnegative whole-hour endpoint block delta; band totals sum contributing entity values."
        }
        (MetricClass::Cumulative, true, ResolvedConversion::RawBlocks) => {
            "The sum of member raw nonnegative whole-hour endpoint block deltas; band totals sum contributing groups."
        }
        (MetricClass::Cumulative, false, ResolvedConversion::ClockTicks) => {
            "The nonnegative whole-hour endpoint tick delta divided by the latest usable recorded clock rate at or before hour_end; band totals sum contributing entity values."
        }
        (MetricClass::Cumulative, true, ResolvedConversion::ClockTicks) => {
            "The sum of member nonnegative whole-hour endpoint tick deltas divided by the latest usable recorded clock rate at or before hour_end; band totals sum contributing groups."
        }
        (MetricClass::Cumulative, false, ResolvedConversion::RawTicks) => {
            "The raw nonnegative whole-hour endpoint tick delta; band totals sum contributing entity values."
        }
        (MetricClass::Cumulative, true, ResolvedConversion::RawTicks) => {
            "The sum of member raw nonnegative whole-hour endpoint tick deltas; band totals sum contributing groups."
        }
        (MetricClass::Gauge, false, ResolvedConversion::Kibibytes) => {
            "The maximum usable reading in the hour multiplied by 1024; the Total band is the peak of its summed interval strip."
        }
        (MetricClass::Gauge, true, ResolvedConversion::Kibibytes) => {
            "The sum of each member's maximum usable reading multiplied by 1024; band totals are peaks of their summed interval strips."
        }
        (MetricClass::Cumulative, false, ResolvedConversion::Native) => {
            "The nonnegative whole-hour endpoint delta; band totals sum contributing entity deltas."
        }
        (MetricClass::Cumulative, true, ResolvedConversion::Native) => {
            "The sum of member nonnegative whole-hour endpoint deltas; band totals sum contributing groups."
        }
        (MetricClass::Gauge, false, ResolvedConversion::Native) => {
            "The maximum usable reading in the hour; the Total band is the peak of its summed interval strip."
        }
        (MetricClass::Gauge, true, ResolvedConversion::Native) => {
            "The sum of each member's maximum usable reading in the hour; band totals are peaks of their summed interval strips."
        }
        (MetricClass::Gauge, _, ResolvedConversion::BlockSize | ResolvedConversion::RawBlocks)
        | (MetricClass::Gauge, _, ResolvedConversion::ClockTicks | ResolvedConversion::RawTicks)
        | (MetricClass::Cumulative, _, ResolvedConversion::Kibibytes) => {
            "The whole-hour value after applying the selected metric conversion."
        }
    }
}

fn shape(
    surface: &SurfaceDefinition,
    level: Option<RelationLevel>,
) -> (
    &'static [&'static str],
    &'static [&'static str],
    EntityShape,
) {
    match level {
        Some(RelationLevel::Object) | None => {
            if surface.surface == Surface::Processes {
                (NO_FIELDS, PROCESS_GROUP, EntityShape::ProcessCommand)
            } else {
                (surface.labels, NO_FIELDS, surface.entity)
            }
        }
        Some(RelationLevel::Schema) => (NO_FIELDS, SCHEMA_GROUP, EntityShape::RelationSchema),
        Some(RelationLevel::Database) => (NO_FIELDS, DATABASE_GROUP, EntityShape::RelationDatabase),
        Some(RelationLevel::Tablespace) => (NO_FIELDS, TABLESPACE_GROUP, EntityShape::Tablespace),
    }
}

/// The eight shipped surface definitions.
#[must_use]
pub(crate) const fn surface_definitions() -> &'static [SurfaceDefinition] {
    &SURFACES
}

pub(super) fn surface_definition(surface: Surface) -> &'static SurfaceDefinition {
    match surface {
        Surface::PostgreSqlStatements => &SURFACES[0],
        Surface::PostgreSqlPlans => &SURFACES[1],
        Surface::PostgreSqlTables => &SURFACES[2],
        Surface::PostgreSqlIndexes => &SURFACES[3],
        Surface::Processes => &SURFACES[4],
        Surface::PostgreSqlDatabases => &SURFACES[5],
        Surface::CgroupCpu => &SURFACES[6],
        Surface::CgroupIo => &SURFACES[7],
    }
}

/// The 37 shipped surface/metric definitions.
#[must_use]
pub(crate) const fn metric_definitions() -> &'static [MetricDefinition] {
    &METRICS
}

pub(super) fn metric_definition(
    surface: Surface,
    metric: &str,
) -> Option<&'static MetricDefinition> {
    METRICS
        .iter()
        .find(|definition| definition.surface == surface && definition.metric.as_str() == metric)
}

fn metric_definition_by_id(surface: Surface, metric: Metric) -> Option<&'static MetricDefinition> {
    METRICS
        .iter()
        .find(|definition| definition.surface == surface && definition.metric == metric)
}

const SURFACES: [SurfaceDefinition; 8] = [
    SurfaceDefinition {
        surface: Surface::PostgreSqlStatements,
        default_metric: Metric::ExecTime,
        intervals: 60,
        description: "Ranks recorded pg_stat_statements identities in 60 exact one-minute intervals; default metric exec_time.",
        section: "pg_stat_statements",
        labels: STATEMENT_LABELS,
        entity: EntityShape::Statement,
    },
    SurfaceDefinition {
        surface: Surface::PostgreSqlPlans,
        default_metric: Metric::ExecTime,
        intervals: 60,
        description: "Ranks recorded pg_store_plans identities in 60 exact one-minute intervals; default metric exec_time.",
        section: "pg_store_plans",
        labels: STATEMENT_LABELS,
        entity: EntityShape::Plan,
    },
    SurfaceDefinition {
        surface: Surface::PostgreSqlTables,
        default_metric: Metric::Writes,
        intervals: 12,
        description: "Ranks recorded user tables or selected relation groups in 12 exact five-minute intervals; default metric writes.",
        section: "pg_stat_user_tables",
        labels: TABLE_LABELS,
        entity: EntityShape::Table,
    },
    SurfaceDefinition {
        surface: Surface::PostgreSqlIndexes,
        default_metric: Metric::IdxScan,
        intervals: 12,
        description: "Ranks recorded user indexes or selected relation groups in 12 exact five-minute intervals; default metric idx_scan.",
        section: "pg_stat_user_indexes",
        labels: INDEX_LABELS,
        entity: EntityShape::Index,
    },
    SurfaceDefinition {
        surface: Surface::Processes,
        default_metric: Metric::Cpu,
        intervals: 60,
        description: "Ranks recorded process commands, combining all recorded PIDs of each command, in 60 exact one-minute intervals; default metric cpu.",
        section: "os_process",
        labels: NO_FIELDS,
        entity: EntityShape::ProcessCommand,
    },
    SurfaceDefinition {
        surface: Surface::PostgreSqlDatabases,
        default_metric: Metric::Commits,
        intervals: 60,
        description: "Ranks recorded PostgreSQL database identities in 60 exact one-minute intervals; default metric commits.",
        section: "pg_stat_database",
        labels: DATABASE_LABELS,
        entity: EntityShape::Database,
    },
    SurfaceDefinition {
        surface: Surface::CgroupCpu,
        default_metric: Metric::CgCpu,
        intervals: 60,
        description: "Ranks recorded cgroup paths by CPU activity in 60 exact one-minute intervals; default metric cg_cpu.",
        section: "os_cgroup_cpu",
        labels: NO_FIELDS,
        entity: EntityShape::CgroupCpu,
    },
    SurfaceDefinition {
        surface: Surface::CgroupIo,
        default_metric: Metric::CgRead,
        intervals: 60,
        description: "Ranks recorded cgroup and block-device identities by I/O activity in 60 exact one-minute intervals; default metric cg_read.",
        section: "os_cgroup_io",
        labels: NO_FIELDS,
        entity: EntityShape::CgroupIo,
    },
];

macro_rules! metric {
    ($surface:ident, $metric:ident, [$($field:literal),+], $class:ident, $conversion:ident, $cell:ident, $total:ident, $description:literal) => {
        MetricDefinition {
            surface: Surface::$surface,
            metric: Metric::$metric,
            fields: &[$($field),+],
            class: MetricClass::$class,
            conversion: Conversion::$conversion,
            cell_unit: MetricUnit::$cell,
            total_unit: MetricUnit::$total,
            description: $description,
        }
    };
}

const METRICS: [MetricDefinition; 37] = [
    metric!(
        PostgreSqlStatements,
        ExecTime,
        ["total_exec_time"],
        Cumulative,
        None,
        MillisecondsPerSecond,
        Milliseconds,
        "Accumulated statement execution time."
    ),
    metric!(
        PostgreSqlStatements,
        Calls,
        ["calls"],
        Cumulative,
        None,
        CountPerSecond,
        Count,
        "Statement executions."
    ),
    metric!(
        PostgreSqlStatements,
        Rows,
        ["rows"],
        Cumulative,
        None,
        CountPerSecond,
        Count,
        "Rows returned or affected by statements."
    ),
    metric!(
        PostgreSqlStatements,
        SharedRead,
        ["shared_blks_read"],
        Cumulative,
        BlockSize,
        BytesPerSecond,
        Bytes,
        "Blocks read outside PostgreSQL shared buffers; the OS page cache may serve them."
    ),
    metric!(
        PostgreSqlStatements,
        SharedDirtied,
        ["shared_blks_dirtied"],
        Cumulative,
        BlockSize,
        BytesPerSecond,
        Bytes,
        "Blocks marked dirty in PostgreSQL shared buffers."
    ),
    metric!(
        PostgreSqlStatements,
        TempWritten,
        ["temp_blks_written"],
        Cumulative,
        BlockSize,
        BytesPerSecond,
        Bytes,
        "Temporary blocks written by statements."
    ),
    metric!(
        PostgreSqlStatements,
        WalBytes,
        ["wal_bytes"],
        Cumulative,
        None,
        BytesPerSecond,
        Bytes,
        "WAL bytes generated by statements."
    ),
    metric!(
        PostgreSqlPlans,
        ExecTime,
        ["total_time"],
        Cumulative,
        None,
        MillisecondsPerSecond,
        Milliseconds,
        "Accumulated plan execution time."
    ),
    metric!(
        PostgreSqlPlans,
        Calls,
        ["calls"],
        Cumulative,
        None,
        CountPerSecond,
        Count,
        "Plan executions."
    ),
    metric!(
        PostgreSqlPlans,
        Rows,
        ["rows"],
        Cumulative,
        None,
        CountPerSecond,
        Count,
        "Rows returned or affected by plan executions."
    ),
    metric!(
        PostgreSqlPlans,
        SharedRead,
        ["shared_blks_read"],
        Cumulative,
        BlockSize,
        BytesPerSecond,
        Bytes,
        "Blocks read outside PostgreSQL shared buffers by plans; the OS page cache may serve them."
    ),
    metric!(
        PostgreSqlPlans,
        TempWritten,
        ["temp_blks_written"],
        Cumulative,
        BlockSize,
        BytesPerSecond,
        Bytes,
        "Temporary blocks written by plans."
    ),
    metric!(
        PostgreSqlTables,
        Writes,
        ["n_tup_ins", "n_tup_upd", "n_tup_del"],
        Cumulative,
        None,
        CountPerSecond,
        Count,
        "Inserted, updated, and deleted rows summed for each recorded table."
    ),
    metric!(
        PostgreSqlTables,
        SeqRead,
        ["seq_tup_read"],
        Cumulative,
        None,
        CountPerSecond,
        Count,
        "Rows read by sequential scans."
    ),
    metric!(
        PostgreSqlTables,
        HeapRead,
        ["heap_blks_read"],
        Cumulative,
        BlockSize,
        BytesPerSecond,
        Bytes,
        "Table-heap blocks read outside PostgreSQL shared buffers; the OS page cache may serve them."
    ),
    metric!(
        PostgreSqlTables,
        DeadTuples,
        ["n_dead_tup"],
        Gauge,
        None,
        Count,
        Count,
        "Estimated dead tuples."
    ),
    metric!(
        PostgreSqlTables,
        AutovacuumTime,
        ["total_autovacuum_time"],
        Cumulative,
        None,
        MillisecondsPerSecond,
        Milliseconds,
        "Accumulated autovacuum time where the recorded layout supplies it."
    ),
    metric!(
        PostgreSqlIndexes,
        IdxScan,
        ["idx_scan"],
        Cumulative,
        None,
        CountPerSecond,
        Count,
        "Index scans."
    ),
    metric!(
        PostgreSqlIndexes,
        IdxTupRead,
        ["idx_tup_read"],
        Cumulative,
        None,
        CountPerSecond,
        Count,
        "Index entries returned by scans."
    ),
    metric!(
        PostgreSqlIndexes,
        IdxBlksRead,
        ["idx_blks_read"],
        Cumulative,
        BlockSize,
        BytesPerSecond,
        Bytes,
        "Index-file blocks read outside PostgreSQL shared buffers; the OS page cache may serve them."
    ),
    metric!(
        Processes,
        Cpu,
        ["utime", "stime"],
        Cumulative,
        ClockTicks,
        SecondsPerSecond,
        Seconds,
        "Recorded user plus system CPU time across every PID in the command group."
    ),
    metric!(
        Processes,
        Rss,
        ["rmem_kb"],
        Gauge,
        Kibibytes,
        Bytes,
        Bytes,
        "Resident memory summed across PIDs in the command group."
    ),
    metric!(
        Processes,
        IoRead,
        ["read_bytes"],
        Cumulative,
        None,
        BytesPerSecond,
        Bytes,
        "Per-process block-layer bytes read; page-cache reads are excluded."
    ),
    metric!(
        Processes,
        IoWrite,
        ["write_bytes"],
        Cumulative,
        None,
        BytesPerSecond,
        Bytes,
        "Per-process block-layer bytes written."
    ),
    metric!(
        Processes,
        Majflt,
        ["majflt"],
        Cumulative,
        None,
        CountPerSecond,
        Count,
        "Major page faults across PIDs in the command group."
    ),
    metric!(
        Processes,
        RunDelay,
        ["rundelay_ns"],
        Cumulative,
        None,
        NanosecondsPerSecond,
        Nanoseconds,
        "Scheduler run delay across PIDs in the command group."
    ),
    metric!(
        PostgreSqlDatabases,
        Commits,
        ["xact_commit"],
        Cumulative,
        None,
        CountPerSecond,
        Count,
        "Committed transactions."
    ),
    metric!(
        PostgreSqlDatabases,
        Rollbacks,
        ["xact_rollback"],
        Cumulative,
        None,
        CountPerSecond,
        Count,
        "Rolled-back transactions."
    ),
    metric!(
        PostgreSqlDatabases,
        DbRead,
        ["blks_read"],
        Cumulative,
        BlockSize,
        BytesPerSecond,
        Bytes,
        "Database blocks read outside PostgreSQL shared buffers; the OS page cache may serve them."
    ),
    metric!(
        PostgreSqlDatabases,
        TempBytes,
        ["temp_bytes"],
        Cumulative,
        None,
        BytesPerSecond,
        Bytes,
        "Bytes written to temporary files."
    ),
    metric!(
        PostgreSqlDatabases,
        Deadlocks,
        ["deadlocks"],
        Cumulative,
        None,
        CountPerSecond,
        Count,
        "Detected PostgreSQL deadlocks."
    ),
    metric!(
        CgroupCpu,
        CgCpu,
        ["usage_usec"],
        Cumulative,
        None,
        MicrosecondsPerSecond,
        Microseconds,
        "Accumulated cgroup CPU time."
    ),
    metric!(
        CgroupCpu,
        CgThrottled,
        ["throttled_usec"],
        Cumulative,
        None,
        MicrosecondsPerSecond,
        Microseconds,
        "Accumulated cgroup CPU-quota throttling time."
    ),
    metric!(
        CgroupIo,
        CgRead,
        ["rbytes"],
        Cumulative,
        None,
        BytesPerSecond,
        Bytes,
        "Block-layer bytes read by the cgroup and device identity."
    ),
    metric!(
        CgroupIo,
        CgWrite,
        ["wbytes"],
        Cumulative,
        None,
        BytesPerSecond,
        Bytes,
        "Block-layer bytes written by the cgroup and device identity."
    ),
    metric!(
        CgroupIo,
        CgRios,
        ["rios"],
        Cumulative,
        None,
        CountPerSecond,
        Count,
        "Block-layer read operations."
    ),
    metric!(
        CgroupIo,
        CgWios,
        ["wios"],
        Cumulative,
        None,
        CountPerSecond,
        Count,
        "Block-layer write operations."
    ),
];
