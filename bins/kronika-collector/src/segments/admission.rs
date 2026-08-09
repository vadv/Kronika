//! Whether the next window still fits the format's per-segment caps.

use super::{
    BTreeMap, CodecError, DICT_BLOBS_TYPE_ID, DICT_STRINGS_TYPE_ID, DictStats, Error, FlushSummary,
    Interner, MAX_SECTION_ROWS, Placement, dict, final_data_body_bound, fmt,
};

#[derive(Debug)]
pub(super) enum AdmissionError {
    Capacity {
        resource: &'static str,
        projected: usize,
        max: usize,
    },
    ArithmeticOverflow {
        resource: &'static str,
    },
    Codec(CodecError),
}

impl AdmissionError {
    pub(super) const fn is_capacity(&self) -> bool {
        matches!(
            self,
            Self::Capacity { .. }
                | Self::Codec(
                    CodecError::TooManyRows { .. }
                        | CodecError::TooManyListValues { .. }
                        | CodecError::PlainPageTooLarge { .. }
                        | CodecError::SectionTooLarge { .. }
                )
        )
    }
}

impl fmt::Display for AdmissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Capacity {
                resource,
                projected,
                max,
            } => write!(
                f,
                "window would grow {resource} to {projected}, above the finished segment limit of {max}"
            ),
            Self::ArithmeticOverflow { resource } => {
                write!(
                    f,
                    "{resource} overflow while checking finished segment admission"
                )
            }
            Self::Codec(err) => write!(f, "finished segment admission: {err}"),
        }
    }
}

impl Error for AdmissionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Codec(err) => Some(err),
            Self::Capacity { .. } | Self::ArithmeticOverflow { .. } => None,
        }
    }
}

impl From<CodecError> for AdmissionError {
    fn from(err: CodecError) -> Self {
        Self::Codec(err)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct DataAdmission {
    pub(super) rows: usize,
    pub(super) list_i32_child_values: usize,
}

#[derive(Debug, Default)]
pub(super) struct AdmissionDelta {
    pub(super) data_by_type: BTreeMap<u32, DataAdmission>,
    pub(super) descriptors: usize,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct SegmentAdmission {
    pub(super) data_by_type: BTreeMap<u32, DataAdmission>,
    pub(super) descriptors: usize,
}

impl SegmentAdmission {
    pub(super) fn assess(
        &self,
        summary: &FlushSummary,
        interner: &Interner,
    ) -> Result<AdmissionDelta, AdmissionError> {
        self.assess_with_dictionary(summary, interner.stats())
    }

    pub(super) fn assess_window(
        summary: &FlushSummary,
        interner: &Interner,
    ) -> Result<AdmissionDelta, AdmissionError> {
        Self::default().assess_with_dictionary(summary, interner.window().stats())
    }

    fn assess_with_dictionary(
        &self,
        summary: &FlushSummary,
        dictionary: DictStats,
    ) -> Result<AdmissionDelta, AdmissionError> {
        let mut delta = AdmissionDelta {
            descriptors: summary.sections.len(),
            ..AdmissionDelta::default()
        };
        let descriptors = self.descriptors.checked_add(delta.descriptors).ok_or(
            AdmissionError::ArithmeticOverflow {
                resource: "section descriptors",
            },
        )?;
        if descriptors > MAX_SECTION_ROWS {
            return Err(AdmissionError::Capacity {
                resource: "section descriptors",
                projected: descriptors,
                max: MAX_SECTION_ROWS,
            });
        }
        self.assess_data(summary, &mut delta)?;
        Self::assess_dictionary(dictionary)?;
        Ok(delta)
    }

    pub(super) fn assess_data(
        &self,
        summary: &FlushSummary,
        delta: &mut AdmissionDelta,
    ) -> Result<(), AdmissionError> {
        for section in &summary.sections {
            if matches!(section.type_id, DICT_STRINGS_TYPE_ID | DICT_BLOBS_TYPE_ID) {
                continue;
            }
            let incoming = delta.data_by_type.entry(section.type_id).or_default();
            incoming.rows = incoming.rows.checked_add(section.rows as usize).ok_or(
                AdmissionError::ArithmeticOverflow {
                    resource: "data rows",
                },
            )?;
            incoming.list_i32_child_values = incoming
                .list_i32_child_values
                .checked_add(section.list_i32_child_value_count)
                .ok_or(AdmissionError::ArithmeticOverflow {
                    resource: "ListI32 child values",
                })?;
        }
        for (&type_id, &incoming) in &delta.data_by_type {
            let current = self.data_by_type.get(&type_id).copied().unwrap_or_default();
            let rows = current.rows.checked_add(incoming.rows).ok_or(
                AdmissionError::ArithmeticOverflow {
                    resource: "data rows",
                },
            )?;
            let list_i32_child_values = current
                .list_i32_child_values
                .checked_add(incoming.list_i32_child_values)
                .ok_or(AdmissionError::ArithmeticOverflow {
                    resource: "ListI32 child values",
                })?;
            final_data_body_bound(type_id, rows, list_i32_child_values)?;
        }
        Ok(())
    }

    fn assess_dictionary(stats: DictStats) -> Result<(), AdmissionError> {
        let string_bytes = usize::try_from(stats.string_bytes).map_err(|_error| {
            AdmissionError::ArithmeticOverflow {
                resource: "dictionary bytes",
            }
        })?;
        let blob_bytes = usize::try_from(stats.blob_bytes).map_err(|_error| {
            AdmissionError::ArithmeticOverflow {
                resource: "dictionary bytes",
            }
        })?;
        if stats.string_count != 0 {
            dict::final_dictionary_body_bound(
                Placement::Strings,
                stats.string_count,
                string_bytes,
                0,
            )?;
        }
        if stats.blob_count != 0 {
            dict::final_dictionary_body_bound(
                Placement::Blobs,
                stats.blob_count,
                blob_bytes,
                stats.truncated_blob_count,
            )?;
        }
        Ok(())
    }

    pub(super) fn commit(&mut self, delta: AdmissionDelta) {
        for (type_id, incoming) in delta.data_by_type {
            let data = self.data_by_type.entry(type_id).or_default();
            data.rows = data.rows.saturating_add(incoming.rows);
            data.list_i32_child_values = data
                .list_i32_child_values
                .saturating_add(incoming.list_i32_child_values);
        }
        self.descriptors = self.descriptors.saturating_add(delta.descriptors);
    }
}
