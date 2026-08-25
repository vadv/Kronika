//! One pinned catalog and public product context shared by HTTP and MCP.

use std::cell::Cell;
use std::ops::Bound::{Included, Unbounded};
use std::path::{Path, PathBuf};

use hyper::StatusCode;
use kronika_reader::{Listing, Reader};
use serde_json::{Value, json};

use super::catalog::PreparedCatalog;
use super::{ApiError, ProductError, ResponseMeta};
use crate::route::Window;

const MAX_CONTEXT_SEGMENTS: usize = 64;
const MAX_CONTEXT_SECTION_PRESENCES: usize = 512;
const MAX_CONTEXT_WARNINGS: usize = 64;
const MAX_CONTEXT_JSON_BYTES: usize = 64 * 1_024;

pub(crate) struct ProductContext {
    pub(crate) value: Value,
    records: Vec<Value>,
}

pub(crate) struct PreparedCatalogResponse {
    root: PathBuf,
    window: Window,
    configured_sources: u32,
    synthetic_demo: bool,
}

pub(super) fn prepare(
    root: &Path,
    window: Window,
    configured_sources: u32,
    synthetic_demo: bool,
) -> PreparedCatalogResponse {
    PreparedCatalogResponse {
        root: root.to_owned(),
        window,
        configured_sources,
        synthetic_demo,
    }
}

pub(crate) fn produce(
    root: &Path,
    window: Window,
    configured_sources: u32,
    synthetic_demo: bool,
    cancelled: &impl Fn() -> bool,
) -> Result<ProductContext, ApiError> {
    let records = catalog_records(root, window, configured_sources, synthetic_demo, cancelled)?;
    check_cancelled(cancelled)?;
    let heatmap = crate::heatmap_product::vocabulary().map_err(|error| {
        product_error(
            "heatmap_registry_unreadable",
            error.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
            false,
        )
    })?;
    check_cancelled(cancelled)?;
    let surfaces = super::surface::product_surface_vocabulary();
    let mut product_semantics = Vec::new();
    for definition in crate::product_semantics::all().map_err(|error| {
        product_error(
            "semantics_unreadable",
            error.to_string(),
            StatusCode::INTERNAL_SERVER_ERROR,
            false,
        )
    })? {
        check_cancelled(cancelled)?;
        product_semantics.push(serde_json::to_value(definition)?);
    }
    check_cancelled(cancelled)?;
    let finding_semantics = crate::product_semantics::findings();
    check_cancelled(cancelled)?;
    let health_semantics = crate::product_semantics::health();
    check_cancelled(cancelled)?;
    let value = json!({
        "catalog": &records,
        "surfaces": {
            "heatmap": heatmap,
            "process": surfaces["process"],
            "postgresql": surfaces["postgresql"],
        },
        "semantics": {
            "products": product_semantics,
            "findings": finding_semantics,
            "health": health_semantics,
        },
    });
    check_cancelled(cancelled)?;
    validate_json_bytes(serde_json::to_vec(&value)?.len())?;
    check_cancelled(cancelled)?;
    Ok(ProductContext { value, records })
}

impl PreparedCatalogResponse {
    pub(super) const fn meta() -> ResponseMeta {
        PreparedCatalog::meta()
    }

    pub(super) fn stream(
        self,
        emit: &mut impl FnMut(Value) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), ApiError> {
        let context = produce(
            &self.root,
            self.window,
            self.configured_sources,
            self.synthetic_demo,
            cancelled,
        )?;
        for record in context.records {
            if cancelled() || !emit(record) {
                return Ok(());
            }
        }
        if !cancelled() {
            let _emitted = emit(json!({
                "record": "product_context",
                "context": context.value,
            }));
        }
        Ok(())
    }
}

fn catalog_records(
    root: &Path,
    window: Window,
    configured_sources: u32,
    synthetic_demo: bool,
    cancelled: &impl Fn() -> bool,
) -> Result<Vec<Value>, ApiError> {
    check_cancelled(cancelled)?;
    let started = std::time::Instant::now();
    let reader = Reader::open(root)?;
    check_cancelled(cancelled)?;
    let listed = reader.catalog_segments_cancellable(
        (
            window.from.map_or(Unbounded, Included),
            window.to.map_or(Unbounded, Included),
        ),
        cancelled,
    );
    let listing = match listed {
        Ok(listing) => listing,
        Err(_error) if cancelled() => return Err(cancelled_error()),
        Err(error) => return Err(error.into()),
    };
    super::catalog::log_open(listing.segments.len(), &listing.warnings, started);
    render_catalog_records(
        listing,
        window,
        configured_sources,
        synthetic_demo,
        cancelled,
    )
}

fn render_catalog_records(
    listing: Listing,
    window: Window,
    configured_sources: u32,
    synthetic_demo: bool,
    cancelled: &impl Fn() -> bool,
) -> Result<Vec<Value>, ApiError> {
    validate_listing(&listing, cancelled)?;
    let saw_cancel = Cell::new(false);
    let tracked_cancel = || {
        let stopped = cancelled();
        saw_cancel.set(saw_cancel.get() || stopped);
        stopped
    };
    let mut records = Vec::new();
    PreparedCatalog::from_listing(listing, window, configured_sources, synthetic_demo).stream(
        &mut |record| {
            records.push(record);
            true
        },
        &tracked_cancel,
    )?;
    if saw_cancel.get() || cancelled() {
        return Err(cancelled_error());
    }
    Ok(records)
}

fn validate_listing(listing: &Listing, cancelled: &impl Fn() -> bool) -> Result<(), ApiError> {
    validate_bounds(listing.segments.len(), 0, listing.warnings.len())?;
    let mut sections = 0_usize;
    for segment in &listing.segments {
        check_cancelled(cancelled)?;
        for _section in segment.sections() {
            check_cancelled(cancelled)?;
            sections = sections.saturating_add(1);
            if sections > MAX_CONTEXT_SECTION_PRESENCES {
                validate_bounds(listing.segments.len(), sections, listing.warnings.len())?;
            }
        }
    }
    Ok(())
}

fn validate_bounds(segments: usize, sections: usize, warnings: usize) -> Result<(), ApiError> {
    if segments > MAX_CONTEXT_SEGMENTS {
        return Err(product_error(
            "segment_limit_exceeded",
            "The catalog contains more than 64 segments.",
            StatusCode::UNPROCESSABLE_ENTITY,
            false,
        ));
    }
    if sections > MAX_CONTEXT_SECTION_PRESENCES {
        return Err(product_error(
            "layout_limit_exceeded",
            "The catalog contains more than 512 segment-layout presences.",
            StatusCode::UNPROCESSABLE_ENTITY,
            false,
        ));
    }
    if warnings > MAX_CONTEXT_WARNINGS {
        return Err(product_error(
            "warning_limit_exceeded",
            "The catalog warnings exceed their bounded result limit.",
            StatusCode::UNPROCESSABLE_ENTITY,
            false,
        ));
    }
    Ok(())
}

fn validate_json_bytes(bytes: usize) -> Result<(), ApiError> {
    if bytes > MAX_CONTEXT_JSON_BYTES {
        return Err(product_error(
            "context_byte_limit_exceeded",
            "The catalog and shared product definitions exceed their bounded result size.",
            StatusCode::UNPROCESSABLE_ENTITY,
            false,
        ));
    }
    Ok(())
}

fn check_cancelled(cancelled: &impl Fn() -> bool) -> Result<(), ApiError> {
    if cancelled() {
        Err(cancelled_error())
    } else {
        Ok(())
    }
}

fn cancelled_error() -> ApiError {
    product_error(
        "cancelled",
        "The catalog context read was cancelled.",
        StatusCode::REQUEST_TIMEOUT,
        true,
    )
}

fn product_error(
    code: &'static str,
    message: impl Into<String>,
    status: StatusCode,
    retryable: bool,
) -> ApiError {
    ApiError::Product(Box::new(ProductError {
        code,
        message: message.into(),
        parameter: None,
        retryable,
        status,
    }))
}

#[cfg(test)]
mod tests;
