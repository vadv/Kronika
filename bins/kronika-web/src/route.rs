//! Native-only routes around the shared recorded-data parser.

#[cfg(test)]
pub(crate) use kronika_api::{
    ActiveCursor, DEFAULT_SNAPSHOT_PAGE_SIZE, MAX_SEARCH_EXPRESSION_CHARS, SeriesRequest,
};
pub(crate) use kronika_api::{
    DataRequest, HeatmapRequest, HourPart, HourRequest, MAX_QUERY_BYTES, MAX_SNAPSHOT_PAGE_SIZE,
    RouteError, RowsRequest, SegmentRequest, Window,
};
#[cfg(test)]
pub(crate) use kronika_query::Filter;
pub(crate) use kronika_query::{Order, RelationGroup, SnapshotRequest};

/// A parsed native web route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Route {
    Catalog(Window),
    Hour(HourRequest),
    Index(SegmentRequest),
    History(DataRequest),
    Rows(RowsRequest),
    Snapshot(Box<SnapshotRequest>),
    Heatmap(HeatmapRequest),
    Events(kronika_query::EventsQuery),
    RowDetail(String),
    McpAccess,
    InstanceLabel,
}

impl From<kronika_api::Route> for Route {
    fn from(route: kronika_api::Route) -> Self {
        match route {
            kronika_api::Route::Catalog(request) => Self::Catalog(request),
            kronika_api::Route::Hour(request) => Self::Hour(request),
            kronika_api::Route::Index(request) => Self::Index(request),
            kronika_api::Route::History(request) => Self::History(request),
            kronika_api::Route::Rows(request) => Self::Rows(request),
            kronika_api::Route::Snapshot(request) => Self::Snapshot(request),
            kronika_api::Route::Heatmap(request) => Self::Heatmap(request),
            kronika_api::Route::Events(request) => Self::Events(request),
            kronika_api::Route::RowDetail(request) => Self::RowDetail(request),
        }
    }
}

pub(crate) fn parse(path: &str, query: Option<&str>) -> Result<Route, RouteError> {
    let query = query.unwrap_or("");
    match path {
        "/api/mcp-access" => native(query, Route::McpAccess),
        "/api/instance-label" => native(query, Route::InstanceLabel),
        _ => kronika_api::parse(path, Some(query)).map(Into::into),
    }
}

fn native(query: &str, route: Route) -> Result<Route, RouteError> {
    if query.is_empty() {
        Ok(route)
    } else {
        Err(RouteError::BadParameter("query".to_owned()))
    }
}

#[cfg(test)]
mod tests;
