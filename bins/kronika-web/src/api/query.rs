//! Native request conversion for shared physical-layout planning.

use kronika_reader::Segment;

#[cfg(test)]
pub(super) use kronika_query::OutputField;
pub(super) use kronika_query::Plan;

use super::ApiError;
use crate::route::DataRequest;

pub(super) fn plans(
    segment: &Segment,
    request: &DataRequest,
    history_coordinates: bool,
) -> Result<Vec<Plan>, ApiError> {
    let request = kronika_query::DataRequest {
        segment: kronika_query::SegmentRequest {
            segment_id: request.segment.segment_id,
            section: request.segment.section.clone(),
        },
        fields: request.fields.clone(),
        filters: request
            .filters
            .iter()
            .map(|filter| kronika_query::Filter {
                column: filter.column.clone(),
                value: filter.value.clone(),
            })
            .collect(),
        type_id: request.type_id,
        after: request.after.map(|cursor| kronika_query::ActiveCursor {
            segment_id: cursor.segment_id,
            wal_position: cursor.wal_position,
        }),
    };
    kronika_query::plans(segment, &request, history_coordinates).map_err(ApiError::from)
}

pub(super) fn resolved_dictionary(
    segment: &Segment,
    ids: &std::collections::HashSet<u64>,
) -> Result<kronika_reader::Dictionary, ApiError> {
    Ok(kronika_query::resolved_dictionary(segment, ids)?)
}
