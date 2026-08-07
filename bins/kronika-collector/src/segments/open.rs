//! Opening the journal at startup and moving a damaged one aside.

use super::{
    Context, FileKind, Instant, Journal, JournalConfig, JournalError, LayoutError, LogLevel,
    PathBuf, Result, SegmentAddress, WriterOwner, duration_ms, field, log_event, write_segment,
};

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
pub(super) fn set_aside_and_reopen(
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

pub(super) const fn localized_journal_error(error: &JournalError) -> bool {
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
pub(super) fn write_recovered_journal(
    journal: &mut Journal,
    owner: &WriterOwner,
) -> Result<Option<PathBuf>> {
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
