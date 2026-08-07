//! Whether the next window still fits the format's per-segment caps.

use super::{
    BTreeMap, CodecError, DICT_BLOBS_TYPE_ID, DICT_STRINGS_TYPE_ID, EntrySnapshot, Error,
    FlushSummary, Interner, MAX_SECTION_ROWS, Placement, StrId, dict, final_data_body_bound, fmt,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AdmissionDictionaryValue {
    String(Vec<u8>),
    Blob {
        bytes: Vec<u8>,
        full_len: u64,
        truncated: bool,
        full_sha256: Option<[u8; 32]>,
    },
}

impl AdmissionDictionaryValue {
    pub(super) fn from_snapshot(entry: EntrySnapshot<'_>) -> Self {
        match entry.placement {
            Placement::Strings => Self::String(entry.stored_bytes.to_vec()),
            Placement::Blobs => Self::Blob {
                bytes: entry.stored_bytes.to_vec(),
                full_len: entry.full_len,
                truncated: entry.truncated,
                full_sha256: entry.full_sha256,
            },
        }
    }

    pub(super) fn matches_snapshot(&self, entry: EntrySnapshot<'_>) -> bool {
        match self {
            Self::String(bytes) => {
                entry.placement == Placement::Strings && bytes.as_slice() == entry.stored_bytes
            }
            Self::Blob {
                bytes,
                full_len,
                truncated,
                full_sha256,
            } => {
                entry.placement == Placement::Blobs
                    && bytes.as_slice() == entry.stored_bytes
                    && *full_len == entry.full_len
                    && *truncated == entry.truncated
                    && *full_sha256 == entry.full_sha256
            }
        }
    }

    pub(super) const fn placement(&self) -> Placement {
        match self {
            Self::String(_) => Placement::Strings,
            Self::Blob { .. } => Placement::Blobs,
        }
    }

    pub(super) const fn stored_len(&self) -> usize {
        match self {
            Self::String(bytes) | Self::Blob { bytes, .. } => bytes.len(),
        }
    }

    pub(super) const fn truncated(&self) -> bool {
        matches!(
            self,
            Self::Blob {
                truncated: true,
                ..
            }
        )
    }
}

#[derive(Debug)]
pub(super) enum AdmissionError {
    Capacity {
        resource: &'static str,
        projected: usize,
        max: usize,
    },
    DictionaryConflict {
        str_id: u64,
    },
    DictionaryPlacementConflict {
        str_id: u64,
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
            Self::DictionaryConflict { str_id } => {
                write!(f, "dictionary id {str_id} maps to conflicting values")
            }
            Self::DictionaryPlacementConflict { str_id } => {
                write!(f, "dictionary id {str_id} occurs in both strings and blobs")
            }
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
            Self::Capacity { .. }
            | Self::DictionaryConflict { .. }
            | Self::DictionaryPlacementConflict { .. }
            | Self::ArithmeticOverflow { .. } => None,
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
    pub(super) dictionary: Vec<(StrId, AdmissionDictionaryValue)>,
    pub(super) descriptors: usize,
    pub(super) string_rows: usize,
    pub(super) string_stored_bytes: usize,
    pub(super) blob_rows: usize,
    pub(super) blob_stored_bytes: usize,
    pub(super) truncated_blob_rows: usize,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct SegmentAdmission {
    pub(super) data_by_type: BTreeMap<u32, DataAdmission>,
    pub(super) dictionary: BTreeMap<StrId, AdmissionDictionaryValue>,
    pub(super) descriptors: usize,
    pub(super) string_rows: usize,
    pub(super) string_stored_bytes: usize,
    pub(super) blob_rows: usize,
    pub(super) blob_stored_bytes: usize,
    pub(super) truncated_blob_rows: usize,
}

impl SegmentAdmission {
    pub(super) fn assess(
        &self,
        summary: &FlushSummary,
        interner: &Interner,
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
        self.assess_dictionary(interner, &mut delta)?;
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

    pub(super) fn assess_dictionary(
        &self,
        interner: &Interner,
        delta: &mut AdmissionDelta,
    ) -> Result<(), AdmissionError> {
        let mut string_rows = self.string_rows;
        let mut string_stored_bytes = self.string_stored_bytes;
        let mut blob_rows = self.blob_rows;
        let mut blob_stored_bytes = self.blob_stored_bytes;
        let mut truncated_blob_rows = self.truncated_blob_rows;
        for entry in interner.window().entries() {
            match self.dictionary.get(&entry.str_id) {
                Some(existing) if existing.placement() != entry.placement => {
                    return Err(AdmissionError::DictionaryPlacementConflict {
                        str_id: entry.str_id.get(),
                    });
                }
                Some(existing) if existing.matches_snapshot(entry) => continue,
                Some(_) => {
                    return Err(AdmissionError::DictionaryConflict {
                        str_id: entry.str_id.get(),
                    });
                }
                None => {}
            }
            let value = AdmissionDictionaryValue::from_snapshot(entry);
            match value.placement() {
                Placement::Strings => {
                    string_rows =
                        string_rows
                            .checked_add(1)
                            .ok_or(AdmissionError::ArithmeticOverflow {
                                resource: "dictionary rows",
                            })?;
                    string_stored_bytes = string_stored_bytes
                        .checked_add(value.stored_len())
                        .ok_or(AdmissionError::ArithmeticOverflow {
                            resource: "dictionary bytes",
                        })?;
                    delta.string_rows += 1;
                    delta.string_stored_bytes = delta
                        .string_stored_bytes
                        .checked_add(value.stored_len())
                        .ok_or(AdmissionError::ArithmeticOverflow {
                            resource: "dictionary bytes",
                        })?;
                    dict::final_dictionary_body_bound(
                        Placement::Strings,
                        string_rows,
                        string_stored_bytes,
                        0,
                    )?;
                }
                Placement::Blobs => {
                    blob_rows =
                        blob_rows
                            .checked_add(1)
                            .ok_or(AdmissionError::ArithmeticOverflow {
                                resource: "dictionary rows",
                            })?;
                    blob_stored_bytes = blob_stored_bytes.checked_add(value.stored_len()).ok_or(
                        AdmissionError::ArithmeticOverflow {
                            resource: "dictionary bytes",
                        },
                    )?;
                    if value.truncated() {
                        truncated_blob_rows = truncated_blob_rows.checked_add(1).ok_or(
                            AdmissionError::ArithmeticOverflow {
                                resource: "truncated dictionary rows",
                            },
                        )?;
                        delta.truncated_blob_rows += 1;
                    }
                    delta.blob_rows += 1;
                    delta.blob_stored_bytes = delta
                        .blob_stored_bytes
                        .checked_add(value.stored_len())
                        .ok_or(AdmissionError::ArithmeticOverflow {
                            resource: "dictionary bytes",
                        })?;
                    dict::final_dictionary_body_bound(
                        Placement::Blobs,
                        blob_rows,
                        blob_stored_bytes,
                        truncated_blob_rows,
                    )?;
                }
            }
            delta.dictionary.push((entry.str_id, value));
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
        for (str_id, value) in delta.dictionary {
            self.dictionary.insert(str_id, value);
        }
        self.descriptors = self.descriptors.saturating_add(delta.descriptors);
        self.string_rows = self.string_rows.saturating_add(delta.string_rows);
        self.string_stored_bytes = self
            .string_stored_bytes
            .saturating_add(delta.string_stored_bytes);
        self.blob_rows = self.blob_rows.saturating_add(delta.blob_rows);
        self.blob_stored_bytes = self
            .blob_stored_bytes
            .saturating_add(delta.blob_stored_bytes);
        self.truncated_blob_rows = self
            .truncated_blob_rows
            .saturating_add(delta.truncated_blob_rows);
    }
}
