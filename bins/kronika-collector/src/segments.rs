use crate::config::Config;
use crate::logging::{
    LogLevel, duration_ms, field, log_event, log_flush_summary, log_journal_append, peak_rss_kib,
    summary_rows,
};
use anyhow::{Context, Result};
use kronika_format::{EntrySnapshot, Placement, StrId};
use kronika_layout::{FileKind, SegmentAddress, SegmentId, WriterOwner};
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

mod admission;
mod open;
mod prefix;

use admission::{AdmissionDelta, SegmentAdmission};
pub(crate) use open::open_collector_journal;

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
            field("rss_kib", peak_rss_kib()),
        ],
    );
    // Leave active.wal intact if write_segment() fails.
    segment.published_pending_reset = true;
    reset(journal).context("reset the journal after the segment write")?;
    *segment = SegmentState::default();
    Ok(dest)
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
mod admission_tests;
