//! One typed top-activity producer shared unchanged by HTTP and MCP.

use std::fmt;
use std::path::Path;

use kronika_registry::contract;
use serde_json::Value;

use super::{
    Band, Entity, EntityShape, FiniteValue, I64String, Interval, Query, Row, TopActivityResult,
    U64String,
};
use crate::api::ApiError;
use crate::api::heatmap::{RawBand, RawTopError, RawTopIdentity, RawTopRow, collect_top_activity};
use crate::product::execution::{Execution, ExecutionStop};
use crate::product::page::SHARED_RESULT_MAX_BYTES;

/// Stable failure from the complete semantic producer.
#[derive(Debug)]
pub(crate) enum TopActivityError {
    Read(ApiError),
    ResultTooLarge,
    Cancelled,
    DeadlineExceeded,
}

impl TopActivityError {
    /// Stable tool-level error code.
    #[must_use]
    pub(crate) const fn code(&self) -> &'static str {
        match self {
            Self::Read(_) => "heatmap_read_failed",
            Self::ResultTooLarge => "result_too_large",
            Self::Cancelled => "cancelled",
            Self::DeadlineExceeded => "deadline_exceeded",
        }
    }

    /// Compact caller-facing message.
    #[must_use]
    pub(crate) const fn message(&self) -> &'static str {
        match self {
            Self::Read(_) => "recorded top activity could not be read",
            Self::ResultTooLarge => {
                "the complete top-activity result exceeds the shared output ceiling"
            }
            Self::Cancelled => "top-activity retrieval was cancelled",
            Self::DeadlineExceeded => "top-activity retrieval exceeded its deadline",
        }
    }
}

impl fmt::Display for TopActivityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for TopActivityError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read(error) => Some(error),
            Self::ResultTooLarge | Self::Cancelled | Self::DeadlineExceeded => None,
        }
    }
}

impl From<ExecutionStop> for TopActivityError {
    fn from(stop: ExecutionStop) -> Self {
        match stop {
            ExecutionStop::Cancelled => Self::Cancelled,
            ExecutionStop::DeadlineExceeded => Self::DeadlineExceeded,
        }
    }
}

impl From<RawTopError> for TopActivityError {
    fn from(error: RawTopError) -> Self {
        match error {
            RawTopError::Read(error) => Self::Read(error),
            RawTopError::Stopped(stop) => stop.into(),
        }
    }
}

/// Execute one normalized query and return its sole typed product result.
pub(crate) fn execute_top_activity(
    root: &Path,
    query: Query,
    execution: &Execution,
) -> Result<TopActivityResult, TopActivityError> {
    execution.checkpoint()?;
    let recipe = query
        .recipe()
        .ok_or(TopActivityError::Read(ApiError::NoSuchSection))?;
    let raw = collect_top_activity(root, query, execution)?;
    let resolved = recipe.resolve(raw.conversion);
    let rows = raw
        .rows
        .into_iter()
        .map(|row| semantic_row(recipe.entity, row, resolved))
        .collect::<Result<Vec<_>, _>>()?;
    let result = TopActivityResult {
        hour_start: I64String::new(query.hour().start()),
        hour_end: I64String::new(query.hour().end()),
        surface: query.selection().surface(),
        metric: query.selection().metric(),
        level: query.selection().level(),
        definition: resolved.definition,
        intervals: raw
            .intervals
            .into_iter()
            .map(|(start, end)| Interval {
                start: I64String::new(start),
                end: I64String::new(end),
            })
            .collect(),
        totals: semantic_band(raw.totals, resolved)?,
        others: semantic_band(raw.others, resolved)?,
        entity_count: raw.entity_count,
        others_count: raw.entity_count.saturating_sub(rows.len()),
        top: rows.len(),
        rows,
        out_of_order: U64String::new(raw.out_of_order),
    };
    execution.checkpoint()?;
    let bytes = serde_json::to_vec(&result)
        .map_err(|error| TopActivityError::Read(ApiError::from(error)))?;
    if bytes.len() > SHARED_RESULT_MAX_BYTES {
        return Err(TopActivityError::ResultTooLarge);
    }
    Ok(result)
}

fn semantic_row(
    shape: EntityShape,
    raw: RawTopRow,
    resolved: super::ResolvedMetric,
) -> Result<Row, TopActivityError> {
    let (recorded_layout, entity, members) = match raw.identity {
        RawTopIdentity::Ungrouped {
            type_id,
            identity,
            labels,
        } => (
            Some(type_id),
            ungrouped_entity(shape, type_id, &identity, &labels)?,
            None,
        ),
        RawTopIdentity::Group { values, members } => {
            (None, grouped_entity(shape, &values)?, Some(members))
        }
    };
    Ok(Row {
        recorded_layout,
        entity,
        members,
        total: finite_scaled(resolved, raw.total)?,
        cells: raw
            .cells
            .into_iter()
            .map(|value| finite_scaled(resolved, value))
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn semantic_band(raw: RawBand, resolved: super::ResolvedMetric) -> Result<Band, TopActivityError> {
    Ok(Band {
        total: finite_scaled(resolved, raw.total)?,
        cells: raw
            .cells
            .into_iter()
            .map(|value| finite_scaled(resolved, value))
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn finite_scaled(
    resolved: super::ResolvedMetric,
    raw: Option<f64>,
) -> Result<Option<FiniteValue>, TopActivityError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let scaled = resolved
        .scale(raw)
        .map_err(|_error| invalid_numeric_result())?;
    FiniteValue::new(scaled)
        .map(Some)
        .ok_or_else(invalid_numeric_result)
}

fn ungrouped_entity(
    shape: EntityShape,
    type_id: u32,
    identity: &[Value],
    labels: &[Value],
) -> Result<Entity, TopActivityError> {
    let layout = contract(type_id).ok_or_else(invalid_recorded_shape)?;
    let identity = |name: &str| {
        layout
            .identity
            .iter()
            .position(|candidate| *candidate == name)
            .and_then(|index| identity.get(index))
    };
    match shape {
        EntityShape::Statement => Ok(Entity::PostgreSqlStatement {
            query_id: nullable_i64(identity("queryid"))?,
            role_oid: required_u32(identity("userid"))?,
            database_oid: required_u32(identity("dbid"))?,
            top_level: nullable_bool(identity("toplevel"))?,
            database_name: nullable_text(labels.first())?,
            role_name: nullable_text(labels.get(1))?,
        }),
        EntityShape::Plan => Ok(Entity::PostgreSqlPlan {
            role_oid: required_u32(identity("userid"))?,
            database_oid: required_u32(identity("dbid"))?,
            entry_query_id: required_i64(identity("queryid"))?,
            plan_id: required_i64(identity("planid"))?,
            database_name: nullable_text(labels.first())?,
            role_name: nullable_text(labels.get(1))?,
        }),
        EntityShape::Table => Ok(Entity::PostgreSqlTable {
            database_oid: required_u32(identity("datid"))?,
            relation_oid: required_u32(identity("relid"))?,
            database_name: required_text(labels.first())?,
            schema_name: required_text(labels.get(1))?,
            relation_name: required_text(labels.get(2))?,
        }),
        EntityShape::Index => Ok(Entity::PostgreSqlIndex {
            database_oid: required_u32(identity("datid"))?,
            index_oid: required_u32(identity("indexrelid"))?,
            database_name: required_text(labels.first())?,
            schema_name: required_text(labels.get(1))?,
            table_name: required_text(labels.get(2))?,
            index_name: required_text(labels.get(3))?,
        }),
        EntityShape::Database => Ok(Entity::PostgreSqlDatabase {
            database_oid: required_u32(identity("datid"))?,
            database_name: nullable_text(labels.first())?,
        }),
        EntityShape::CgroupCpu => Ok(Entity::CgroupCpu {
            path: required_text(identity("cgroup_path"))?,
        }),
        EntityShape::CgroupIo => Ok(Entity::CgroupIoDevice {
            path: required_text(identity("cgroup_path"))?,
            major: required_u32(identity("major"))?,
            minor: required_u32(identity("minor"))?,
        }),
        EntityShape::ProcessCommand
        | EntityShape::RelationSchema
        | EntityShape::RelationDatabase
        | EntityShape::Tablespace => Err(invalid_recorded_shape()),
    }
}

fn grouped_entity(shape: EntityShape, values: &[Value]) -> Result<Entity, TopActivityError> {
    match shape {
        EntityShape::ProcessCommand => Ok(Entity::ProcessCommand {
            command: required_text(values.first())?,
        }),
        EntityShape::RelationSchema => Ok(Entity::PostgreSqlRelationSchema {
            database_name: required_text(values.first())?,
            schema_name: required_text(values.get(1))?,
        }),
        EntityShape::RelationDatabase => Ok(Entity::PostgreSqlRelationDatabase {
            database_name: required_text(values.first())?,
        }),
        EntityShape::Tablespace => Ok(Entity::PostgreSqlTablespace {
            tablespace_name: nullable_text(values.first())?,
        }),
        EntityShape::Statement
        | EntityShape::Plan
        | EntityShape::Table
        | EntityShape::Index
        | EntityShape::Database
        | EntityShape::CgroupCpu
        | EntityShape::CgroupIo => Err(invalid_recorded_shape()),
    }
}

fn required_u32(value: Option<&Value>) -> Result<u32, TopActivityError> {
    value
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(invalid_recorded_shape)
}

fn required_i64(value: Option<&Value>) -> Result<I64String, TopActivityError> {
    value
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<i64>().ok())
        .map(I64String::new)
        .ok_or_else(invalid_recorded_shape)
}

fn nullable_i64(value: Option<&Value>) -> Result<Option<I64String>, TopActivityError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => required_i64(Some(value)).map(Some),
    }
}

fn nullable_bool(value: Option<&Value>) -> Result<Option<bool>, TopActivityError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(invalid_recorded_shape()),
    }
}

fn required_text(value: Option<&Value>) -> Result<String, TopActivityError> {
    value
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(invalid_recorded_shape)
}

fn nullable_text(value: Option<&Value>) -> Result<Option<String>, TopActivityError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(invalid_recorded_shape()),
    }
}

fn invalid_recorded_shape() -> TopActivityError {
    TopActivityError::Read(ApiError::Unreadable(Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "recorded top-activity identity does not match its registry layout",
    ))))
}

fn invalid_numeric_result() -> TopActivityError {
    TopActivityError::Read(ApiError::Unreadable(Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "recorded top-activity arithmetic produced a non-finite value",
    ))))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Entity, EntityShape, grouped_entity};

    #[test]
    fn nullable_tablespace_is_a_stable_distinct_group() {
        let entity = grouped_entity(EntityShape::Tablespace, &[json!(null)])
            .expect("nullable tablespace group");
        assert_eq!(
            entity,
            Entity::PostgreSqlTablespace {
                tablespace_name: None,
            }
        );
    }
}
