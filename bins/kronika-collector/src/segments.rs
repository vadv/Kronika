use crate::config::Config;
use crate::logging::{
    LogLevel, duration_ms, field, log_event, log_flush_summary, log_journal_append, summary_rows,
};
use anyhow::{Context, Result};
use kronika_format::{EntrySnapshot, Placement, StrId};
use kronika_layout::{FileKind, LayoutError, SegmentAddress, SegmentId, WriterOwner};
use kronika_registry::{
    CodecError, DICT_BLOBS_TYPE_ID, DICT_STRINGS_TYPE_ID, MAX_SECTION_ROWS, final_data_body_bound,
};
use kronika_writer::{
    FlushSummary, FlushedPart, Interner, Journal, JournalConfig, JournalError, SectionBuffers,
    dict, write_segment,
};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
enum AdmissionDictionaryValue {
    String(Vec<u8>),
    Blob {
        bytes: Vec<u8>,
        full_len: u64,
        truncated: bool,
        full_sha256: Option<[u8; 32]>,
    },
}

impl AdmissionDictionaryValue {
    fn from_snapshot(entry: EntrySnapshot<'_>) -> Self {
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

    fn matches_snapshot(&self, entry: EntrySnapshot<'_>) -> bool {
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

    const fn placement(&self) -> Placement {
        match self {
            Self::String(_) => Placement::Strings,
            Self::Blob { .. } => Placement::Blobs,
        }
    }

    const fn stored_len(&self) -> usize {
        match self {
            Self::String(bytes) | Self::Blob { bytes, .. } => bytes.len(),
        }
    }

    const fn truncated(&self) -> bool {
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
enum AdmissionError {
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
    const fn is_capacity(&self) -> bool {
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
struct DataAdmission {
    rows: usize,
    list_i32_child_values: usize,
}

#[derive(Debug, Default)]
struct AdmissionDelta {
    data_by_type: BTreeMap<u32, DataAdmission>,
    dictionary: Vec<(StrId, AdmissionDictionaryValue)>,
    descriptors: usize,
    string_rows: usize,
    string_stored_bytes: usize,
    blob_rows: usize,
    blob_stored_bytes: usize,
    truncated_blob_rows: usize,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct SegmentAdmission {
    data_by_type: BTreeMap<u32, DataAdmission>,
    dictionary: BTreeMap<StrId, AdmissionDictionaryValue>,
    descriptors: usize,
    string_rows: usize,
    string_stored_bytes: usize,
    blob_rows: usize,
    blob_stored_bytes: usize,
    truncated_blob_rows: usize,
}

impl SegmentAdmission {
    fn assess(
        &self,
        summary: &FlushSummary,
        interner: &Interner,
    ) -> std::result::Result<AdmissionDelta, AdmissionError> {
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

    fn assess_data(
        &self,
        summary: &FlushSummary,
        delta: &mut AdmissionDelta,
    ) -> std::result::Result<(), AdmissionError> {
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

    fn assess_dictionary(
        &self,
        interner: &Interner,
        delta: &mut AdmissionDelta,
    ) -> std::result::Result<(), AdmissionError> {
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

    fn commit(&mut self, delta: AdmissionDelta) {
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

/// The open (not yet finished) segment: its file name comes from the first
/// window's timestamp, its age from the moment that window was appended.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct SegmentState {
    first_id: Option<SegmentId>,
    opened_at: Option<Instant>,
    admission: SegmentAdmission,
    published_pending_reset: bool,
}

impl SegmentState {
    /// Register the appended window; the first one opens the segment.
    pub(crate) const fn on_window_appended(&mut self, id: SegmentId, now: Instant) {
        if self.first_id.is_none() {
            self.first_id = Some(id);
            self.opened_at = Some(now);
        }
    }

    /// Whether the open segment has reached `max_age`.
    pub(crate) fn age_expired(&self, now: Instant, max_age: Duration) -> bool {
        self.opened_at
            .is_some_and(|opened| now.duration_since(opened) >= max_age)
    }

    pub(crate) fn time_until_age(&self, now: Instant, max_age: Duration) -> Option<Duration> {
        Some(max_age.saturating_sub(now.saturating_duration_since(self.opened_at?)))
    }

    pub(crate) fn ensure_append_allowed(&self) -> Result<()> {
        anyhow::ensure!(
            !self.published_pending_reset,
            "a ZMS was published but active.wal was not reset; restart recovery is required"
        );
        Ok(())
    }

    pub(crate) const fn requires_restart(&self) -> bool {
        self.published_pending_reset
    }

    #[cfg(test)]
    pub(crate) const fn first_ts(&self) -> Option<i64> {
        match self.first_id {
            Some(id) => Some(id.get()),
            None => None,
        }
    }
}

/// Why the open segment must write now, or `None` to keep collecting.
///
/// Forced ticks write immediately, `max_bytes = 0` selects one segment per
/// collection window, and otherwise the raw journal size or segment age closes
/// the segment.
pub(crate) const fn close_reason(
    forced: bool,
    journal_bytes: usize,
    max_bytes: u64,
    age_expired: bool,
) -> Option<&'static str> {
    if forced {
        Some("forced")
    } else if max_bytes == 0 {
        Some("tick")
    } else if journal_bytes as u64 >= max_bytes {
        Some("size")
    } else if age_expired {
        Some("age")
    } else {
        None
    }
}

/// Encode the buffered window into one journal-ready part.
pub(crate) fn encode_window(
    mut buffers: SectionBuffers,
    interner: &Interner,
) -> Result<FlushedPart> {
    let started = Instant::now();
    let dict_sections = dict::encode(interner.window()).context("encode the segment dictionary")?;
    let flushed = buffers
        .flush_with_summary(&dict_sections)
        .context("encode the collection window")?
        .context("a buffered row must yield a part")?;
    log_flush_summary(&flushed.summary, started.elapsed());
    Ok(flushed)
}

/// Write the open segment into its first window's canonical UTC path and reset
/// the journal.
pub(crate) fn close_open_segment(
    journal: &mut Journal,
    owner: &WriterOwner,
    segment: &mut SegmentState,
    reason: &'static str,
) -> Result<PathBuf> {
    close_open_segment_with_reset(journal, owner, segment, reason, Journal::reset)
}

pub(crate) fn close_open_segment_with_reset<F>(
    journal: &mut Journal,
    owner: &WriterOwner,
    segment: &mut SegmentState,
    reason: &'static str,
    reset: F,
) -> Result<PathBuf>
where
    F: FnOnce(&mut Journal) -> Result<(), JournalError>,
{
    let segment_id = segment
        .first_id
        .context("writing an open segment requires an appended window")?;
    let address = SegmentAddress::new(segment_id).context("derive the segment UTC address")?;
    let dest = owner.root().diagnostic_file_path(address, FileKind::Zms);
    let journal_bytes = journal.bytes();
    let journal_parts = journal.parts().len();
    let started = Instant::now();
    let summary = write_segment(journal, owner, address).context("write the segment")?;
    log_event(
        LogLevel::Info,
        "segment_write_finish",
        &[
            field("segment_path", dest.display()),
            field("segment_id", segment_id.get()),
            field("reason", reason),
            field("sections", summary.sections),
            field("segment_bytes", summary.bytes),
            field("journal_bytes", journal_bytes),
            field("journal_parts", journal_parts),
            field("min_ts", summary.min_ts),
            field("max_ts", summary.max_ts),
            field("elapsed_ms", duration_ms(started.elapsed())),
        ],
    );
    // Leave active.wal intact if write_segment() fails.
    segment.published_pending_reset = true;
    reset(journal).context("reset the journal after the segment write")?;
    *segment = SegmentState::default();
    Ok(dest)
}

/// Open the journal under the output directory and write out windows a
/// previous process left behind, so a restart loses no collected data.
///
/// A journal this build cannot read is moved aside and a fresh one takes its
/// place. Collection continues; the damaged bytes stay on disk for an operator
/// to look at.
pub(crate) fn open_collector_journal(
    owner: &WriterOwner,
    journal_max_bytes: u64,
) -> Result<(Journal, Option<PathBuf>)> {
    let config = JournalConfig {
        max_journal_len: usize::try_from(journal_max_bytes)
            .context("KRONIKA_JOURNAL_MAX_BYTES exceeds usize")?,
        ..JournalConfig::default()
    };
    match Journal::open(owner, config) {
        Ok(journal) if journal.parts().is_empty() => Ok((journal, None)),
        Ok(mut journal) => match write_recovered_journal(&mut journal, owner) {
            Ok(dest) => Ok((journal, dest)),
            Err(error) => {
                drop(journal);
                Ok((
                    set_aside_and_reopen(owner, config, &error.to_string())?,
                    None,
                ))
            }
        },
        Err(error) if localized_journal_error(&error) => Ok((
            set_aside_and_reopen(owner, config, &error.to_string())?,
            None,
        )),
        Err(error) => Err(error).context("open the journal"),
    }
}

/// Move the unreadable journal aside, log one line, and open a fresh one.
fn set_aside_and_reopen(
    owner: &WriterOwner,
    config: JournalConfig,
    reason: &str,
) -> Result<Journal> {
    let path = owner
        .set_aside_damaged_journal()
        .context("move the damaged journal aside")?;
    log_event(
        LogLevel::Warn,
        "journal_damaged",
        &[
            field("path", path.display().to_string()),
            field("reason", reason),
        ],
    );
    Journal::open(owner, config).context("open a fresh journal")
}

const fn localized_journal_error(error: &JournalError) -> bool {
    matches!(
        error,
        JournalError::JournalTooLarge { .. }
            | JournalError::TooManyParts { .. }
            | JournalError::UnsupportedJournalFormat
            | JournalError::TornHeader { .. }
            | JournalError::InvalidHeader(_)
            | JournalError::BodyLengthMismatch { .. }
            | JournalError::EmptyWithFrames { .. }
            | JournalError::ActiveWithoutFirstFrame
            | JournalError::DamagedBody { .. }
            | JournalError::InvalidSegmentId(_)
            | JournalError::InvalidPart(_)
            | JournalError::Layout(
                LayoutError::SymlinkNotAllowed { .. }
                    | LayoutError::UnexpectedRootEntryType { .. }
                    | LayoutError::UnexpectedRootEntry { .. }
                    | LayoutError::ActiveJournalMissing
            )
    )
}

/// Write recovered windows under the exact identity persisted in journal v1.
///
/// Parts without a data timestamp hold no rows (a dictionary needs a data
/// section to be referenced from), so a journal made only of those is reset
/// without producing a segment.
fn write_recovered_journal(journal: &mut Journal, owner: &WriterOwner) -> Result<Option<PathBuf>> {
    let segment_id = journal
        .segment_id()
        .context("an active journal must carry SegmentId")?;
    let mut has_data = false;
    for part in journal.parts().to_vec() {
        let body = journal.read_part(part).context("read a recovered part")?;
        let catalog = kronika_format::validate_part(&body).context("validate a recovered part")?;
        if catalog.entries.is_empty() {
            continue;
        }
        if catalog.min_ts == i64::MAX || catalog.max_ts == i64::MIN {
            anyhow::bail!(
                "recovered part has populated sections but no data timestamp; active.wal is preserved"
            );
        }
        has_data = true;
    }
    if !has_data {
        journal
            .reset()
            .context("reset a recovered journal with no data windows")?;
        log_event(
            LogLevel::Info,
            "journal_recovery_empty",
            &[
                field("journal_bytes", journal.bytes()),
                field("journal_parts", journal.parts().len()),
                field("reason", "no_sections"),
            ],
        );
        return Ok(None);
    }
    let address = SegmentAddress::new(segment_id).context("derive the recovered UTC address")?;
    let dest = owner.root().diagnostic_file_path(address, FileKind::Zms);
    let journal_bytes = journal.bytes();
    let journal_parts = journal.parts().len();
    let started = Instant::now();
    let summary = write_segment(journal, owner, address).context("write the recovered segment")?;
    log_event(
        LogLevel::Info,
        "segment_write_finish",
        &[
            field("segment_path", dest.display()),
            field("segment_id", segment_id.get()),
            field("reason", "recovered"),
            field("sections", summary.sections),
            field("segment_bytes", summary.bytes),
            field("journal_bytes", journal_bytes),
            field("journal_parts", journal_parts),
            field("min_ts", summary.min_ts),
            field("max_ts", summary.max_ts),
            field("elapsed_ms", duration_ms(started.elapsed())),
        ],
    );
    journal
        .reset()
        .context("reset the journal after the recovered segment write")?;
    Ok(Some(dest))
}

fn prepare_window_admission(
    journal: &mut Journal,
    owner: &WriterOwner,
    segment: &mut SegmentState,
    flushed: &FlushedPart,
    interner: &Interner,
    finished: &mut Vec<(PathBuf, &'static str)>,
) -> Result<AdmissionDelta> {
    match segment.admission.assess(&flushed.summary, interner) {
        Ok(delta) => Ok(delta),
        Err(err) if err.is_capacity() && segment.first_id.is_some() => {
            // Prove that the incoming window fits by itself before publishing
            // and resetting the accumulated journal. An intrinsically
            // inadmissible window must leave active.wal untouched.
            let fresh = SegmentAdmission::default()
                .assess(&flushed.summary, interner)
                .context("one collection window exceeds finished segment limits")?;
            log_event(
                LogLevel::Warn,
                "segment_admission_full",
                &[
                    field("journal_bytes", journal.bytes()),
                    field("journal_parts", journal.parts().len()),
                    field("error", &err),
                ],
            );
            finished.push((
                close_open_segment(journal, owner, segment, "format-limit")?,
                "format-limit",
            ));
            Ok(fresh)
        }
        Err(err) => Err(err).context("reject the window before journal append"),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "one transaction keeps admission, journal append, early write, and SegmentId state synchronized"
)]
pub(crate) fn append_window_and_maybe_close(
    journal: &mut Journal,
    owner: &WriterOwner,
    config: &Config,
    segment: &mut SegmentState,
    ts: i64,
    forced: bool,
    flushed: &FlushedPart,
    interner: &Interner,
) -> Result<Vec<(PathBuf, &'static str)>> {
    segment.ensure_append_allowed()?;
    let mut finished = Vec::new();
    let mut admission =
        prepare_window_admission(journal, owner, segment, flushed, interner, &mut finished)?;
    let segment_id = match segment.first_id {
        Some(segment_id) => segment_id,
        None => SegmentId::new(ts).context("collection timestamp is outside the layout range")?,
    };
    let append_started = Instant::now();
    let journal_bytes_before = journal.bytes();
    match journal.append(segment_id, &flushed.body) {
        Ok(part_ref) => log_journal_append(
            &flushed.summary,
            part_ref.offset(),
            part_ref.len(),
            journal_bytes_before,
            journal.bytes(),
            append_started.elapsed(),
            false,
        ),
        Err(JournalError::Full { len, max }) if segment.first_id.is_some() => {
            let fresh = SegmentAdmission::default()
                .assess(&flushed.summary, interner)
                .context("one collection window exceeds finished segment limits")?;
            log_event(
                LogLevel::Warn,
                "journal_full",
                &[
                    field("journal_bytes", len),
                    field("journal_max_bytes", max),
                    field("part_bytes", flushed.summary.part_bytes),
                    field("sections", flushed.summary.sections.len()),
                    field("section_rows", summary_rows(&flushed.summary)),
                ],
            );
            finished.push((
                close_open_segment(journal, owner, segment, "journal-full")?,
                "journal-full",
            ));
            admission = fresh;
            let retry_started = Instant::now();
            let journal_bytes_before = journal.bytes();
            let part_ref = journal
                .append(
                    SegmentId::new(ts)
                        .context("collection timestamp is outside the layout range")?,
                    &flushed.body,
                )
                .context("append the window after an early close")?;
            log_journal_append(
                &flushed.summary,
                part_ref.offset(),
                part_ref.len(),
                journal_bytes_before,
                journal.bytes(),
                retry_started.elapsed(),
                true,
            );
        }
        Err(other) => {
            log_event(
                LogLevel::Error,
                "journal_append_failure",
                &[
                    field("part_bytes", flushed.summary.part_bytes),
                    field("sections", flushed.summary.sections.len()),
                    field("section_rows", summary_rows(&flushed.summary)),
                    field("journal_bytes_before", journal_bytes_before),
                    field("error", &other),
                    field("elapsed_ms", duration_ms(append_started.elapsed())),
                ],
            );
            return Err(anyhow::Error::new(other).context("append the part to the journal"));
        }
    }
    segment.admission.commit(admission);
    let now = Instant::now();
    let active_id = journal
        .segment_id()
        .context("a successful journal append must persist SegmentId")?;
    segment.on_window_appended(active_id, now);
    let age = Duration::from_secs(config.segment_max_age_secs);
    if let Some(reason) = close_reason(
        forced,
        journal.bytes(),
        config.segment_max_bytes,
        segment.age_expired(now, age),
    ) {
        finished.push((close_open_segment(journal, owner, segment, reason)?, reason));
    }
    Ok(finished)
}

#[cfg(test)]
mod admission_tests {
    use std::fs;
    use std::path::Path;

    use kronika_format::{
        DictLimits, FRAME_HEADER_LEN, JOURNAL_HEADER_LEN, PartMeta, RESET_MARKER_LEN, SectionInput,
        build_part, validate_part,
    };
    use kronika_layout::{
        DataRoot, FileKind, LayoutLimits, SegmentAddress, SegmentId, WriterOwner,
    };
    use kronika_registry::os_loadavg::OsLoadavg;
    use kronika_registry::{
        CodecError, FINAL_DATA_PAGE_BYTES, MAX_SECTION_ROWS, Section, Ts, final_data_body_bound,
    };
    use kronika_writer::{
        FlushSummary, FlushedPart, Interner, Journal, JournalConfig, SectionBuffers,
        SectionFlushSummary,
    };

    use crate::config::Config;
    use crate::scheduler::Intervals;

    use super::{
        AdmissionError, DataAdmission, SegmentAdmission, SegmentState,
        append_window_and_maybe_close, write_recovered_journal,
    };

    fn data_summary(type_id: u32, rows: usize, list_i32_child_value_count: usize) -> FlushSummary {
        FlushSummary {
            sections: vec![SectionFlushSummary {
                type_id,
                rows: u32::try_from(rows).expect("test row count fits u32"),
                body_bytes: 1,
                list_i32_child_value_count,
            }],
            part_bytes: 1,
        }
    }

    fn empty_interner() -> Interner {
        Interner::new(DictLimits::new(8, 64).expect("test dictionary limits are valid"))
    }

    fn interner(blob_threshold: usize, value: &[u8]) -> Interner {
        let mut interner =
            Interner::new(DictLimits::new(blob_threshold, 64).expect("valid limits"));
        interner.intern(value).expect("test value interns");
        interner
    }

    fn loadavg(ts: i64) -> OsLoadavg {
        OsLoadavg {
            ts: Ts(ts),
            load1: 1.5,
            load5: 1.0,
            load15: 0.5,
            running: 2,
            total: 345,
            scope: 0,
        }
    }

    fn flushed_window(ts: i64) -> FlushedPart {
        let mut buffers = SectionBuffers::new();
        buffers.push(loadavg(ts)).expect("one row fits");
        buffers
            .flush_with_summary(&[])
            .expect("window encodes")
            .expect("one row yields one part")
    }

    fn test_config(out_dir: &Path) -> Config {
        Config {
            out_dir: out_dir.to_path_buf(),
            tick_secs: 5,
            intervals: Intervals::default(),
            segment_max_bytes: u64::MAX,
            segment_max_age_secs: u64::MAX,
            journal_max_bytes: u64::MAX,
            retention: None,
        }
    }

    fn open_journal(root_path: &Path, max_journal_len: usize) -> (WriterOwner, Journal) {
        let root = DataRoot::open(root_path).expect("open test data root");
        let owner = root
            .acquire_writer(LayoutLimits::default())
            .expect("acquire test writer");
        let journal = Journal::open(
            &owner,
            JournalConfig {
                max_journal_len,
                ..JournalConfig::default()
            },
        )
        .expect("open test journal");
        (owner, journal)
    }

    fn one_part_journal_cap(part_len: usize) -> usize {
        JOURNAL_HEADER_LEN + FRAME_HEADER_LEN + part_len + RESET_MARKER_LEN
    }

    fn segment_path(owner: &WriterOwner, ts: i64) -> std::path::PathBuf {
        let id = SegmentId::new(ts).expect("test timestamp is a valid segment id");
        let address = SegmentAddress::new(id).expect("test segment has a UTC address");
        owner.root().diagnostic_file_path(address, FileKind::Zms)
    }

    fn max_admitted_rows(type_id: u32) -> usize {
        let mut low = 0;
        let mut high = MAX_SECTION_ROWS + 1;
        while low + 1 < high {
            let middle = low + (high - low) / 2;
            if final_data_body_bound(type_id, middle, 0).is_ok() {
                low = middle;
            } else {
                high = middle;
            }
        }
        assert!(low > 0, "the test type admits at least one row");
        assert!(final_data_body_bound(type_id, low, 0).is_ok());
        assert!(final_data_body_bound(type_id, low + 1, 0).is_err());
        low
    }

    #[test]
    fn admission_deduplicates_exact_dictionary_values() {
        let type_id = OsLoadavg::CONTRACT.type_id.get();
        let first = interner(8, b"same");
        let second = interner(8, b"same");
        let mut admission = SegmentAdmission::default();

        let delta = admission
            .assess(&data_summary(type_id, 1, 0), &first)
            .expect("first window fits");
        admission.commit(delta);
        let bytes_after_first = admission.string_stored_bytes;
        let delta = admission
            .assess(&data_summary(type_id, 1, 0), &second)
            .expect("the repeated value and second data row fit");
        assert!(
            delta.dictionary.is_empty(),
            "the duplicate adds no dictionary row"
        );
        admission.commit(delta);

        assert_eq!(admission.dictionary.len(), 1);
        assert_eq!(admission.string_rows, 1, "the repeated id counts once");
        assert_eq!(admission.string_stored_bytes, bytes_after_first);
    }

    #[test]
    fn admission_rejects_cross_dictionary_placement() {
        let strings = interner(8, b"same");
        let blobs = interner(1, b"same");
        let mut admission = SegmentAdmission::default();
        let empty = FlushSummary {
            sections: Vec::new(),
            part_bytes: 0,
        };

        let delta = admission.assess(&empty, &strings).expect("string fits");
        admission.commit(delta);
        assert!(matches!(
            admission.assess(&empty, &blobs),
            Err(AdmissionError::DictionaryPlacementConflict { .. })
        ));
    }

    #[test]
    fn dictionary_plain_budgets_are_independent_per_placement() {
        let mut admission = SegmentAdmission {
            string_rows: 1,
            string_stored_bytes: FINAL_DATA_PAGE_BYTES - 5,
            ..SegmentAdmission::default()
        };
        let summary = FlushSummary {
            sections: Vec::new(),
            part_bytes: 0,
        };
        let blob = interner(1, b"blob");
        let delta = admission
            .assess(&summary, &blob)
            .expect("a full strings value page does not consume the blobs value page");
        admission.commit(delta);

        let string = interner(8, b"new");
        assert!(matches!(
            admission.assess(&summary, &string),
            Err(AdmissionError::Codec(CodecError::PlainPageTooLarge {
                name: "bytes",
                ..
            }))
        ));
    }

    #[test]
    fn admission_projects_section_descriptors_against_the_format_cap() {
        let type_id = OsLoadavg::CONTRACT.type_id.get();
        let interner = empty_interner();
        let admission = SegmentAdmission {
            descriptors: MAX_SECTION_ROWS,
            ..SegmentAdmission::default()
        };
        assert!(matches!(
            admission.assess(&data_summary(type_id, 1, 0), &interner),
            Err(AdmissionError::Capacity {
                resource: "section descriptors",
                projected,
                max: MAX_SECTION_ROWS,
            }) if projected == MAX_SECTION_ROWS + 1
        ));
    }

    #[test]
    fn format_capacity_crossing_writes_accumulated_segment_before_append() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (owner, mut journal) =
            open_journal(dir.path(), JournalConfig::default().max_journal_len);
        let config = test_config(dir.path());
        let interner = empty_interner();
        let mut segment = SegmentState::default();
        let first = flushed_window(100);
        let incoming = flushed_window(200);

        assert!(
            append_window_and_maybe_close(
                &mut journal,
                &owner,
                &config,
                &mut segment,
                100,
                false,
                &first,
                &interner,
            )
            .expect("append first")
            .is_empty()
        );
        let type_id = first.summary.sections[0].type_id;
        segment
            .admission
            .data_by_type
            .get_mut(&type_id)
            .expect("first row was admitted")
            .rows = max_admitted_rows(type_id);

        let finished = append_window_and_maybe_close(
            &mut journal,
            &owner,
            &config,
            &mut segment,
            200,
            false,
            &incoming,
            &interner,
        )
        .expect("capacity crossing writes and retries");

        assert_eq!(finished.len(), 1);
        assert_eq!(finished[0].1, "format-limit");
        let old = fs::read(&finished[0].0).expect("read finished old segment");
        let old_catalog = validate_part(&old).expect("old segment is canonical");
        assert_eq!(old_catalog.entries.len(), 1);
        assert_eq!(old_catalog.entries[0].rows, 1);
        assert_eq!(old_catalog.min_ts, 100);
        assert_eq!(journal.parts().len(), 1, "incoming window is active");
        let current = journal
            .read_part(journal.parts()[0])
            .expect("read current part");
        let current_catalog = validate_part(&current).expect("current part is valid");
        assert_eq!(current_catalog.min_ts, 200);
        assert_eq!(segment.first_ts(), Some(200));
        assert_eq!(
            segment.admission.data_by_type.get(&type_id),
            Some(&DataAdmission {
                rows: 1,
                list_i32_child_values: 0,
            })
        );
    }

    #[test]
    fn intrinsically_oversized_window_preserves_active_journal_and_admission() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("active.wal");
        let (owner, mut journal) =
            open_journal(dir.path(), JournalConfig::default().max_journal_len);
        let config = test_config(dir.path());
        let interner = empty_interner();
        let mut segment = SegmentState::default();
        let first = flushed_window(100);
        append_window_and_maybe_close(
            &mut journal,
            &owner,
            &config,
            &mut segment,
            100,
            false,
            &first,
            &interner,
        )
        .expect("append first");
        let bytes_before = fs::read(&path).expect("snapshot active.wal");
        let state_before = segment.clone();
        let mut oversized = flushed_window(200);
        oversized.summary.sections[0].rows =
            u32::try_from(MAX_SECTION_ROWS + 1).expect("row count fits u32");

        let err = append_window_and_maybe_close(
            &mut journal,
            &owner,
            &config,
            &mut segment,
            200,
            false,
            &oversized,
            &interner,
        )
        .expect_err("one oversized window is rejected");
        assert!(
            format!("{err:#}").contains("one collection window exceeds finished segment limits")
        );
        assert_eq!(fs::read(&path).expect("read active.wal"), bytes_before);
        assert_eq!(segment, state_before);
        assert_eq!(journal.parts().len(), 1);
        assert!(!segment_path(&owner, 100).exists());
    }

    #[test]
    fn journal_full_retry_keeps_only_the_incoming_admission() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(dir.path());
        let interner = empty_interner();
        let mut segment = SegmentState::default();
        let first = flushed_window(100);
        let incoming = flushed_window(200);
        let (owner, mut journal) = open_journal(dir.path(), one_part_journal_cap(first.body.len()));
        append_window_and_maybe_close(
            &mut journal,
            &owner,
            &config,
            &mut segment,
            100,
            false,
            &first,
            &interner,
        )
        .expect("the first frame is exempt from the journal cap");

        let finished = append_window_and_maybe_close(
            &mut journal,
            &owner,
            &config,
            &mut segment,
            200,
            false,
            &incoming,
            &interner,
        )
        .expect("full journal writes and retries");

        assert_eq!(finished.len(), 1);
        assert_eq!(finished[0].1, "journal-full");
        assert_eq!(journal.parts().len(), 1);
        assert_eq!(segment.first_ts(), Some(200));
        assert_eq!(segment.admission.descriptors, 1);
        assert_eq!(
            segment
                .admission
                .data_by_type
                .values()
                .map(|data| data.rows)
                .sum::<usize>(),
            1
        );
    }

    #[test]
    fn invalid_part_at_journal_cap_is_transactional() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("active.wal");
        let config = test_config(dir.path());
        let interner = empty_interner();
        let mut segment = SegmentState::default();
        let first = flushed_window(100);
        let (owner, mut journal) = open_journal(dir.path(), one_part_journal_cap(first.body.len()));
        append_window_and_maybe_close(
            &mut journal,
            &owner,
            &config,
            &mut segment,
            100,
            false,
            &first,
            &interner,
        )
        .expect("append first");
        let bytes_before = fs::read(&path).expect("snapshot active.wal");
        let state_before = segment.clone();
        let invalid = FlushedPart {
            body: b"not a ZMS part".to_vec(),
            summary: flushed_window(200).summary,
        };

        append_window_and_maybe_close(
            &mut journal,
            &owner,
            &config,
            &mut segment,
            200,
            false,
            &invalid,
            &interner,
        )
        .expect_err("invalid incoming part is rejected before a full-journal write");

        assert_eq!(fs::read(&path).expect("read active.wal"), bytes_before);
        assert_eq!(segment, state_before);
        assert_eq!(journal.parts().len(), 1);
        assert!(!segment_path(&owner, 100).exists());
    }

    #[test]
    fn recovery_preserves_a_populated_part_without_a_timestamp() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("active.wal");
        let (owner, mut journal) =
            open_journal(dir.path(), JournalConfig::default().max_journal_len);
        let body = OsLoadavg::encode(&[loadavg(100)]).expect("encode section");
        let part = build_part(
            &[SectionInput {
                type_id: OsLoadavg::CONTRACT.type_id.get(),
                rows: 1,
                body: &body,
            }],
            PartMeta {
                min_ts: i64::MAX,
                max_ts: i64::MIN,
            },
        );
        journal
            .append(SegmentId::new(100).expect("valid recovery identity"), &part)
            .expect("append structurally valid part");
        let bytes_before = fs::read(&path).expect("snapshot active.wal");

        let err = write_recovered_journal(&mut journal, &owner)
            .expect_err("populated sentinel-timestamp part is not empty");

        assert!(format!("{err:#}").contains("active.wal is preserved"));
        assert_eq!(fs::read(&path).expect("read active.wal"), bytes_before);
        assert_eq!(journal.parts().len(), 1);
        assert!(
            fs::read_dir(dir.path())
                .expect("read output directory")
                .all(|entry| {
                    entry.expect("directory entry").path().extension()
                        != Some(std::ffi::OsStr::new("zms"))
                })
        );
    }
}
