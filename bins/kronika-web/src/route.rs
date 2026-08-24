//! Strict parsing of the four resource families.

const DEFAULT_PAGE_SIZE: usize = 100;
const MAX_PAGE_SIZE: usize = 1_000;
const DEFAULT_SNAPSHOT_PAGE_SIZE: usize = 200;
const MAX_QUERY_BYTES: usize = 64 * 1024;
const MAX_SECTION_BYTES: usize = 128;
const MAX_SNAPSHOT_SECTIONS: usize = 16;
const MAX_SNAPSHOT_PAGE_SIZE: usize = 5_000;
const MAX_SEARCH_EXPRESSION_CHARS: usize = 1_024;
const MAX_FIELDS: usize = 256;
const DEFAULT_HEATMAP_COLUMNS: usize = 60;
const MAX_HEATMAP_COLUMNS: usize = 1_440;
const DEFAULT_HEATMAP_TOP: usize = 25;
const MAX_HEATMAP_TOP: usize = 500;
const MAX_HEATMAP_LABELS: usize = 8;
const MAX_HEATMAP_FIELDS: usize = 4;
const MAX_HEATMAP_GROUP: usize = 4;
const MAX_FILTERS: usize = 64;
const MAX_ORDER_FIELDS: usize = 16;

/// The requests the server answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Route {
    /// Actual finished/current segment catalog.
    Catalog(Window),
    Hour(HourRequest),
    /// One logical indexed series in one explicit segment.
    Index(SegmentRequest),
    /// Projected full-resolution history in one explicit segment.
    History(DataRequest),
    /// One stable page of physical rows in one explicit segment.
    Rows(RowsRequest),
    Snapshot(Box<SnapshotRequest>),
    /// The ranked top view of one section over one window.
    Heatmap(HeatmapRequest),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HeatmapRequest {
    pub(crate) from: i64,
    pub(crate) to: i64,
    pub(crate) section: String,
    pub(crate) fields: Vec<String>,
    pub(crate) columns: usize,
    pub(crate) top: usize,
    pub(crate) labels: Vec<String>,
    pub(crate) group: Vec<String>,
    pub(crate) type_id: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SnapshotRequest {
    pub(crate) segment_id: i64,
    /// Exact active WAL prefix captured by an earlier pass in the same read.
    pub(crate) active_position: Option<u64>,
    pub(crate) at: i64,
    pub(crate) sections: Vec<String>,
    pub(crate) fields: Vec<String>,
    pub(crate) by: Vec<String>,
    pub(crate) direction: Order,
    pub(crate) group: Option<RelationGroup>,
    /// A typed PostgreSQL product surface resolved by the shared Rust registry.
    pub(crate) postgresql: Option<PostgresqlSurfaceRequest>,
    /// A typed Process product surface resolved by the shared Rust registry.
    pub(crate) process: Option<ProcessSurfaceRequest>,
    /// Present only for the paged single-section form.
    pub(crate) page_size: Option<usize>,
    pub(crate) cursor: Option<String>,
    pub(crate) search: Option<String>,
    /// Dedicated bounded Statement query-text lookup.
    pub(crate) first_match: bool,
    pub(crate) text: Option<usize>,
    pub(crate) filters: Vec<Filter>,
    pub(crate) activity_visibility: Option<ActivityVisibility>,
    pub(crate) type_id: Option<u32>,
    /// Requires `type_id` and addresses a finished source row.
    pub(crate) row_ordinal: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcessSurfaceRequest {
    pub(crate) lens: ProcessLens,
    pub(crate) order: Option<String>,
    pub(crate) direction: Option<Order>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProcessLens {
    Generic,
    Cpu,
    Memory,
    Disk,
    Tree,
}

impl ProcessLens {
    pub(crate) fn parse(lens: Option<&str>) -> Option<Self> {
        match lens {
            None | Some("tree") => Some(Self::Tree),
            Some("generic") => Some(Self::Generic),
            Some("cpu") => Some(Self::Cpu),
            Some("memory") => Some(Self::Memory),
            Some("disk") => Some(Self::Disk),
            Some(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PostgresqlSurfaceRequest {
    pub(crate) surface: PostgresqlSurface,
    pub(crate) order: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PostgresqlSurface {
    Activity,
    Statements(StatementLens),
    Plans(PlanLens),
    Databases,
    Tables(TableLens),
    Indexes(IndexLens),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatementLens {
    Load,
    PerCall,
    Io,
    Resources,
    Stability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanLens {
    Load,
    Timing,
    Io,
    Identity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TableLens {
    Access,
    Changes,
    Maintenance,
    SizeBuffers,
    Freeze,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IndexLens {
    Usage,
    LowActivity,
    SizeBuffers,
    State,
}

impl PostgresqlSurface {
    pub(crate) fn parse(section: &str, lens: Option<&str>) -> Option<Self> {
        match (section, lens) {
            ("pg_stat_activity", None) => Some(Self::Activity),
            ("pg_stat_statements", None | Some("load")) => {
                Some(Self::Statements(StatementLens::Load))
            }
            ("pg_stat_statements", Some("per_call")) => {
                Some(Self::Statements(StatementLens::PerCall))
            }
            ("pg_stat_statements", Some("io")) => Some(Self::Statements(StatementLens::Io)),
            ("pg_stat_statements", Some("resources")) => {
                Some(Self::Statements(StatementLens::Resources))
            }
            ("pg_stat_statements", Some("stability")) => {
                Some(Self::Statements(StatementLens::Stability))
            }
            ("pg_store_plans", None | Some("load")) => Some(Self::Plans(PlanLens::Load)),
            ("pg_store_plans", Some("timing")) => Some(Self::Plans(PlanLens::Timing)),
            ("pg_store_plans", Some("io")) => Some(Self::Plans(PlanLens::Io)),
            ("pg_store_plans", Some("identity")) => Some(Self::Plans(PlanLens::Identity)),
            ("pg_stat_database", None) => Some(Self::Databases),
            ("pg_stat_user_tables", None | Some("access")) => Some(Self::Tables(TableLens::Access)),
            ("pg_stat_user_tables", Some("changes")) => Some(Self::Tables(TableLens::Changes)),
            ("pg_stat_user_tables", Some("maintenance")) => {
                Some(Self::Tables(TableLens::Maintenance))
            }
            ("pg_stat_user_tables", Some("size_buffers")) => {
                Some(Self::Tables(TableLens::SizeBuffers))
            }
            ("pg_stat_user_tables", Some("freeze")) => Some(Self::Tables(TableLens::Freeze)),
            ("pg_stat_user_indexes", None | Some("usage")) => Some(Self::Indexes(IndexLens::Usage)),
            ("pg_stat_user_indexes", Some("low_activity")) => {
                Some(Self::Indexes(IndexLens::LowActivity))
            }
            ("pg_stat_user_indexes", Some("size_buffers")) => {
                Some(Self::Indexes(IndexLens::SizeBuffers))
            }
            ("pg_stat_user_indexes", Some("state")) => Some(Self::Indexes(IndexLens::State)),
            _ => None,
        }
    }

    pub(crate) const fn section(self) -> &'static str {
        match self {
            Self::Activity => "pg_stat_activity",
            Self::Statements(_) => "pg_stat_statements",
            Self::Plans(_) => "pg_store_plans",
            Self::Databases => "pg_stat_database",
            Self::Tables(_) => "pg_stat_user_tables",
            Self::Indexes(_) => "pg_stat_user_indexes",
        }
    }

    pub(crate) const fn lens_error(section: &str) -> &'static str {
        match section.as_bytes() {
            b"pg_stat_statements" => "lens must be load, per_call, io, resources, or stability",
            b"pg_store_plans" => "lens must be load, timing, io, or identity",
            b"pg_stat_user_tables" => {
                "lens must be access, changes, maintenance, size_buffers, or freeze"
            }
            b"pg_stat_user_indexes" => "lens must be usage, low_activity, size_buffers, or state",
            _ => "lens is not supported by this surface",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HourRequest {
    pub(crate) window: Window,
    /// Exact active segment and WAL prefix captured by an earlier pass.
    pub(crate) active_segment: Option<(i64, u64)>,
    pub(crate) series: Option<SeriesRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SeriesRequest {
    pub(crate) section: String,
    pub(crate) fields: Vec<String>,
    pub(crate) filters: Vec<Filter>,
    pub(crate) type_id: Option<u32>,
    pub(crate) group: Option<RelationGroup>,
}

/// Optional inclusive timestamp bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Window {
    pub(crate) from: Option<i64>,
    pub(crate) to: Option<i64>,
}

impl Window {
    pub(crate) fn contains(self, timestamp: i64) -> bool {
        self.from.is_none_or(|from| timestamp >= from) && self.to.is_none_or(|to| timestamp <= to)
    }
}

/// One logical section in one explicit segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SegmentRequest {
    pub(crate) segment_id: i64,
    pub(crate) section: String,
}

/// One exact typed equality predicate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Filter {
    pub(crate) column: String,
    pub(crate) value: String,
}

/// Committed prefix of one active segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ActiveCursor {
    pub(crate) segment_id: i64,
    pub(crate) wal_position: u64,
}

/// Projection and predicates shared by history and rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DataRequest {
    pub(crate) segment: SegmentRequest,
    pub(crate) fields: Vec<String>,
    pub(crate) filters: Vec<Filter>,
    pub(crate) type_id: Option<u32>,
    pub(crate) after: Option<ActiveCursor>,
}

/// Physical row ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Order {
    Asc,
    Desc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RelationGroup {
    Database,
    Schema,
    Tablespace,
    Object,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ActivityVisibility {
    pub(crate) include_idle: bool,
    pub(crate) include_system: bool,
}

/// Bounded page request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RowsRequest {
    pub(crate) data: DataRequest,
    pub(crate) order: Order,
    pub(crate) page_size: usize,
    pub(crate) cursor: Option<String>,
}

/// Why a request was refused before touching disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RouteError {
    /// No resource has this path shape.
    NoSuchPath,
    /// One named path/query field is absent, duplicated, or invalid.
    BadParameter(String),
}

impl std::fmt::Display for RouteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchPath => write!(f, "no such path"),
            Self::BadParameter(name) => write!(f, "invalid parameter {name}"),
        }
    }
}

/// Parse a path and optional query into a typed resource request.
///
/// # Errors
///
/// Returns a path or named-parameter refusal; malformed percent escapes and
/// non-UTF-8 data are always refused.
pub(crate) fn parse(path: &str, query: Option<&str>) -> Result<Route, RouteError> {
    let query = query.unwrap_or("");
    if query.len() > MAX_QUERY_BYTES {
        return Err(RouteError::BadParameter("query".to_owned()));
    }
    if path == "/api/catalog" {
        return parse_catalog(query).map(Route::Catalog);
    }
    if path == "/api/hour" {
        return parse_hour(query).map(Route::Hour);
    }
    if path == "/api/heatmap" {
        return parse_heatmap(query).map(Route::Heatmap);
    }
    let tail = path
        .strip_prefix("/api/segments/")
        .ok_or(RouteError::NoSuchPath)?;
    let pieces: Vec<&str> = tail.split('/').collect();
    if pieces.len() == 2 && pieces[1] == "snapshot" && !pieces[0].is_empty() {
        return parse_snapshot(number("segment_id", pieces[0])?, query)
            .map(Box::new)
            .map(Route::Snapshot);
    }
    if pieces.len() != 4 || pieces[1] != "sections" || pieces.iter().any(|piece| piece.is_empty()) {
        return Err(RouteError::NoSuchPath);
    }
    let segment_id = number("segment_id", pieces[0])?;
    let section = decoded("section", pieces[2], false)?;
    if section.is_empty() || section.len() > MAX_SECTION_BYTES {
        return Err(RouteError::BadParameter("section".to_owned()));
    }
    let segment = SegmentRequest {
        segment_id,
        section,
    };
    match pieces[3] {
        "index" if query.is_empty() => Ok(Route::Index(segment)),
        "index" => Err(RouteError::BadParameter("query".to_owned())),
        "history" => parse_data(segment, query).map(Route::History),
        "rows" => parse_rows(segment, query).map(Route::Rows),
        _ => Err(RouteError::NoSuchPath),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the bounded snapshot query grammar is deliberately parsed in one strict pass"
)]
fn parse_snapshot(segment_id: i64, query: &str) -> Result<SnapshotRequest, RouteError> {
    let mut at = None;
    let mut sections = Vec::new();
    let mut fields = Vec::new();
    let mut by = Vec::new();
    let mut lens = None;
    let mut semantic_order = None;
    let mut direction = None;
    let mut group = None;
    let mut page_size = None;
    let mut cursor = None;
    let mut search = None;
    let mut find = None;
    let mut first_match = None;
    let mut text = None;
    let mut filters: Vec<Filter> = Vec::new();
    let mut type_id = None;
    let mut row_ordinal = None;
    for (raw_name, raw_value) in pairs(query)? {
        match raw_name {
            "at" if at.is_none() => at = Some(number("at", raw_value)?),
            "section" => {
                let section = decoded("section", raw_value, false)?;
                if section.is_empty() || section.len() > MAX_SECTION_BYTES {
                    return Err(RouteError::BadParameter("section".to_owned()));
                }
                if sections.contains(&section) || sections.len() >= MAX_SNAPSHOT_SECTIONS {
                    return Err(RouteError::BadParameter("section".to_owned()));
                }
                sections.push(section);
            }
            "field" => {
                let field = decoded("field", raw_value, true)?;
                if field.is_empty() || fields.contains(&field) || fields.len() >= MAX_FIELDS {
                    return Err(RouteError::BadParameter("field".to_owned()));
                }
                fields.push(field);
            }
            "by" => {
                let field = decoded("by", raw_value, true)?;
                if field.is_empty() || by.contains(&field) || by.len() >= MAX_ORDER_FIELDS {
                    return Err(RouteError::BadParameter("by".to_owned()));
                }
                by.push(field);
            }
            "lens" if lens.is_none() => {
                let value = decoded("lens", raw_value, true)?;
                if value.is_empty() || value.len() > MAX_SECTION_BYTES {
                    return Err(RouteError::BadParameter("lens".to_owned()));
                }
                lens = Some(value);
            }
            "order" if semantic_order.is_none() => {
                let value = decoded("order", raw_value, true)?;
                if value.is_empty() || value.len() > MAX_SECTION_BYTES {
                    return Err(RouteError::BadParameter("order".to_owned()));
                }
                semantic_order = Some(value);
            }
            "direction" if direction.is_none() => {
                direction = Some(match raw_value {
                    "asc" => Order::Asc,
                    "desc" => Order::Desc,
                    _ => return Err(RouteError::BadParameter("direction".to_owned())),
                });
            }
            "group" if group.is_none() => {
                group = Some(match raw_value {
                    "database" => RelationGroup::Database,
                    "schema" => RelationGroup::Schema,
                    "tablespace" => RelationGroup::Tablespace,
                    "object" => RelationGroup::Object,
                    _ => return Err(RouteError::BadParameter("group".to_owned())),
                });
            }
            "type_id" if type_id.is_none() => {
                type_id = Some(unsigned_32("type_id", raw_value)?);
            }
            "row_ordinal" if row_ordinal.is_none() => {
                row_ordinal = Some(unsigned_64("row_ordinal", raw_value)?);
            }
            "text" if text.is_none() => {
                let kept = number("text", raw_value)?;
                if kept <= 0 {
                    return Err(RouteError::BadParameter("text".to_owned()));
                }
                text = Some(usize::try_from(kept).unwrap_or(usize::MAX));
            }
            "page_size" if page_size.is_none() => {
                page_size = Some(bounded("page_size", raw_value, MAX_SNAPSHOT_PAGE_SIZE)?);
            }
            "cursor" if cursor.is_none() => {
                let value = decoded("cursor", raw_value, true)?;
                if value.is_empty() {
                    return Err(RouteError::BadParameter("cursor".to_owned()));
                }
                cursor = Some(value);
            }
            "search" if search.is_none() => search = Some(snapshot_search("search", raw_value)?),
            "find" if find.is_none() => find = Some(snapshot_search("find", raw_value)?),
            "first_match" if first_match.is_none() => {
                if raw_value != "1" {
                    return Err(RouteError::BadParameter("first_match".to_owned()));
                }
                first_match = Some(true);
            }
            other => {
                let name = decoded("parameter", other, true)?;
                let Some(column) = name.strip_prefix("where.") else {
                    return Err(RouteError::BadParameter(name));
                };
                if column.is_empty()
                    || filters.len() >= MAX_FILTERS
                    || filters.iter().any(|filter| filter.column == column)
                {
                    return Err(RouteError::BadParameter("where".to_owned()));
                }
                filters.push(Filter {
                    column: column.to_owned(),
                    value: decoded(&name, raw_value, true)?,
                });
            }
        }
    }
    if semantic_order.is_some() && !by.is_empty() {
        return Err(RouteError::BadParameter("order".to_owned()));
    }
    if find.is_some() && search.is_some() {
        return Err(RouteError::BadParameter("find".to_owned()));
    }
    let typed_surface = lens.is_some() || semantic_order.is_some() || find.is_some();
    let process = if typed_surface && sections.as_slice() == ["os_process"] {
        Some(ProcessSurfaceRequest {
            lens: ProcessLens::parse(lens.as_deref())
                .ok_or_else(|| RouteError::BadParameter("lens".to_owned()))?,
            order: semantic_order.take(),
            direction,
        })
    } else {
        None
    };
    let typed_postgresql = typed_surface && process.is_none();
    let postgresql = if typed_postgresql {
        let [section] = sections.as_slice() else {
            return Err(RouteError::BadParameter("section".to_owned()));
        };
        let surface = PostgresqlSurface::parse(section, lens.as_deref()).ok_or_else(|| {
            RouteError::BadParameter(if lens.is_some() {
                "lens".to_owned()
            } else if semantic_order.is_some() {
                "order".to_owned()
            } else {
                "find".to_owned()
            })
        })?;
        if find.is_some()
            && matches!(
                surface,
                PostgresqlSurface::Activity | PostgresqlSurface::Databases
            )
        {
            return Err(RouteError::BadParameter("find".to_owned()));
        }
        if group.is_none()
            && matches!(
                surface,
                PostgresqlSurface::Tables(_) | PostgresqlSurface::Indexes(_)
            )
        {
            group = Some(RelationGroup::Object);
        }
        Some(PostgresqlSurfaceRequest {
            surface,
            order: semantic_order,
        })
    } else {
        None
    };
    if let Some(find) = find {
        search = Some(find);
    }
    let first_match = first_match.unwrap_or(false);
    if first_match
        && (sections.as_slice() != ["pg_stat_statements"]
            || fields.as_slice() != ["query"]
            || page_size != Some(1)
            || cursor.is_some()
            || search.is_none()
            || text.is_some()
            || !filters.is_empty()
            || type_id.is_some()
            || row_ordinal.is_some()
            || !by.is_empty()
            || postgresql.is_some()
            || direction.is_some()
            || group.is_some())
    {
        return Err(RouteError::BadParameter("first_match".to_owned()));
    }
    let paged = page_size.is_some()
        || cursor.is_some()
        || search.is_some()
        || !by.is_empty()
        || postgresql.is_some()
        || direction.is_some()
        || group.is_some();
    validate_snapshot_shape(&sections, paged, &filters, type_id, row_ordinal, group)?;
    let resolved_page_size = if process.is_some() {
        page_size
    } else {
        paged.then_some(page_size.unwrap_or(DEFAULT_SNAPSHOT_PAGE_SIZE))
    };
    Ok(SnapshotRequest {
        segment_id,
        active_position: None,
        at: at.ok_or_else(|| RouteError::BadParameter("at".to_owned()))?,
        sections,
        fields,
        by,
        direction: direction.unwrap_or(Order::Desc),
        group,
        postgresql,
        process,
        page_size: resolved_page_size,
        cursor,
        search,
        first_match,
        text,
        filters,
        activity_visibility: None,
        type_id,
        row_ordinal,
    })
}

fn snapshot_search(name: &str, raw: &str) -> Result<String, RouteError> {
    let value = decoded(name, raw, true)?;
    let value = value.trim();
    if value.is_empty() || value.chars().count() > MAX_SEARCH_EXPRESSION_CHARS {
        return Err(RouteError::BadParameter(name.to_owned()));
    }
    Ok(value.to_owned())
}

fn validate_snapshot_shape(
    sections: &[String],
    paged: bool,
    filters: &[Filter],
    type_id: Option<u32>,
    row_ordinal: Option<u64>,
    group: Option<RelationGroup>,
) -> Result<(), RouteError> {
    if sections.is_empty() {
        return Err(RouteError::BadParameter("section".to_owned()));
    }
    if sections.len() != 1
        && (paged || !filters.is_empty() || type_id.is_some() || row_ordinal.is_some())
    {
        return Err(RouteError::BadParameter("section".to_owned()));
    }
    if row_ordinal.is_some() && (type_id.is_none() || paged || !filters.is_empty()) {
        return Err(RouteError::BadParameter("row_ordinal".to_owned()));
    }
    if group.is_some() && type_id.is_some() {
        return Err(RouteError::BadParameter("type_id".to_owned()));
    }
    if group.is_some()
        && (!paged
            || row_ordinal.is_some()
            || !matches!(sections, [section] if section == "pg_stat_user_tables" || section == "pg_stat_user_indexes"))
    {
        return Err(RouteError::BadParameter("group".to_owned()));
    }
    Ok(())
}

fn parse_heatmap(query: &str) -> Result<HeatmapRequest, RouteError> {
    let mut from = None;
    let mut to = None;
    let mut section = None;
    let mut fields: Vec<String> = Vec::new();
    let mut columns = None;
    let mut top = None;
    let mut labels: Vec<String> = Vec::new();
    let mut group: Vec<String> = Vec::new();
    let mut type_id = None;
    for (raw_name, raw_value) in pairs(query)? {
        let name = decoded("parameter", raw_name, true)?;
        let value = decoded(&name, raw_value, true)?;
        match name.as_str() {
            "from" if from.is_none() => from = Some(number("from", &value)?),
            "to" if to.is_none() => to = Some(number("to", &value)?),
            "section" if section.is_none() => {
                if value.is_empty() || value.len() > MAX_SECTION_BYTES {
                    return Err(RouteError::BadParameter("section".to_owned()));
                }
                section = Some(value);
            }
            "field" => {
                if value.is_empty() || fields.contains(&value) || fields.len() >= MAX_HEATMAP_FIELDS
                {
                    return Err(RouteError::BadParameter("field".to_owned()));
                }
                fields.push(value);
            }
            "columns" if columns.is_none() => {
                columns = Some(bounded("columns", &value, MAX_HEATMAP_COLUMNS)?);
            }
            "top" if top.is_none() => top = Some(bounded("top", &value, MAX_HEATMAP_TOP)?),
            "label" => {
                if value.is_empty() || labels.contains(&value) || labels.len() >= MAX_HEATMAP_LABELS
                {
                    return Err(RouteError::BadParameter("label".to_owned()));
                }
                labels.push(value);
            }
            "group" => {
                if value.is_empty() || group.contains(&value) || group.len() >= MAX_HEATMAP_GROUP {
                    return Err(RouteError::BadParameter("group".to_owned()));
                }
                group.push(value);
            }
            "type_id" if type_id.is_none() => type_id = Some(unsigned_32("type_id", &value)?),
            _ => return Err(RouteError::BadParameter(name)),
        }
    }
    let from = from.ok_or_else(|| RouteError::BadParameter("from".to_owned()))?;
    let to = to.ok_or_else(|| RouteError::BadParameter("to".to_owned()))?;
    if from > to {
        return Err(RouteError::BadParameter("from".to_owned()));
    }
    if fields.is_empty() {
        return Err(RouteError::BadParameter("field".to_owned()));
    }
    // Group rows cannot carry per-identity labels.
    if !group.is_empty() && !labels.is_empty() {
        return Err(RouteError::BadParameter("label".to_owned()));
    }
    Ok(HeatmapRequest {
        from,
        to,
        section: section.ok_or_else(|| RouteError::BadParameter("section".to_owned()))?,
        fields,
        columns: columns.unwrap_or(DEFAULT_HEATMAP_COLUMNS),
        top: top.unwrap_or(DEFAULT_HEATMAP_TOP),
        labels,
        group,
        type_id,
    })
}

fn bounded(name: &str, value: &str, cap: usize) -> Result<usize, RouteError> {
    value
        .parse::<usize>()
        .ok()
        .filter(|parsed| (1..=cap).contains(parsed))
        .ok_or_else(|| RouteError::BadParameter(name.to_owned()))
}

fn parse_catalog(query: &str) -> Result<Window, RouteError> {
    let mut window = Window::default();
    for (raw_name, raw_value) in pairs(query)? {
        let name = decoded("parameter", raw_name, true)?;
        let value = decoded(&name, raw_value, true)?;
        match name.as_str() {
            "from" if window.from.is_none() => window.from = Some(number("from", &value)?),
            "to" if window.to.is_none() => window.to = Some(number("to", &value)?),
            _ => return Err(RouteError::BadParameter(name)),
        }
    }
    if window
        .from
        .zip(window.to)
        .is_some_and(|(from, to)| from > to)
    {
        return Err(RouteError::BadParameter("from".to_owned()));
    }
    Ok(window)
}

fn parse_hour(query: &str) -> Result<HourRequest, RouteError> {
    let (fields, filters, extras) = data_parameters(query)?;
    let mut window = Window::default();
    let mut section = None;
    let mut type_id = None;
    let mut group = None;
    for (name, value) in extras {
        match name.as_str() {
            "from" if window.from.is_none() => window.from = Some(number("from", &value)?),
            "to" if window.to.is_none() => window.to = Some(number("to", &value)?),
            "section" if section.is_none() => {
                if value.is_empty() || value.len() > MAX_SECTION_BYTES {
                    return Err(RouteError::BadParameter("section".to_owned()));
                }
                section = Some(value);
            }
            "type_id" if type_id.is_none() => type_id = Some(unsigned_32("type_id", &value)?),
            "group" if group.is_none() => {
                group = Some(match value.as_str() {
                    "database" => RelationGroup::Database,
                    "schema" => RelationGroup::Schema,
                    "tablespace" => RelationGroup::Tablespace,
                    "object" => RelationGroup::Object,
                    _ => return Err(RouteError::BadParameter("group".to_owned())),
                });
            }
            _ => return Err(RouteError::BadParameter(name)),
        }
    }
    if window
        .from
        .zip(window.to)
        .is_some_and(|(from, to)| from > to)
    {
        return Err(RouteError::BadParameter("from".to_owned()));
    }
    let Some(section) = section else {
        if fields.is_empty() && filters.is_empty() && type_id.is_none() && group.is_none() {
            return Ok(HourRequest {
                window,
                active_segment: None,
                series: None,
            });
        }
        return Err(RouteError::BadParameter("section".to_owned()));
    };
    validate_relation_series(&section, &fields, &filters, type_id, group)?;
    Ok(HourRequest {
        window,
        active_segment: None,
        series: Some(SeriesRequest {
            section,
            fields,
            filters,
            type_id,
            group,
        }),
    })
}

fn validate_relation_series(
    section: &str,
    fields: &[String],
    filters: &[Filter],
    type_id: Option<u32>,
    group: Option<RelationGroup>,
) -> Result<(), RouteError> {
    let Some(group) = group else {
        return Ok(());
    };
    if type_id.is_some()
        || fields.is_empty()
        || !matches!(section, "pg_stat_user_tables" | "pg_stat_user_indexes")
        || group == RelationGroup::Object
    {
        return Err(RouteError::BadParameter("group".to_owned()));
    }
    let required: &[&str] = match group {
        RelationGroup::Database => &["datid"],
        RelationGroup::Schema => &["datid", "schemaname"],
        RelationGroup::Tablespace => &["tablespace_oid"],
        RelationGroup::Object => unreachable!(),
    };
    if filters.len() != required.len()
        || required
            .iter()
            .any(|name| !filters.iter().any(|filter| filter.column == *name))
    {
        return Err(RouteError::BadParameter("where".to_owned()));
    }
    if group == RelationGroup::Tablespace
        && filters[0]
            .value
            .parse::<u32>()
            .ok()
            .is_none_or(|oid| oid == 0)
    {
        return Err(RouteError::BadParameter("where.tablespace_oid".to_owned()));
    }
    Ok(())
}

fn parse_data(segment: SegmentRequest, query: &str) -> Result<DataRequest, RouteError> {
    let (fields, filters, extras) = data_parameters(query)?;
    let mut after = None;
    let mut type_id = None;
    for (name, value) in extras {
        match name.as_str() {
            "after" if after.is_none() => after = Some(active_cursor(&value)?),
            "type_id" if type_id.is_none() => type_id = Some(unsigned_32("type_id", &value)?),
            _ => return Err(RouteError::BadParameter(name)),
        }
    }
    validate_after(&segment, after)?;
    Ok(DataRequest {
        segment,
        fields,
        filters,
        type_id,
        after,
    })
}

fn parse_rows(segment: SegmentRequest, query: &str) -> Result<RowsRequest, RouteError> {
    let (fields, filters, extras) = data_parameters(query)?;
    let mut order = Order::Asc;
    let mut page_size = DEFAULT_PAGE_SIZE;
    let mut cursor = None;
    let mut after = None;
    let mut type_id = None;
    let mut saw_order = false;
    let mut saw_page_size = false;
    for (name, value) in extras {
        match name.as_str() {
            "order" if !saw_order => {
                order = match value.as_str() {
                    "asc" => Order::Asc,
                    "desc" => Order::Desc,
                    _ => return Err(RouteError::BadParameter(name)),
                };
                saw_order = true;
            }
            "page_size" if !saw_page_size => {
                page_size = bounded("page_size", &value, MAX_PAGE_SIZE)?;
                saw_page_size = true;
            }
            "cursor" if cursor.is_none() && !value.is_empty() => cursor = Some(value),
            "after" if after.is_none() => after = Some(active_cursor(&value)?),
            "type_id" if type_id.is_none() => type_id = Some(unsigned_32("type_id", &value)?),
            _ => return Err(RouteError::BadParameter(name)),
        }
    }
    validate_after(&segment, after)?;
    Ok(RowsRequest {
        data: DataRequest {
            segment,
            fields,
            filters,
            type_id,
            after,
        },
        order,
        page_size,
        cursor,
    })
}

fn active_cursor(value: &str) -> Result<ActiveCursor, RouteError> {
    let (segment_id, wal_position) = value
        .split_once(',')
        .filter(|(segment_id, wal_position)| {
            !segment_id.is_empty() && !wal_position.is_empty() && !wal_position.contains(',')
        })
        .ok_or_else(|| RouteError::BadParameter("after".to_owned()))?;
    Ok(ActiveCursor {
        segment_id: number("after", segment_id)?,
        wal_position: wal_position
            .parse()
            .map_err(|_error| RouteError::BadParameter("after".to_owned()))?,
    })
}

fn validate_after(segment: &SegmentRequest, after: Option<ActiveCursor>) -> Result<(), RouteError> {
    if after.is_some_and(|cursor| cursor.segment_id != segment.segment_id) {
        return Err(RouteError::BadParameter("after".to_owned()));
    }
    Ok(())
}

type Extra = (String, String);
type DataParameters = (Vec<String>, Vec<Filter>, Vec<Extra>);

fn data_parameters(query: &str) -> Result<DataParameters, RouteError> {
    let mut fields = Vec::new();
    let mut filters = Vec::new();
    let mut extras = Vec::new();
    for (raw_name, raw_value) in pairs(query)? {
        let name = decoded("parameter", raw_name, true)?;
        let value = decoded(&name, raw_value, true)?;
        if name == "field" {
            if value.is_empty() || fields.len() >= MAX_FIELDS || fields.contains(&value) {
                return Err(RouteError::BadParameter("field".to_owned()));
            }
            fields.push(value);
        } else if let Some(column) = name.strip_prefix("where.") {
            if column.is_empty()
                || filters.len() >= MAX_FILTERS
                || filters
                    .iter()
                    .any(|filter: &Filter| filter.column == column)
            {
                return Err(RouteError::BadParameter("where".to_owned()));
            }
            filters.push(Filter {
                column: column.to_owned(),
                value,
            });
        } else {
            extras.push((name, value));
        }
    }
    Ok((fields, filters, extras))
}

fn pairs(query: &str) -> Result<Vec<(&str, &str)>, RouteError> {
    if query.is_empty() {
        return Ok(Vec::new());
    }
    query
        .split('&')
        .map(|part| {
            if part.is_empty() {
                return Err(RouteError::BadParameter("query".to_owned()));
            }
            Ok(part.split_once('=').unwrap_or((part, "")))
        })
        .collect()
}

fn decoded(name: &str, value: &str, plus_as_space: bool) -> Result<String, RouteError> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0_usize;
    while at < bytes.len() {
        match bytes[at] {
            b'+' if plus_as_space => {
                out.push(b' ');
                at += 1;
            }
            b'%' => {
                let raw = bytes
                    .get(at + 1..at + 3)
                    .ok_or_else(|| RouteError::BadParameter(name.to_owned()))?;
                let text = std::str::from_utf8(raw)
                    .map_err(|_error| RouteError::BadParameter(name.to_owned()))?;
                let byte = u8::from_str_radix(text, 16)
                    .map_err(|_error| RouteError::BadParameter(name.to_owned()))?;
                out.push(byte);
                at += 3;
            }
            byte => {
                out.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_error| RouteError::BadParameter(name.to_owned()))
}

fn number(name: &str, value: &str) -> Result<i64, RouteError> {
    value
        .parse()
        .map_err(|_error| RouteError::BadParameter(name.to_owned()))
}

fn unsigned_32(name: &str, value: &str) -> Result<u32, RouteError> {
    value
        .parse()
        .map_err(|_error| RouteError::BadParameter(name.to_owned()))
}

fn unsigned_64(name: &str, value: &str) -> Result<u64, RouteError> {
    value
        .parse()
        .map_err(|_error| RouteError::BadParameter(name.to_owned()))
}

#[cfg(test)]
mod tests;
