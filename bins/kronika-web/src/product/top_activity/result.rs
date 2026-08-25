//! Typed structured result returned unchanged by HTTP and MCP transports.

use serde::{Serialize, Serializer};

use super::{Metric, MetricClass, MetricUnit, Ranking, RelationLevel, Surface};

/// Signed i64 serialized as canonical decimal text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct I64String(i64);

impl I64String {
    /// Build from an exact signed value.
    #[must_use]
    pub(crate) const fn new(value: i64) -> Self {
        Self(value)
    }
}

impl Serialize for I64String {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

/// Unsigned u64 serialized as canonical decimal text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct U64String(u64);

impl U64String {
    /// Build from an exact unsigned value.
    #[must_use]
    pub(crate) const fn new(value: u64) -> Self {
        Self(value)
    }
}

impl Serialize for U64String {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

/// A JSON-publishable finite floating value.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub(crate) struct FiniteValue(f64);

impl FiniteValue {
    /// Accept a value only when JSON can represent it as a number.
    #[must_use]
    pub(crate) fn new(value: f64) -> Option<Self> {
        value.is_finite().then_some(Self(value))
    }

    /// Recover the finite value.
    #[must_use]
    #[cfg(test)]
    pub(crate) const fn get(self) -> f64 {
        self.0
    }
}

impl Serialize for FiniteValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_f64(self.0)
    }
}

/// Exact metric semantics effective for the complete result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct ValueDefinition {
    pub(crate) class: MetricClass,
    pub(crate) cell_unit: MetricUnit,
    pub(crate) total_unit: MetricUnit,
    pub(crate) ranking: Ranking,
    pub(crate) metric_description: &'static str,
    pub(crate) cell_formula: &'static str,
    pub(crate) total_formula: &'static str,
}

/// One exact inclusive display interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct Interval {
    pub(crate) start: I64String,
    pub(crate) end: I64String,
}

/// A Total or Other band.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Band {
    pub(crate) total: Option<FiniteValue>,
    pub(crate) cells: Vec<Option<FiniteValue>>,
}

/// Stable semantic entity or relation group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Entity {
    #[serde(rename = "postgresql_statement")]
    PostgreSqlStatement {
        query_id: Option<I64String>,
        role_oid: u32,
        database_oid: u32,
        top_level: Option<bool>,
        database_name: Option<String>,
        role_name: Option<String>,
    },
    #[serde(rename = "postgresql_plan")]
    PostgreSqlPlan {
        role_oid: u32,
        database_oid: u32,
        entry_query_id: I64String,
        plan_id: I64String,
        database_name: Option<String>,
        role_name: Option<String>,
    },
    #[serde(rename = "postgresql_table")]
    PostgreSqlTable {
        database_oid: u32,
        relation_oid: u32,
        database_name: String,
        schema_name: String,
        relation_name: String,
    },
    #[serde(rename = "postgresql_index")]
    PostgreSqlIndex {
        database_oid: u32,
        index_oid: u32,
        database_name: String,
        schema_name: String,
        table_name: String,
        index_name: String,
    },
    ProcessCommand {
        command: String,
    },
    #[serde(rename = "postgresql_database")]
    PostgreSqlDatabase {
        database_oid: u32,
        database_name: Option<String>,
    },
    CgroupCpu {
        path: String,
    },
    #[serde(rename = "cgroup_io_device")]
    CgroupIoDevice {
        path: String,
        major: u32,
        minor: u32,
    },
    #[serde(rename = "postgresql_relation_database")]
    PostgreSqlRelationDatabase {
        database_name: String,
    },
    #[serde(rename = "postgresql_relation_schema")]
    PostgreSqlRelationSchema {
        database_name: String,
        schema_name: String,
    },
    #[serde(rename = "postgresql_tablespace")]
    PostgreSqlTablespace {
        tablespace_name: Option<String>,
    },
}

/// One ranked row with interval cells matching the result interval count.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Row {
    pub(crate) recorded_layout: Option<u32>,
    pub(crate) entity: Entity,
    pub(crate) members: Option<u32>,
    pub(crate) total: Option<FiniteValue>,
    pub(crate) cells: Vec<Option<FiniteValue>>,
}

/// One complete top-K result for the selected hour.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct TopActivityResult {
    pub(crate) hour_start: I64String,
    pub(crate) hour_end: I64String,
    pub(crate) surface: Surface,
    pub(crate) metric: Metric,
    pub(crate) level: Option<RelationLevel>,
    pub(crate) definition: ValueDefinition,
    pub(crate) intervals: Vec<Interval>,
    pub(crate) rows: Vec<Row>,
    pub(crate) totals: Band,
    pub(crate) others: Band,
    pub(crate) entity_count: usize,
    pub(crate) others_count: usize,
    pub(crate) top: usize,
    pub(crate) out_of_order: U64String,
}
