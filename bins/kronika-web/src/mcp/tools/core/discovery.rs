use serde_json::Value;

use super::{Failure, Payload, anchor, api_failure};
use crate::api;
use crate::mcp::State;
use crate::route::Window;

pub(super) fn payload(state: &State, cancelled: &impl Fn() -> bool) -> Result<Payload, Failure> {
    let context = api::produce_product_context(
        &state.data_root,
        Window::default(),
        state.sources,
        state.synthetic_demo,
        cancelled,
    )
    .map_err(|error| api_failure(&error))?;
    Ok(Payload {
        anchor: anchor(None, None, None, None),
        data: context.value,
        page: Value::Null,
        // Catalog warnings are already part of the shared payload. Keeping the
        // envelope empty avoids duplicating them outside the shared size bound.
        warnings: Vec::new(),
        summary: "Returned the recorded catalog and shared product definitions.".to_owned(),
    })
}

#[cfg(test)]
mod tests;
