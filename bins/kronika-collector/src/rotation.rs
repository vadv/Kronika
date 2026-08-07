//! Storage rotation for the `KRONIKA_OUT_DIR` tree.
//!
//! Rotation keeps the whole data tree inside a fixed byte budget or a partition
//! used-fraction target by deleting the oldest replaceable data. It runs after
//! every ZMS publication and on a periodic tick, never deletes the active
//! journal or the newest finished segment, and emits a degradation event when
//! only that non-deletable minimum remains. The tree size is tracked
//! incrementally: a full scan seeds it once at startup, publications and
//! deletions adjust it, and the per-tick check stays scan-free (a scan happens
//! only when the tree is over its target or the hourly recount is due).
//! Every enforcement scan re-seeds the counter, folding in index sidecars
//! the web process published since the previous scan; the hourly recount
//! guarantees that sidecar growth is folded in even when the incremental
//! counter alone never crosses the budget. Writer temporaries count toward the
//! budget too.
//!
//! One pass deletes against a deficit computed once at its start and counts
//! every confirmed removal as reclaimed, because unlinked-but-open files
//! release their blocks only when the last reader closes them; in `auto` mode
//! those bytes stay pending until `statvfs` shows the drop, so held
//! descriptors never trigger deleting extra history.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use kronika_layout::{LayoutLimits, LayoutSnapshot, SegmentAddress, TemporaryKind, WriterOwner};

use crate::config::RetentionConfig;
use crate::logging::{LogLevel, field, log_event};

/// Period of the size re-check independent of publications.
const TICK: Duration = Duration::from_mins(1);

/// Period of the authoritative fixed-mode recount scan. It bounds how long
/// growth invisible to the incremental counter (index sidecars, failed
/// publication stats) can keep the tree over budget without triggering a scan.
const RECOUNT_PERIOD: Duration = Duration::from_hours(1);

/// Live rotation state for one running collector.
pub(crate) struct Rotation {
    config: RetentionConfig,
    limits: LayoutLimits,
    /// Incremental size of every countable file except the active journal,
    /// seeded by the startup scan, adjusted on publications and deletions, and
    /// re-seeded from every enforcement scan (index sidecars appear outside
    /// the collector's publication path, so the counter under-counts between
    /// scans).
    non_journal_bytes: u64,
    /// Instant of the last periodic size re-check.
    last_tick: Instant,
    /// Instant of the last authoritative recount scan.
    last_recount: Instant,
    /// Instant of the last degradation event; throttles it to one per tick.
    last_degradation: Option<Instant>,
    /// `auto` only: bytes logically freed whose physical release `statvfs` has
    /// not shown yet (readers may hold the unlinked files open).
    pending_reclaim: u64,
    /// `auto` only: partition used bytes at the previous observation, the
    /// baseline for crediting physical drops against `pending_reclaim`.
    last_observed_used: Option<u64>,
}

/// One file rotation may delete, in priority order.
enum Victim {
    /// A stale writer ZMS publication temporary.
    Temporary(kronika_layout::TemporaryObject),
    /// An index sidecar with no sibling ZMS.
    OrphanIndex(SegmentAddress),
    /// A finished segment (its ZMS and sibling IDX).
    Segment(SegmentAddress),
}

impl Victim {
    /// Whether this victim's bytes are part of the incremental tree counter.
    ///
    /// Orphan indexes carry no scanned size, so they may not adjust the
    /// counter on removal.
    const fn counted_in_tree(&self) -> bool {
        matches!(self, Self::Temporary(_) | Self::Segment(_))
    }

    const fn log_kind(&self) -> &'static str {
        match self {
            Self::Temporary(_) => "temporary",
            Self::OrphanIndex(_) => "orphan_index",
            Self::Segment(_) => "segment",
        }
    }

    fn log_path(&self) -> String {
        match self {
            Self::Temporary(temporary) => {
                format!("{}/{}", temporary.address.day, temporary.file_name())
            }
            Self::OrphanIndex(address) => format!("{}/{}", address.day, address.idx_name()),
            Self::Segment(address) => format!("{}/{}", address.day, address.zms_name()),
        }
    }

    const fn segment_id(&self) -> i64 {
        match self {
            Self::Temporary(temporary) => temporary.address.id.get(),
            Self::OrphanIndex(address) | Self::Segment(address) => address.id.get(),
        }
    }
}

impl Rotation {
    /// Seeds the incremental size counter from one full scan (the restart
    /// recount) and returns `None` when rotation is disabled.
    ///
    /// # Errors
    ///
    /// Returns an error if the seeding scan fails.
    pub(crate) fn new(
        config: Option<RetentionConfig>,
        owner: &WriterOwner,
        limits: LayoutLimits,
        now: Instant,
    ) -> Result<Option<Self>> {
        let Some(config) = config else {
            return Ok(None);
        };
        let snapshot = owner
            .root()
            .scan(limits)
            .context("scan the tree to seed rotation")?;
        let non_journal_bytes = countable_bytes(&snapshot);
        log_event(
            LogLevel::Info,
            "rotation_seed",
            &[
                field("reason", reason_label(config)),
                field("non_journal_bytes", non_journal_bytes),
                field("segments", snapshot.segments.len()),
            ],
        );
        Ok(Some(Self {
            config,
            limits,
            non_journal_bytes,
            last_tick: now,
            last_recount: now,
            last_degradation: None,
            pending_reclaim: 0,
            last_observed_used: None,
        }))
    }

    /// Duration until the next periodic tick is due.
    pub(crate) fn time_until_tick(&self, now: Instant) -> Duration {
        TICK.saturating_sub(now.saturating_duration_since(self.last_tick))
    }

    /// Records that a publication grew the tree by `bytes`.
    pub(crate) const fn record_publication(&mut self, bytes: u64) {
        self.non_journal_bytes = self.non_journal_bytes.saturating_add(bytes);
    }

    /// Enforces the target if a publication happened or the tick came due.
    pub(crate) fn maybe_enforce(
        &mut self,
        owner: &WriterOwner,
        journal_bytes: u64,
        published: bool,
        now: Instant,
    ) {
        let tick_due = self.time_until_tick(now).is_zero();
        if !published && !tick_due {
            return;
        }
        if tick_due {
            self.last_tick = now;
        }
        if let Err(err) = self.enforce(owner, journal_bytes, now) {
            log_event(
                LogLevel::Warn,
                "rotation_failure",
                &[field("error", format!("{err:#}"))],
            );
        }
    }

    /// One enforcement pass.
    ///
    /// The pass checks the target scan-free, then, only when over the target
    /// or when the hourly recount is due, scans once, re-seeds the counter
    /// from that scan, and deletes the oldest replaceable data against a
    /// deficit computed once for the pass. Confirmed removals count as
    /// reclaimed immediately: physical release may lag while readers hold the
    /// unlinked files open, and re-reading `statvfs` inside the loop would
    /// mistake that lag for "nothing was freed" and delete extra history.
    /// The first removal error ends the pass: the tree state is ambiguous
    /// after a partial mutation, and the next pass rescans from scratch.
    fn enforce(&mut self, owner: &WriterOwner, journal_bytes: u64, now: Instant) -> Result<()> {
        let recount_due = matches!(self.config, RetentionConfig::Fixed(_))
            && now.saturating_duration_since(self.last_recount) >= RECOUNT_PERIOD;
        let (current, threshold) = self.target(owner, journal_bytes)?;
        if current <= threshold && !recount_due {
            return Ok(());
        }
        let snapshot = owner
            .root()
            .scan(self.limits)
            .context("scan the tree for rotation")?;
        // The scan is fresh truth: re-seed the counter so index sidecars
        // published by the web process since the last scan are counted and the
        // per-victim subtraction below stays consistent with the snapshot.
        self.non_journal_bytes = countable_bytes(&snapshot);
        self.last_recount = now;
        let (current, threshold) = self.target(owner, journal_bytes)?;
        let mut deficit = current.saturating_sub(threshold);
        if deficit == 0 {
            return Ok(());
        }

        let mut freed_total: u64 = 0;
        let mut aborted = false;
        for victim in plan_victims(&snapshot) {
            if deficit == 0 {
                break;
            }
            match remove_victim(owner, &victim) {
                Ok(freed) => {
                    if victim.counted_in_tree() {
                        self.non_journal_bytes = self.non_journal_bytes.saturating_sub(freed);
                    }
                    if matches!(self.config, RetentionConfig::Auto(_)) {
                        self.pending_reclaim = self.pending_reclaim.saturating_add(freed);
                    }
                    deficit = deficit.saturating_sub(freed);
                    freed_total = freed_total.saturating_add(freed);
                    self.log_deletion(
                        &victim,
                        freed,
                        current.saturating_sub(freed_total),
                        threshold,
                    );
                }
                Err(err) => {
                    log_event(
                        LogLevel::Warn,
                        "rotation_delete_failure",
                        &[
                            field("path", victim.log_path()),
                            field("error", format!("{err:#}")),
                        ],
                    );
                    aborted = true;
                    break;
                }
            }
        }
        // Degradation asserts the non-deletable minimum was reached; a pass
        // cut short by a removal error proves no such thing.
        if deficit > 0 && !aborted {
            self.log_degradation(current.saturating_sub(freed_total), threshold, now);
        }
        Ok(())
    }

    /// Current size figure and its threshold under the configured target.
    ///
    /// `Fixed` compares the incremental tree counter (journal, segments,
    /// temporaries) against the byte budget. `auto` compares the
    /// partition's used bytes, less the reclaim still pending physical
    /// release, against the percentage threshold; each observation first
    /// credits any physical drop against that pending figure.
    fn target(&mut self, owner: &WriterOwner, journal_bytes: u64) -> Result<(u64, u64)> {
        match self.config {
            RetentionConfig::Fixed(budget) => Ok((self.tree_bytes(journal_bytes), budget)),
            RetentionConfig::Auto(percent) => {
                let usage = owner
                    .root()
                    .filesystem_usage()
                    .context("read partition usage")?;
                self.pending_reclaim = reconcile_pending_reclaim(
                    self.pending_reclaim,
                    self.last_observed_used,
                    usage.used_bytes,
                );
                self.last_observed_used = Some(usage.used_bytes);
                Ok((
                    usage.used_bytes.saturating_sub(self.pending_reclaim),
                    used_threshold_bytes(usage.total_bytes, percent),
                ))
            }
        }
    }

    const fn tree_bytes(&self, journal_bytes: u64) -> u64 {
        self.non_journal_bytes.saturating_add(journal_bytes)
    }

    fn log_deletion(&self, victim: &Victim, freed: u64, current: u64, threshold: u64) {
        let mut fields = vec![
            field("kind", victim.log_kind()),
            field("path", victim.log_path()),
            field("freed_bytes", freed),
            field("reason", reason_label(self.config)),
            field("current_bytes", current),
            field("threshold_bytes", threshold),
        ];
        {
            let segment_id = victim.segment_id();
            fields.push(field("segment_id", segment_id));
        }
        log_event(LogLevel::Info, "rotation_delete", &fields);
    }

    fn log_degradation(&mut self, current: u64, threshold: u64, now: Instant) {
        let throttled = self
            .last_degradation
            .is_some_and(|last| now.saturating_duration_since(last) < TICK);
        if throttled {
            return;
        }
        self.last_degradation = Some(now);
        log_event(
            LogLevel::Warn,
            "rotation_degraded",
            &[
                field("reason", reason_label(self.config)),
                field("current_bytes", current),
                field("threshold_bytes", threshold),
                field(
                    "detail",
                    "minimum viable storage reached; collection continues",
                ),
            ],
        );
    }
}

/// Selects deletion candidates in a fixed order: writer temporaries, then
/// orphan indexes, then finished segments oldest-first. The newest finished
/// segment is never a candidate; together with the active journal it is the
/// non-deletable minimum.
fn plan_victims(snapshot: &LayoutSnapshot) -> Vec<Victim> {
    let mut victims = Vec::new();
    for temporary in &snapshot.temporaries {
        if temporary.kind == TemporaryKind::Zms {
            victims.push(Victim::Temporary(temporary.clone()));
        }
    }
    for orphan in &snapshot.orphan_indexs {
        victims.push(Victim::OrphanIndex(*orphan));
    }
    let deletable_segments = snapshot.segments.len().saturating_sub(1);
    for segment in snapshot.segments.iter().take(deletable_segments) {
        victims.push(Victim::Segment(segment.address));
    }
    victims
}

/// Deletes one victim and returns the bytes it freed.
fn remove_victim(owner: &WriterOwner, victim: &Victim) -> Result<u64> {
    match victim {
        Victim::Temporary(temporary) => {
            let bytes = temporary.identity.len;
            owner
                .remove_temporary(temporary)
                .context("remove a writer temporary")?;
            Ok(bytes)
        }
        Victim::OrphanIndex(address) => owner
            .remove_orphan_index(*address)
            .context("remove an orphan index"),
        Victim::Segment(address) => Ok(owner
            .remove_finished_segment(*address)
            .context("remove a finished segment")?
            .total_bytes()),
    }
}

/// Bytes of every sized file the scan reports: finished segments and writer
/// temporaries. The active journal is excluded (its size is supplied live by
/// the writer's own byte counter), and orphan indexs are excluded because
/// the scan reports no size for them.
fn countable_bytes(snapshot: &LayoutSnapshot) -> u64 {
    let segments = snapshot
        .segments
        .iter()
        .map(|segment| {
            segment
                .zms_bytes
                .saturating_add(segment.idx_bytes.unwrap_or(0))
        })
        .fold(0_u64, u64::saturating_add);
    let temporaries = snapshot
        .temporaries
        .iter()
        .map(|temporary| temporary.identity.len)
        .fold(0_u64, u64::saturating_add);
    segments.saturating_add(temporaries)
}

/// Shrinks the pending logical reclaim by the physical drop the partition has
/// shown since the previous observation.
///
/// Any observed drop is credited to pending reclaim first; a foreign deletion
/// mistaken for ours only makes rotation delete less, never more.
fn reconcile_pending_reclaim(pending: u64, last_observed_used: Option<u64>, used_now: u64) -> u64 {
    let observed_drop = last_observed_used.map_or(0, |last| last.saturating_sub(used_now));
    pending.saturating_sub(observed_drop)
}

/// The byte figure `percent` of `total` maps to.
fn used_threshold_bytes(total: u64, percent: u8) -> u64 {
    u64::try_from(u128::from(total) * u128::from(percent) / 100).unwrap_or(u64::MAX)
}

const fn reason_label(config: RetentionConfig) -> &'static str {
    match config {
        RetentionConfig::Fixed(_) => "budget",
        RetentionConfig::Auto(_) => "auto",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kronika_layout::{FileIdentity, SegmentArtifacts, SegmentId};

    const MICROS_PER_DAY: i64 = 86_400_000_000;

    fn address(id: i64) -> SegmentAddress {
        SegmentAddress::new(SegmentId::new(id).expect("valid segment id")).expect("valid address")
    }

    fn segment(id: i64, zms_bytes: u64, idx_bytes: Option<u64>) -> SegmentArtifacts {
        SegmentArtifacts {
            address: address(id),
            zms_identity: FileIdentity {
                device: 1,
                inode: u64::try_from(id).unwrap_or(0),
                len: zms_bytes,
                mtime_seconds: 0,
                mtime_nanoseconds: 0,
                ctime_seconds: 0,
                ctime_nanoseconds: 0,
            },
            zms_bytes,
            idx_bytes,
        }
    }

    fn snapshot(segments: Vec<SegmentArtifacts>, orphans: Vec<SegmentAddress>) -> LayoutSnapshot {
        LayoutSnapshot {
            days: Vec::new(),
            segments,
            orphan_indexs: orphans,
            temporaries: Vec::new(),
            foreign_entries: Vec::new(),
            visited_entries: 0,
            metadata_bytes: 0,
        }
    }

    #[test]
    fn deficit_is_exact_at_the_percentage_boundary() {
        // 80 of 100 is exactly the 80% threshold: no deficit, one more byte
        // creates one.
        let threshold = used_threshold_bytes(100, 80);
        assert_eq!(80_u64.saturating_sub(threshold), 0);
        assert_eq!(81_u64.saturating_sub(threshold), 1);
        assert_eq!(
            0_u64.saturating_sub(used_threshold_bytes(0, 80)),
            0,
            "an empty partition never has a deficit"
        );
        assert_eq!(
            150_u64.saturating_sub(used_threshold_bytes(100, 99)),
            51,
            "statvfs reporting used above total still yields a full deficit"
        );
    }

    #[test]
    fn reconcile_pending_reclaim_credits_only_observed_drops() {
        assert_eq!(
            reconcile_pending_reclaim(100, None, 500),
            100,
            "the first observation has no baseline to credit against"
        );
        assert_eq!(
            reconcile_pending_reclaim(100, Some(500), 470),
            70,
            "a 30-byte physical drop shrinks the pending figure"
        );
        assert_eq!(
            reconcile_pending_reclaim(100, Some(500), 300),
            0,
            "a drop larger than pending clears it without wrapping"
        );
        assert_eq!(
            reconcile_pending_reclaim(100, Some(500), 600),
            100,
            "foreign growth never inflates pending reclaim"
        );
    }

    #[test]
    fn used_threshold_is_the_percent_of_total() {
        assert_eq!(used_threshold_bytes(1000, 80), 800);
        assert_eq!(used_threshold_bytes(0, 80), 0);
        assert_eq!(
            used_threshold_bytes(199, 80),
            159,
            "the threshold rounds down"
        );
        let expected = u64::try_from(u128::from(u64::MAX) * 99 / 100).expect("fits u64");
        assert_eq!(
            used_threshold_bytes(u64::MAX, 99),
            expected,
            "the figure is exact in u128 even at the type limit"
        );
    }

    #[test]
    fn countable_bytes_sums_zms_and_idx() {
        let snap = snapshot(
            vec![
                segment(1, 100, Some(30)),
                segment(MICROS_PER_DAY, 200, None),
            ],
            Vec::new(),
        );
        assert_eq!(countable_bytes(&snap), 100 + 30 + 200);
    }

    #[test]
    fn countable_bytes_saturates_instead_of_overflowing() {
        let snap = snapshot(
            vec![
                segment(1, u64::MAX, Some(u64::MAX)),
                segment(2, u64::MAX, None),
            ],
            Vec::new(),
        );
        assert_eq!(countable_bytes(&snap), u64::MAX);
    }

    fn bare_rotation(config: RetentionConfig, non_journal_bytes: u64, seeded: Instant) -> Rotation {
        Rotation {
            config,
            limits: LayoutLimits::default(),
            non_journal_bytes,
            last_tick: seeded,
            last_recount: seeded,
            last_degradation: None,
            pending_reclaim: 0,
            last_observed_used: None,
        }
    }

    #[test]
    fn tree_bytes_saturates_instead_of_overflowing() {
        let rotation = bare_rotation(RetentionConfig::Fixed(0), u64::MAX, Instant::now());
        assert_eq!(rotation.tree_bytes(1), u64::MAX);
    }

    #[test]
    fn time_until_tick_counts_down_and_saturates_at_zero() {
        let seeded = Instant::now();
        let rotation = bare_rotation(RetentionConfig::Fixed(0), 0, seeded);
        assert_eq!(rotation.time_until_tick(seeded), TICK);
        assert_eq!(
            rotation.time_until_tick(seeded + TICK / 2),
            TICK / 2,
            "half the period elapsed leaves half the wait"
        );
        assert_eq!(rotation.time_until_tick(seeded + TICK), Duration::ZERO);
        assert_eq!(
            rotation.time_until_tick(seeded + TICK * 3),
            Duration::ZERO,
            "an overdue tick never yields a negative wait"
        );
    }

    #[test]
    fn plan_keeps_the_newest_segment_and_orders_oldest_first() {
        let snap = snapshot(
            vec![
                segment(1, 10, None),
                segment(2, 10, None),
                segment(MICROS_PER_DAY, 10, None),
            ],
            Vec::new(),
        );
        let victims = plan_victims(&snap);
        let ids: Vec<i64> = victims
            .iter()
            .filter_map(|v| match v {
                Victim::Segment(address) => Some(address.id.get()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, vec![1, 2], "oldest two are candidates, newest is kept");
    }

    #[test]
    fn plan_keeps_a_lone_segment() {
        let snap = snapshot(vec![segment(1, 10, None)], Vec::new());
        assert!(
            !plan_victims(&snap)
                .iter()
                .any(|v| matches!(v, Victim::Segment(_))),
            "a single segment is the non-deletable minimum"
        );
    }

    #[test]
    fn plan_orders_orphans_before_segments() {
        let snap = snapshot(
            vec![segment(1, 10, None), segment(2, 10, None)],
            vec![address(MICROS_PER_DAY)],
        );
        let victims = plan_victims(&snap);
        assert!(
            matches!(victims.first(), Some(Victim::OrphanIndex(_))),
            "orphan indexs are reclaimed before finished segments"
        );
    }

    #[test]
    fn only_segments_and_temporaries_adjust_the_tree_counter() {
        assert!(
            Victim::Segment(address(1)).counted_in_tree(),
            "finished segments are seeded and their removal adjusts the counter"
        );
        assert!(
            !Victim::OrphanIndex(address(1)).counted_in_tree(),
            "orphan indexs have no scanned size, so they stay out of the counter"
        );
    }

    #[test]
    fn enforcement_reseeds_the_counter_with_post_seed_indexs() {
        let directory = tempfile::tempdir().expect("create a tempdir");
        let root = kronika_layout::DataRoot::open(directory.path()).expect("open the root");
        let owner = root
            .acquire_writer(LayoutLimits::default())
            .expect("acquire the writer");
        let oldest = address(1);
        for item in [oldest, address(2), address(3)] {
            let mut temp = owner.create_zms_temp(item).expect("create a temporary");
            std::io::Write::write_all(temp.file_mut(), b"ZMSBODY").expect("write the body");
            temp.publish().expect("publish the segment");
        }
        let mut rotation = Rotation::new(
            Some(RetentionConfig::Fixed(1)),
            &owner,
            LayoutLimits::default(),
            Instant::now(),
        )
        .expect("seed rotation")
        .expect("rotation is enabled");
        // A sidecar published by the web process after seeding: the
        // incremental counter has not seen it, only a rescan can.
        let sidecar = directory
            .path()
            .join(oldest.day.to_string())
            .join(oldest.idx_name());
        std::fs::write(&sidecar, b"IDX").expect("write the sidecar");

        rotation
            .enforce(&owner, 0, Instant::now())
            .expect("enforce the budget");

        let recount = countable_bytes(&root.scan(LayoutLimits::default()).expect("rescan"));
        assert_eq!(
            rotation.non_journal_bytes, recount,
            "the counter matches a full recount after deleting a segment with a post-seed sidecar"
        );
        assert_eq!(recount, b"ZMSBODY".len() as u64, "only the newest survives");
    }

    #[test]
    fn hourly_recount_catches_growth_the_counter_never_sees() {
        let directory = tempfile::tempdir().expect("create a tempdir");
        let root = kronika_layout::DataRoot::open(directory.path()).expect("open the root");
        let owner = root
            .acquire_writer(LayoutLimits::default())
            .expect("acquire the writer");
        let oldest = address(1);
        for item in [oldest, address(2), address(3)] {
            let mut temp = owner.create_zms_temp(item).expect("create a temporary");
            std::io::Write::write_all(temp.file_mut(), b"ZMSBODY").expect("write the body");
            temp.publish().expect("publish the segment");
        }
        // Budget 25: the seeded counter (21) stays under it, so the scan-free
        // precheck alone would never fire.
        let mut rotation = Rotation::new(
            Some(RetentionConfig::Fixed(25)),
            &owner,
            LayoutLimits::default(),
            Instant::now(),
        )
        .expect("seed rotation")
        .expect("rotation is enabled");
        // A 10-byte sidecar from the web process pushes the real tree to 31,
        // invisible to the incremental counter.
        let sidecar = directory
            .path()
            .join(oldest.day.to_string())
            .join(oldest.idx_name());
        std::fs::write(&sidecar, b"OVERGROWTH").expect("write the sidecar");

        let now = Instant::now();
        rotation
            .enforce(&owner, 0, now)
            .expect("enforce under the stale counter");
        assert_eq!(
            root.scan(LayoutLimits::default())
                .expect("rescan")
                .segments
                .len(),
            3,
            "without a due recount the stale counter sees no deficit"
        );

        rotation.last_recount = now
            .checked_sub(RECOUNT_PERIOD)
            .expect("the test clock is past the recount period");
        rotation.enforce(&owner, 0, now).expect("enforce recounted");
        let after = root.scan(LayoutLimits::default()).expect("rescan");
        assert_eq!(
            after.segments.len(),
            2,
            "the recount folds the sidecar in and deletes the oldest segment"
        );
        assert_eq!(
            rotation.non_journal_bytes,
            countable_bytes(&after),
            "the counter matches a full recount after the pass"
        );
    }

    #[test]
    fn incremental_counter_converges_with_a_full_recount() {
        // Seed from one tree, apply a publication and a deletion, and confirm the
        // running counter equals a fresh recount of the resulting tree.
        let before = snapshot(
            vec![segment(1, 100, None), segment(2, 200, None)],
            Vec::new(),
        );
        let seed = countable_bytes(&before);

        let published = 150_u64; // a new segment finished
        let deleted = 100_u64; // the oldest segment removed
        let incremental = seed.saturating_add(published).saturating_sub(deleted);

        let after = snapshot(
            vec![segment(2, 200, None), segment(MICROS_PER_DAY, 150, None)],
            Vec::new(),
        );
        assert_eq!(incremental, countable_bytes(&after));
    }
}
