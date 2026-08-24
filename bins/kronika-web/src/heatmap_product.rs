//! Shared Heatmap product surfaces, cuts, groups, and defaults.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::OnceLock;

use kronika_registry::{ColumnClass, logical_section_name, registry};
use serde::{Deserialize, Serialize};

const STORED: &str = include_str!("../product-heatmap.json");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HeatmapPolicy {
    pub(crate) default_top: usize,
    pub(crate) max_top: usize,
    pub(crate) max_columns: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HeatmapSurface {
    pub(crate) id: String,
    pub(crate) default_cut: String,
    pub(crate) default_group: String,
    pub(crate) default_columns: usize,
    pub(crate) groups: Vec<HeatmapGroup>,
    pub(crate) cuts: Vec<HeatmapCut>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HeatmapGroup {
    pub(crate) id: String,
    pub(crate) fields: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HeatmapCut {
    pub(crate) id: String,
    pub(crate) section: String,
    pub(crate) fields: Vec<String>,
    pub(crate) labels: Vec<String>,
    pub(crate) raw_unit: HeatmapUnit,
    pub(crate) conversion: HeatmapConversion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum HeatmapUnit {
    Blocks,
    Bytes,
    ClockTicks,
    Count,
    Kibibytes,
    Microseconds,
    Milliseconds,
    Nanoseconds,
    Seconds,
}

impl HeatmapUnit {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Blocks => "blocks",
            Self::Bytes => "bytes",
            Self::ClockTicks => "clock_ticks",
            Self::Count => "count",
            Self::Kibibytes => "kibibytes",
            Self::Microseconds => "microseconds",
            Self::Milliseconds => "milliseconds",
            Self::Nanoseconds => "nanoseconds",
            Self::Seconds => "seconds",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum HeatmapConversion {
    Identity,
    FixedMultiply {
        factor: u64,
        target_unit: HeatmapUnit,
    },
    RecordedMultiply {
        locator: String,
        target_unit: HeatmapUnit,
    },
    RecordedDivide {
        locator: String,
        target_unit: HeatmapUnit,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HeatmapProductError {
    Registry(String),
    Surface,
    Cut,
    Group,
    Columns,
}

impl HeatmapProductError {
    pub(crate) const fn parameter(&self) -> Option<&'static str> {
        match self {
            Self::Registry(_) => None,
            Self::Surface => Some("surface"),
            Self::Cut => Some("cut"),
            Self::Group => Some("group"),
            Self::Columns => Some("columns"),
        }
    }
}

impl fmt::Display for HeatmapProductError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registry(message) => formatter.write_str(message),
            Self::Surface => formatter.write_str("unknown Heatmap surface"),
            Self::Cut => formatter.write_str("unknown Heatmap cut"),
            Self::Group => formatter.write_str("unknown Heatmap group"),
            Self::Columns => formatter.write_str("Heatmap columns are outside the bounded range"),
        }
    }
}

impl std::error::Error for HeatmapProductError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredRegistry {
    version: u32,
    policy: HeatmapPolicy,
    surfaces: Vec<HeatmapSurface>,
}

struct HeatmapRegistry {
    policy: HeatmapPolicy,
    surfaces: Box<[HeatmapSurface]>,
}

pub(crate) struct HeatmapSelection<'a> {
    pub(crate) surface: &'a HeatmapSurface,
    pub(crate) cut: &'a HeatmapCut,
    pub(crate) group: &'a HeatmapGroup,
    pub(crate) columns: usize,
}

impl HeatmapSelection<'_> {
    pub(crate) fn request(
        &self,
        from: i64,
        to: i64,
        top: usize,
        type_id: Option<u32>,
    ) -> crate::route::HeatmapRequest {
        crate::route::HeatmapRequest {
            from,
            to,
            section: self.cut.section.clone(),
            fields: self.cut.fields.clone(),
            columns: self.columns,
            top,
            labels: if self.group.fields.is_empty() {
                self.cut.labels.clone()
            } else {
                Vec::new()
            },
            group: self.group.fields.clone(),
            type_id,
        }
    }
}

static PRODUCT: OnceLock<Result<HeatmapRegistry, HeatmapProductError>> = OnceLock::new();

pub(crate) fn policy() -> Result<&'static HeatmapPolicy, HeatmapProductError> {
    product().map(|stored| &stored.policy)
}

pub(crate) fn surfaces() -> Result<&'static [HeatmapSurface], HeatmapProductError> {
    product().map(|stored| stored.surfaces.as_ref())
}

pub(crate) fn resolve(
    surface: &str,
    cut: Option<&str>,
    group: Option<&str>,
    columns: Option<usize>,
) -> Result<HeatmapSelection<'static>, HeatmapProductError> {
    let product = product()?;
    let surface = product
        .surfaces
        .iter()
        .find(|candidate| candidate.id == surface)
        .ok_or(HeatmapProductError::Surface)?;
    let cut_name = cut.unwrap_or(&surface.default_cut);
    let cut = surface
        .cuts
        .iter()
        .find(|candidate| candidate.id == cut_name)
        .ok_or(HeatmapProductError::Cut)?;
    let group_name = group.unwrap_or(&surface.default_group);
    let group = surface
        .groups
        .iter()
        .find(|candidate| candidate.id == group_name)
        .ok_or(HeatmapProductError::Group)?;
    let columns = columns.unwrap_or(surface.default_columns);
    if columns == 0 || columns > product.policy.max_columns {
        return Err(HeatmapProductError::Columns);
    }
    Ok(HeatmapSelection {
        surface,
        cut,
        group,
        columns,
    })
}

fn product() -> Result<&'static HeatmapRegistry, HeatmapProductError> {
    match PRODUCT.get_or_init(load) {
        Ok(product) => Ok(product),
        Err(error) => Err(error.clone()),
    }
}

fn load() -> Result<HeatmapRegistry, HeatmapProductError> {
    let stored: StoredRegistry = serde_json::from_str(STORED).map_err(|error| {
        HeatmapProductError::Registry(format!("parse Heatmap product registry: {error}"))
    })?;
    if stored.version != 1 {
        return Err(HeatmapProductError::Registry(format!(
            "unsupported Heatmap product registry version {}",
            stored.version
        )));
    }
    if stored.policy.default_top == 0
        || stored.policy.default_top > stored.policy.max_top
        || stored.policy.max_columns == 0
    {
        return Err(HeatmapProductError::Registry(
            "invalid Heatmap product limits".to_owned(),
        ));
    }
    validate_surfaces(&stored.policy, &stored.surfaces)?;
    Ok(HeatmapRegistry {
        policy: stored.policy,
        surfaces: stored.surfaces.into_boxed_slice(),
    })
}

fn validate_surfaces(
    policy: &HeatmapPolicy,
    surfaces: &[HeatmapSurface],
) -> Result<(), HeatmapProductError> {
    let mut surface_ids = BTreeSet::new();
    for surface in surfaces {
        if surface.id.is_empty() || !surface_ids.insert(surface.id.as_str()) {
            return Err(invalid("surface", &surface.id));
        }
        if surface.default_columns == 0 || surface.default_columns > policy.max_columns {
            return Err(invalid("default columns for surface", &surface.id));
        }
        validate_named_members("group", &surface.id, &surface.groups, |group| {
            (&group.id, group.fields.as_slice())
        })?;
        validate_named_members("cut", &surface.id, &surface.cuts, |cut| {
            (&cut.id, cut.fields.as_slice())
        })?;
        if !surface
            .groups
            .iter()
            .any(|group| group.id == surface.default_group)
        {
            return Err(invalid("default group for surface", &surface.id));
        }
        if !surface.cuts.iter().any(|cut| cut.id == surface.default_cut) {
            return Err(invalid("default cut for surface", &surface.id));
        }
        for cut in &surface.cuts {
            validate_cut(surface, cut)?;
        }
    }
    if surfaces.is_empty() {
        return Err(HeatmapProductError::Registry(
            "Heatmap product registry has no surfaces".to_owned(),
        ));
    }
    Ok(())
}

fn validate_named_members<T>(
    kind: &str,
    surface: &str,
    values: &[T],
    parts: impl Fn(&T) -> (&String, &[String]),
) -> Result<(), HeatmapProductError> {
    let mut ids = BTreeSet::new();
    for value in values {
        let (id, fields) = parts(value);
        if id.is_empty() || !ids.insert(id.as_str()) || fields.len() > 8 {
            return Err(invalid(kind, surface));
        }
        let mut names = BTreeSet::new();
        if fields
            .iter()
            .any(|field| field.is_empty() || !names.insert(field))
        {
            return Err(invalid(kind, surface));
        }
    }
    if values.is_empty() {
        return Err(invalid(kind, surface));
    }
    Ok(())
}

fn validate_cut(surface: &HeatmapSurface, cut: &HeatmapCut) -> Result<(), HeatmapProductError> {
    if cut.fields.is_empty() || cut.fields.len() > 4 || cut.labels.len() > 8 {
        return Err(invalid("cut fields for surface", &surface.id));
    }
    let contracts = registry()
        .iter()
        .filter(|contract| logical_section_name(contract.type_id.get()) == Some(&cut.section))
        .collect::<Vec<_>>();
    if contracts.is_empty() {
        return Err(invalid("cut section for surface", &surface.id));
    }
    let mut class = None;
    for field in &cut.fields {
        let found = contracts
            .iter()
            .filter_map(|contract| contract.column(field))
            .next();
        let Some(column) = found else {
            return Err(invalid("cut field for surface", &surface.id));
        };
        if !matches!(column.class, ColumnClass::Cumulative | ColumnClass::Gauge)
            || class.is_some_and(|stored| stored != column.class)
        {
            return Err(invalid("cut class for surface", &surface.id));
        }
        class = Some(column.class);
    }
    for name in cut
        .labels
        .iter()
        .chain(surface.groups.iter().flat_map(|group| group.fields.iter()))
    {
        if !contracts
            .iter()
            .any(|contract| contract.column(name).is_some())
        {
            return Err(invalid("projection for surface", &surface.id));
        }
    }
    Ok(())
}

fn invalid(kind: &str, value: &str) -> HeatmapProductError {
    HeatmapProductError::Registry(format!("invalid Heatmap {kind} {value}"))
}

#[cfg(test)]
#[path = "heatmap_product/tests.rs"]
mod tests;
