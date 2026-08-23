mod core;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use rmcp::ErrorData;
use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock};
use serde_json::{Map, Value, json};

use super::{State, catalog, expert, postgresql};

const DEFAULT_DATA_BYTES: usize = 32 * 1_024;
const QUEUE_WAIT: Duration = Duration::from_secs(1);
const CONTEXT_DEADLINE: Duration = Duration::from_secs(5);
const SCAN_DEADLINE: Duration = Duration::from_secs(45);
const ROW_VISITS: u64 = 1_000_000;
const DECODED_CELLS: u64 = 2_000_000;

pub(super) struct Payload {
    pub(super) anchor: Value,
    pub(super) data: Value,
    pub(super) page: Value,
    pub(super) warnings: Vec<Value>,
    pub(super) summary: String,
}

#[derive(Debug)]
pub(super) struct Failure {
    pub(super) code: &'static str,
    pub(super) message: String,
    pub(super) parameter: Option<String>,
    pub(super) retryable: bool,
}

impl Failure {
    pub(super) fn input(parameter: &'static str, message: impl Into<String>) -> Self {
        Self {
            code: "invalid_input",
            message: message.into(),
            parameter: Some(parameter.to_owned()),
            retryable: false,
        }
    }

    pub(super) fn bounded(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            parameter: None,
            retryable: false,
        }
    }
}

struct ScanGuard {
    external: Arc<dyn Fn() -> bool + Send + Sync>,
    checks: AtomicU64,
    check_limit: u64,
    limit_hit: AtomicBool,
    deadline_hit: AtomicBool,
    client_cancelled: AtomicBool,
}

impl ScanGuard {
    fn new(external: Arc<dyn Fn() -> bool + Send + Sync>, cells_per_visit: u64) -> Self {
        Self {
            external,
            checks: AtomicU64::new(0),
            check_limit: ROW_VISITS.min(DECODED_CELLS / cells_per_visit.max(1)),
            limit_hit: AtomicBool::new(false),
            deadline_hit: AtomicBool::new(false),
            client_cancelled: AtomicBool::new(false),
        }
    }

    fn cancelled(&self) -> bool {
        if self.deadline_hit.load(Ordering::Relaxed) {
            return true;
        }
        if (self.external)() {
            self.client_cancelled.store(true, Ordering::Relaxed);
            return true;
        }
        if self.checks.fetch_add(1, Ordering::Relaxed) >= self.check_limit {
            self.limit_hit.store(true, Ordering::Relaxed);
            return true;
        }
        false
    }
}

pub(super) async fn dispatch(
    state: State,
    request: CallToolRequestParams,
    cancelled: impl Fn() -> bool + Send + Sync + 'static,
) -> Result<CallToolResult, ErrorData> {
    let name = request.name.into_owned();
    if catalog::find(&name).is_none() {
        return Err(ErrorData::invalid_params("tool not found", None));
    }
    let args = request.arguments.unwrap_or_default();
    let budget = match data_budget(&args) {
        Ok(budget) => budget,
        Err(result) => return Ok(result),
    };
    let external: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(cancelled);
    if external() {
        return Ok(result_from_failure(cancelled_failure()));
    }
    let permit = match tokio::time::timeout(QUEUE_WAIT, state.heavy_scans.clone().acquire_owned())
        .await
    {
        Ok(Ok(permit)) => permit,
        Ok(Err(_closed)) => {
            return Ok(result_from_failure(Failure {
                code: "unavailable",
                message: "The historical scan admission gate is unavailable.".to_owned(),
                parameter: None,
                retryable: true,
            }));
        }
        Err(_elapsed) => {
            return Ok(result_from_failure(Failure {
                code: "busy",
                message: "Two historical scans are already running; retry this call.".to_owned(),
                parameter: None,
                retryable: true,
            }));
        }
    };
    if external() {
        return Ok(result_from_failure(cancelled_failure()));
    }

    let guard = Arc::new(ScanGuard::new(external, cells_per_visit(&name, &args)));
    let task_guard = Arc::clone(&guard);
    let task_name = name.clone();
    let task = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let stopped = || task_guard.cancelled();
        if task_name.starts_with("kronika_find_postgresql_")
            || task_name == "kronika_get_postgresql_overview"
        {
            postgresql::execute(&state, &task_name, &args, &stopped)
                .map(|payload| Payload {
                    anchor: payload.anchor,
                    data: payload.data,
                    page: payload.page,
                    warnings: payload.warnings,
                    summary: payload.summary,
                })
                .map_err(|failure| Failure {
                    code: failure.code,
                    message: failure.message,
                    parameter: failure.parameter,
                    retryable: failure.retryable,
                })
        } else if matches!(
            task_name.as_str(),
            "kronika_find_events"
                | "kronika_get_metric_history"
                | "kronika_get_snapshot"
                | "kronika_get_row_detail"
        ) {
            expert::execute(&state, &task_name, &args, &stopped)
                .map(|payload| Payload {
                    anchor: payload.anchor,
                    data: payload.data,
                    page: payload.page,
                    warnings: payload.warnings,
                    summary: payload.summary,
                })
                .map_err(|failure| Failure {
                    code: failure.code,
                    message: failure.message,
                    parameter: failure.parameter,
                    retryable: failure.retryable,
                })
        } else {
            core::execute(&state, &task_name, &args, budget, &stopped)
        }
    });
    let deadline = if name == "kronika_get_context" {
        CONTEXT_DEADLINE
    } else {
        SCAN_DEADLINE
    };
    let executed = match tokio::time::timeout(deadline, task).await {
        Ok(Ok(executed)) => executed,
        Ok(Err(_join)) => Err(Failure {
            code: "internal_error",
            message: "The historical scan worker failed.".to_owned(),
            parameter: None,
            retryable: true,
        }),
        Err(_elapsed) => {
            guard.deadline_hit.store(true, Ordering::Relaxed);
            return Ok(result_from_failure(Failure {
                code: "deadline_exceeded",
                message: "The bounded historical scan exceeded its deadline.".to_owned(),
                parameter: None,
                retryable: true,
            }));
        }
    };
    if guard.client_cancelled.load(Ordering::Relaxed) {
        return Ok(result_from_failure(cancelled_failure()));
    }
    if guard.limit_hit.load(Ordering::Relaxed) {
        return Ok(result_from_failure(Failure::bounded(
            "scan_limit_exceeded",
            "The scan reached the physical row or decoded-cell limit.",
        )));
    }
    match executed {
        Ok(payload) => Ok(result_from_payload(payload, budget)),
        Err(failure) => Ok(result_from_failure(failure)),
    }
}

fn data_budget(args: &Map<String, Value>) -> Result<usize, CallToolResult> {
    let Some(value) = args.get("data_budget_bytes") else {
        return Ok(DEFAULT_DATA_BYTES);
    };
    let Some(value) = value.as_u64().and_then(|value| usize::try_from(value).ok()) else {
        return Err(result_from_failure(Failure::input(
            "data_budget_bytes",
            "data_budget_bytes must be an integer.",
        )));
    };
    if !(1_024..=super::STRUCTURED_CONTENT_BYTES).contains(&value) {
        return Err(result_from_failure(Failure::input(
            "data_budget_bytes",
            "data_budget_bytes must be between 1024 and 98304.",
        )));
    }
    Ok(value)
}

fn cells_per_visit(name: &str, args: &Map<String, Value>) -> u64 {
    let projected = args
        .get("fields")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let labels = args
        .get("labels")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let groups = args
        .get("group")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let base = if name == "kronika_rank_heatmap" {
        32_usize
            .saturating_add(projected)
            .saturating_add(labels)
            .saturating_add(groups)
    } else if projected == 0 {
        128
    } else {
        projected.saturating_add(16)
    };
    u64::try_from(base).unwrap_or(u64::MAX)
}

fn result_from_payload(payload: Payload, budget: usize) -> CallToolResult {
    let structured = json!({
        "status": "ok",
        "anchor": payload.anchor,
        "data": payload.data,
        "page": payload.page,
        "warnings": payload.warnings,
    });
    let encoded = serde_json::to_vec(&structured).map_or(usize::MAX, |bytes| bytes.len());
    if encoded > budget {
        return result_from_failure(Failure::bounded(
            "output_budget_exceeded",
            format!(
                "The structured result needs {encoded} bytes; reduce the page, fields, top, or columns, or raise data_budget_bytes."
            ),
        ));
    }
    let mut result = CallToolResult::structured(structured);
    let summary = if payload.summary.len() <= super::TEXT_SUMMARY_BYTES {
        payload.summary
    } else {
        "Kronika returned a bounded historical result; inspect structuredContent.".to_owned()
    };
    result.content = vec![ContentBlock::text(summary)];
    result
}

fn result_from_failure(failure: Failure) -> CallToolResult {
    let structured = json!({
        "status": "error",
        "error": {
            "code": failure.code,
            "message": failure.message,
            "parameter": failure.parameter,
            "retryable": failure.retryable,
        }
    });
    let mut result = CallToolResult::structured_error(structured);
    result.content = vec![ContentBlock::text(failure.message)];
    result
}

fn cancelled_failure() -> Failure {
    Failure {
        code: "cancelled",
        message: "The client cancelled the historical scan.".to_owned(),
        parameter: None,
        retryable: true,
    }
}
