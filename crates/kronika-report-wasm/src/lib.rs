//! Browser-callable request adapter for one standalone Kronika report.

use kronika_api::RouteError;
use kronika_layout::SegmentId;
use kronika_query::{QueryError, QuerySink};
use kronika_report::{ReportEngine, ReportError, ReportInput};
use wasm_bindgen::prelude::*;

const OK: u16 = 200;
const BAD_REQUEST: u16 = 400;
const NOT_FOUND: u16 = 404;
const INTERNAL_SERVER_ERROR: u16 = 500;

/// Stable response parts sufficient to construct a Fetch `Response`.
#[wasm_bindgen]
#[derive(Debug)]
pub struct ReportResponse {
    status: u16,
    code: Option<String>,
    parameter: Option<String>,
    message: Option<String>,
    body: Vec<u8>,
}

#[wasm_bindgen]
impl ReportResponse {
    /// HTTP-compatible status for this request.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn status(&self) -> u16 {
        self.status
    }

    /// Stable refusal code, absent on success.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn code(&self) -> Option<String> {
        self.code.clone()
    }

    /// Named invalid parameter when one applies.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn parameter(&self) -> Option<String> {
        self.parameter.clone()
    }

    /// Human-readable refusal text, absent on success.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn message(&self) -> Option<String> {
        self.message.clone()
    }

    /// Move the unchanged NDJSON bytes out of a successful response.
    #[wasm_bindgen(js_name = takeBody)]
    pub fn take_body(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.body)
    }
}

impl ReportResponse {
    fn success(body: Vec<u8>) -> Self {
        Self {
            status: OK,
            code: None,
            parameter: None,
            message: None,
            body,
        }
    }

    fn refusal(
        status: u16,
        code: &'static str,
        parameter: Option<String>,
        message: String,
    ) -> Self {
        Self {
            status,
            code: Some(code.to_owned()),
            parameter,
            message: Some(message),
            body: Vec::new(),
        }
    }

    fn bad_parameter(parameter: &'static str) -> Self {
        Self::refusal(
            BAD_REQUEST,
            "bad_parameter",
            Some(parameter.to_owned()),
            format!("invalid parameter {parameter}"),
        )
    }

    fn route(error: RouteError) -> Self {
        match error {
            RouteError::NoSuchPath => {
                Self::refusal(NOT_FOUND, "no_such_path", None, "no such path".to_owned())
            }
            RouteError::BadParameter(parameter) => Self::refusal(
                BAD_REQUEST,
                "bad_parameter",
                Some(parameter.clone()),
                format!("invalid parameter {parameter}"),
            ),
        }
    }

    fn query(error: QueryError) -> Self {
        let status = match error {
            QueryError::NoSuchSegment | QueryError::NoSuchSection => NOT_FOUND,
            QueryError::NoSuchColumn(_)
            | QueryError::MixedUnits(_)
            | QueryError::BadFilter(_)
            | QueryError::BadCursor
            | QueryError::BadLocator(_) => BAD_REQUEST,
            QueryError::Cancelled | QueryError::Unreadable(_) => INTERNAL_SERVER_ERROR,
            _ => INTERNAL_SERVER_ERROR,
        };
        let code = error.code();
        let parameter = error.parameter().map(str::to_owned);
        Self::refusal(status, code, parameter, error.to_string())
    }

    fn report(error: ReportError) -> Self {
        Self::refusal(INTERNAL_SERVER_ERROR, "unreadable", None, error.to_string())
    }
}

#[derive(Debug, Default)]
struct NdjsonSink {
    body: Vec<u8>,
}

impl QuerySink for NdjsonSink {
    fn record(&mut self, bytes: Vec<u8>) -> bool {
        self.body.extend_from_slice(&bytes);
        true
    }

    fn cancelled(&self) -> bool {
        false
    }
}

/// Answer one parsed request from one owned finished ZMS and canonical IDX.
///
/// The decimal segment text is converted immediately because JavaScript
/// numbers cannot represent every valid segment identity.
#[wasm_bindgen]
#[must_use]
pub fn request(
    segment_id: &str,
    zms: Vec<u8>,
    idx: Vec<u8>,
    configured_sources: u32,
    max_zms_bytes: u64,
    path: &str,
    query: &str,
) -> ReportResponse {
    let Ok(segment_id) = segment_id.parse::<i64>() else {
        return ReportResponse::bad_parameter("segment_id");
    };
    let Ok(segment_id) = SegmentId::new(segment_id) else {
        return ReportResponse::bad_parameter("segment_id");
    };
    let route = match kronika_api::parse(path, Some(query)) {
        Ok(route) => route,
        Err(error) => return ReportResponse::route(error),
    };
    let request = match route.into_query() {
        Ok(request) => request,
        Err(error) => return ReportResponse::query(error),
    };
    let engine = match ReportEngine::new(ReportInput {
        segment_id,
        zms,
        idx,
        configured_sources,
        max_zms_bytes,
    }) {
        Ok(engine) => engine,
        Err(error) => return ReportResponse::report(error),
    };
    let mut sink = NdjsonSink::default();
    match engine.execute(request, &mut sink) {
        Ok(()) => ReportResponse::success(sink.body),
        Err(error) => ReportResponse::query(error),
    }
}

#[cfg(test)]
mod tests;
