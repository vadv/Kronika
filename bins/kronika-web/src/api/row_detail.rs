//! One exact stored row addressed by an opaque reference.

use std::path::Path;

use serde_json::{Map, Value, json};

use super::events::label_event_fields;
use super::render::record;
use super::row_key::DetailLocator;
use super::snapshot::PreparedSnapshot;
use super::{ApiError, CachePolicy, Prepared, ResponseMeta};
use crate::route::{Order, SnapshotRequest};

pub(crate) struct PreparedRowDetail {
    locator: DetailLocator,
    snapshot: PreparedSnapshot,
}

pub(crate) struct ResolvedRowDetail {
    pub(crate) section: String,
    pub(crate) at: i64,
    pub(crate) fields: Map<String, Value>,
}

pub(crate) fn prepare(root: &Path, detail_ref: &str) -> Result<PreparedRowDetail, ApiError> {
    let locator = DetailLocator::from_detail_ref(detail_ref)
        .map_err(|_error| ApiError::BadLocator("invalid detail_ref".to_owned()))?;
    prepare_locator(root, locator)
}

pub(crate) fn prepare_locator(
    root: &Path,
    locator: DetailLocator,
) -> Result<PreparedRowDetail, ApiError> {
    let request = SnapshotRequest {
        segment_id: locator.segment_id,
        at: locator.at,
        sections: vec![locator.section.clone()],
        fields: Vec::new(),
        by: Vec::new(),
        direction: Order::Asc,
        group: None,
        page_size: None,
        cursor: None,
        search: None,
        first_match: false,
        text: None,
        filters: Vec::new(),
        type_id: Some(locator.type_id),
        row_ordinal: None,
    };
    let Prepared::Snapshot(snapshot) = super::snapshot::prepare(root, request, None)? else {
        return Err(ApiError::BadLocator(
            "detail_ref does not identify one recorded row".to_owned(),
        ));
    };
    Ok(PreparedRowDetail { locator, snapshot })
}

impl PreparedRowDetail {
    pub(crate) const fn meta() -> ResponseMeta {
        ResponseMeta::ok(CachePolicy::NoStore)
    }

    pub(crate) fn resolve(
        &self,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Option<ResolvedRowDetail>, ApiError> {
        let Some(row) = self.snapshot.fetch_identity_row(
            self.locator.row_ordinal,
            &self.locator.identity,
            cancelled,
        )?
        else {
            return Ok(None);
        };
        let Value::Object(mut fields) = row else {
            return Err(ApiError::BadLocator(
                "detail_ref does not identify one recorded row".to_owned(),
            ));
        };
        for field in [
            "segment_id",
            "type_id",
            "row_ordinal",
            "row_key",
            "identity",
            "detail_locator",
        ] {
            fields.remove(field);
        }
        label_event_fields(&self.locator.section, &mut fields);
        normalize_detail_text(&self.locator.section, &mut fields).map_err(|error| {
            ApiError::Unreadable(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                error,
            )))
        })?;
        Ok(Some(ResolvedRowDetail {
            section: self.locator.section.clone(),
            at: self.locator.at,
            fields,
        }))
    }

    pub(crate) fn stream(
        self,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), ApiError> {
        let Some(mut detail) = self.resolve(cancelled)? else {
            return Err(ApiError::BadLocator(
                "detail_ref does not identify one recorded row".to_owned(),
            ));
        };
        detail.fields.remove("at");
        if !cancelled() {
            emit(record(json!({
                "record": "row_detail",
                "section": detail.section,
                "at": detail.at.to_string(),
                "fields": detail.fields,
            }))?);
        }
        Ok(())
    }
}

pub(crate) fn normalize_detail_text(
    section: &str,
    fields: &mut Map<String, Value>,
) -> Result<(), String> {
    for (field, value) in fields {
        if super::row_key::is_detail_text(section, field) && !value.is_null() {
            *value = stable_text(std::mem::take(value)).map_err(|error| {
                format!("internal error: {section}.{field} is not stored text: {error}")
            })?;
        }
    }
    Ok(())
}

fn stable_text(value: Value) -> Result<Value, &'static str> {
    match value {
        Value::String(stored_text) => Ok(json!({
            "full_len": stored_text.len().to_string(),
            "sha256": null,
            "stored_text": stored_text,
            "truncated": false,
        })),
        Value::Object(object) if object.get("representation") == Some(&json!("text")) => {
            let stored_text = object.get("stored_text").ok_or("missing stored_text")?;
            let full_len = object.get("full_len").ok_or("missing full_len")?;
            let truncated = object.get("truncated").ok_or("missing truncated")?;
            let sha256 = object.get("sha256").ok_or("missing sha256")?;
            Ok(json!({
                "full_len": full_len,
                "sha256": sha256,
                "stored_text": stored_text,
                "truncated": truncated,
            }))
        }
        _ => Err("expected a UTF-8 string"),
    }
}
