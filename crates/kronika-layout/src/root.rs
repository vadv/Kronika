use std::cell::Cell;
use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io;
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use rustix::fs::RenameFlags;
use rustix::fs::{AtFlags, Dir, FileType, FlockOperation, Mode, OFlags};

use crate::{LayoutError, LimitKind, OwnerKind, SegmentAddress, SegmentId, UtcDay};

/// Root-level active segment journal.
mod entry;
mod fsops;
mod index;
mod names;
mod scan;
mod writer;

pub use entry::{
    EntryDiagnostic, EntryFileType, EntryScope, FileKind, FilesystemUsage, ForeignEntry,
    ForeignEntryReason, LayoutSnapshot, PathIdentity, SegmentArtifacts, SegmentRemoval,
    TemporaryKind, TemporaryObject,
};
pub use index::{IdxTemp, IndexOwner};
pub use writer::{WriterLease, WriterOwner, ZmsTemp};

use entry::EntryParent;
use fsops::{
    create_regular_at, ensure_directory_at, errno_to_io, errno_to_layout, link_open_file,
    open_directory_at, open_or_create_regular, open_regular_at, remove_empty_directory_at,
    remove_regular_if_identity, remove_verified_regular, rename_noreplace, stat_no_follow,
    stat_no_follow_name, sync_source_directory, temporary_name, unlink_named_if_identity,
    unlink_regular_capturing_size, verify_named_identity,
};
use names::{
    ascii_name, foreign_type_reason, hash_name, is_control_name, is_dot, parse_day, parse_leaf,
    parse_month, parse_year, validate_limit, writer_lock_is_poisoned,
};
use scan::{DayArtifacts, ParsedLeaf, ScanState};

/// The raw journal the collector appends to.
pub const ACTIVE_JOURNAL_NAME: &str = "active.wal";
/// Permanent process-ownership lock for the collector.
pub const WRITER_OWNER_LOCK_NAME: &str = ".kronika-writer.owner.lock";
/// Where a journal the writer could not read is moved so collection can
/// continue. At most one exists at a time.
pub const DAMAGED_JOURNAL_NAME: &str = "active.wal.damaged";
/// Permanent process-ownership lock for index publication and GC.
pub const OVERVIEW_OWNER_LOCK_NAME: &str = ".kronika-index.owner.lock";
const HARD_MAX_VISITED_ENTRIES: usize = 4_000_000;
const HARD_MAX_ENTRIES_PER_DAY: usize = 1_000_000;
const HARD_MAX_SEGMENTS: usize = 2_000_000;
const HARD_MAX_METADATA_BYTES: usize = 128 * 1024 * 1024;
const ENTRY_METADATA_BYTES: usize = 128;
const SCAN_RACE_ATTEMPTS: usize = 4;
const WRITER_LOCK_HANDOFF_TIMEOUT: Duration = Duration::from_millis(100);
const DIRECTORY_MODE: Mode = Mode::RUSR
    .union(Mode::WUSR)
    .union(Mode::XUSR)
    .union(Mode::RGRP)
    .union(Mode::XGRP);
const DATA_FILE_MODE: Mode = Mode::RUSR.union(Mode::WUSR).union(Mode::RGRP);
const LOCK_FILE_MODE: Mode = Mode::RUSR.union(Mode::WUSR);

/// Non-zero hard-capped resource limits for one strict tree traversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(
    clippy::struct_field_names,
    reason = "`LayoutLimits::max_*` makes each public bound explicit at call sites"
)]
pub struct LayoutLimits {
    /// Maximum number of entries visited across the entire tree.
    pub max_visited_entries: usize,
    /// Maximum number of entries visited in a single day directory.
    pub max_entries_per_day: usize,
    /// Maximum number of finished ZMS segments returned.
    pub max_segments: usize,
    /// Maximum accounted bytes for names and result metadata.
    pub max_metadata_bytes: usize,
}

impl LayoutLimits {
    /// Validates all limits against their non-zero hard ranges.
    ///
    /// # Errors
    ///
    /// Returns [`LayoutError::InvalidLimits`] for a zero or excessive value.
    pub fn validate(self) -> Result<Self, LayoutError> {
        validate_limit(
            LimitKind::VisitedEntries,
            self.max_visited_entries,
            HARD_MAX_VISITED_ENTRIES,
        )?;
        validate_limit(
            LimitKind::EntriesPerDay,
            self.max_entries_per_day,
            HARD_MAX_ENTRIES_PER_DAY,
        )?;
        validate_limit(LimitKind::Segments, self.max_segments, HARD_MAX_SEGMENTS)?;
        validate_limit(
            LimitKind::MetadataBytes,
            self.max_metadata_bytes,
            HARD_MAX_METADATA_BYTES,
        )?;
        Ok(self)
    }
}

impl Default for LayoutLimits {
    fn default() -> Self {
        Self {
            max_visited_entries: 1_000_000,
            max_entries_per_day: 10_000,
            max_segments: 500_000,
            max_metadata_bytes: HARD_MAX_METADATA_BYTES,
        }
    }
}

/// A validated, already-open data-directory root.
///
/// Every component is opened relative to this descriptor without following a
/// symbolic link, so a path swapped underneath cannot redirect the writer.
#[derive(Debug, Clone)]
pub struct DataRoot {
    directory: Arc<File>,
    diagnostic_path: Arc<Path>,
}

impl DataRoot {
    /// Opens an existing data root without following a symbolic link.
    ///
    /// This only opens the root itself. Call [`scan`](Self::scan) before using
    /// its contents.
    ///
    /// # Errors
    ///
    /// Returns a structural or filesystem error when the root cannot be opened
    /// as a real directory.
    pub fn open(path: &Path) -> Result<Self, LayoutError> {
        let directory = rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map(File::from)
        .map_err(|error| {
            if error == rustix::io::Errno::LOOP {
                LayoutError::SymlinkNotAllowed {
                    name: path.display().to_string(),
                }
            } else {
                LayoutError::Io(errno_to_io(error))
            }
        })?;
        Ok(Self {
            directory: Arc::new(directory),
            diagnostic_path: Arc::from(path),
        })
    }

    /// Returns the configured root path for diagnostics only.
    ///
    /// Filesystem access should use this type's descriptor-relative methods.
    #[must_use]
    pub fn diagnostic_path(&self) -> &Path {
        &self.diagnostic_path
    }

    /// Builds a path for logs, error reports, and test assertions only.
    ///
    /// The returned path is not a capability and production I/O must use
    /// [`open_zms`](Self::open_zms), [`open_idx`](Self::open_idx), or an owner
    /// token.
    #[must_use]
    pub fn diagnostic_file_path(&self, address: SegmentAddress, kind: FileKind) -> PathBuf {
        let name = match kind {
            FileKind::Zms => address.zms_name(),
            FileKind::Idx => address.idx_name(),
        };
        self.diagnostic_path
            .join(address.day.year_component())
            .join(address.day.month_component())
            .join(address.day.day_component())
            .join(name)
    }

    /// Performs a tolerant, closed-grammar, three-level, bounded traversal.
    ///
    /// Entries outside the owned-store grammar are classified in
    /// [`LayoutSnapshot::foreign_entries`] and never followed or traversed.
    /// Valid entries remain in the returned inventory. Resource exhaustion and
    /// failures while reading the verified tree still fail the whole scan.
    ///
    /// # Errors
    ///
    /// Returns [`LayoutError`] for exhausted limits or filesystem failures
    /// that prevent bounded traversal of the verified tree.
    pub fn scan(&self, limits: LayoutLimits) -> Result<LayoutSnapshot, LayoutError> {
        let limits = limits.validate()?;
        for attempt in 0..SCAN_RACE_ATTEMPTS {
            // Fresh per attempt: a retried scan revisits the same entries, and
            // carrying the count over would fail valid trees near the limit.
            let mut visited_entries = 0_usize;
            match self.scan_once(limits, &mut visited_entries) {
                Err(LayoutError::Io(error))
                    if error.kind() == io::ErrorKind::NotFound
                        && attempt + 1 < SCAN_RACE_ATTEMPTS =>
                {
                    std::thread::yield_now();
                }
                result => return result,
            }
        }
        unreachable!("the bounded scan loop always returns on its final attempt")
    }

    fn scan_once(
        &self,
        limits: LayoutLimits,
        visited_entries: &mut usize,
    ) -> Result<LayoutSnapshot, LayoutError> {
        let mut state = ScanState::new(limits, visited_entries);
        let mut root_entries = Dir::read_from(&*self.directory).map_err(errno_to_layout)?;
        for entry in &mut root_entries {
            let entry = entry.map_err(errno_to_layout)?;
            let name = entry.file_name();
            let name_bytes = name.to_bytes();
            if is_dot(name_bytes) {
                continue;
            }
            state.account(name_bytes.len())?;
            let stat = stat_no_follow_name(&self.directory, name)?;
            let file_type = FileType::from_raw_mode(stat.st_mode);
            let Some(name_string) = ascii_name(name_bytes) else {
                state.record_foreign(
                    EntryParent::Root,
                    name,
                    &stat,
                    ForeignEntryReason::NonAsciiName,
                )?;
                continue;
            };
            if is_control_name(name_string) {
                if file_type != FileType::RegularFile {
                    state.record_foreign(
                        EntryParent::Root,
                        name,
                        &stat,
                        foreign_type_reason(file_type),
                    )?;
                }
                continue;
            }
            let root_extension = Path::new(name_string).extension();
            if root_extension.is_some_and(|extension| extension == "zms" || extension == "idx") {
                state.record_foreign(
                    EntryParent::Root,
                    name,
                    &stat,
                    if file_type == FileType::RegularFile {
                        ForeignEntryReason::UnsupportedFlatArtifact
                    } else {
                        foreign_type_reason(file_type)
                    },
                )?;
                continue;
            }
            let Some(year) = parse_year(name_string) else {
                state.record_foreign(
                    EntryParent::Root,
                    name,
                    &stat,
                    if file_type == FileType::Symlink {
                        ForeignEntryReason::SymbolicLink
                    } else {
                        ForeignEntryReason::UnsupportedName
                    },
                )?;
                continue;
            };
            if file_type != FileType::Directory {
                state.record_foreign(
                    EntryParent::Root,
                    name,
                    &stat,
                    foreign_type_reason(file_type),
                )?;
                continue;
            }
            let year_directory = open_directory_at(&self.directory, name_string)?;
            Self::scan_year(&year_directory, year, &mut state)?;
        }
        Ok(state.finish())
    }

    fn scan_year(
        year_directory: &File,
        year: u16,
        state: &mut ScanState<'_>,
    ) -> Result<(), LayoutError> {
        let mut entries = Dir::read_from(year_directory).map_err(errno_to_layout)?;
        for entry in &mut entries {
            let entry = entry.map_err(errno_to_layout)?;
            let name = entry.file_name();
            let name_bytes = name.to_bytes();
            if is_dot(name_bytes) {
                continue;
            }
            state.account(name_bytes.len())?;
            let stat = stat_no_follow_name(year_directory, name)?;
            let file_type = FileType::from_raw_mode(stat.st_mode);
            let parent = EntryParent::Year(year);
            let Some(name_string) = ascii_name(name_bytes) else {
                state.record_foreign(parent, name, &stat, ForeignEntryReason::NonAsciiName)?;
                continue;
            };
            let Some(month) = parse_month(name_string) else {
                state.record_foreign(
                    parent,
                    name,
                    &stat,
                    if file_type == FileType::Symlink {
                        ForeignEntryReason::SymbolicLink
                    } else {
                        ForeignEntryReason::UnsupportedName
                    },
                )?;
                continue;
            };
            if file_type != FileType::Directory {
                state.record_foreign(parent, name, &stat, foreign_type_reason(file_type))?;
                continue;
            }
            let month_directory = open_directory_at(year_directory, name_string)?;
            Self::scan_month(&month_directory, year, month, state)?;
        }
        Ok(())
    }

    fn scan_month(
        month_directory: &File,
        year: u16,
        month: u8,
        state: &mut ScanState<'_>,
    ) -> Result<(), LayoutError> {
        let mut entries = Dir::read_from(month_directory).map_err(errno_to_layout)?;
        for entry in &mut entries {
            let entry = entry.map_err(errno_to_layout)?;
            let name = entry.file_name();
            let name_bytes = name.to_bytes();
            if is_dot(name_bytes) {
                continue;
            }
            state.account(name_bytes.len())?;
            let stat = stat_no_follow_name(month_directory, name)?;
            let file_type = FileType::from_raw_mode(stat.st_mode);
            let parent = EntryParent::Month { year, month };
            let Some(name_string) = ascii_name(name_bytes) else {
                state.record_foreign(parent, name, &stat, ForeignEntryReason::NonAsciiName)?;
                continue;
            };
            let Some(day_number) = parse_day(year, month, name_string) else {
                state.record_foreign(
                    parent,
                    name,
                    &stat,
                    if file_type == FileType::Symlink {
                        ForeignEntryReason::SymbolicLink
                    } else {
                        ForeignEntryReason::UnsupportedName
                    },
                )?;
                continue;
            };
            if file_type != FileType::Directory {
                state.record_foreign(parent, name, &stat, foreign_type_reason(file_type))?;
                continue;
            }
            let day_directory = open_directory_at(month_directory, name_string)?;
            let day = UtcDay::new(year, month, day_number)?;
            state.account_metadata(size_of::<UtcDay>())?;
            state.days.push(day);
            Self::scan_day(&day_directory, day, state)?;
        }
        Ok(())
    }

    fn scan_day(
        day_directory: &File,
        day: UtcDay,
        state: &mut ScanState<'_>,
    ) -> Result<(), LayoutError> {
        let mut entries = Dir::read_from(day_directory).map_err(errno_to_layout)?;
        let mut day_entries = 0_usize;
        let mut finals: BTreeMap<SegmentId, DayArtifacts> = BTreeMap::new();
        for entry in &mut entries {
            let entry = entry.map_err(errno_to_layout)?;
            let name = entry.file_name();
            let name_bytes = name.to_bytes();
            if is_dot(name_bytes) {
                continue;
            }
            day_entries =
                day_entries
                    .checked_add(1)
                    .ok_or(LayoutError::TraversalLimitExceeded {
                        kind: LimitKind::EntriesPerDay,
                        limit: state.limits.max_entries_per_day,
                    })?;
            if day_entries > state.limits.max_entries_per_day {
                return Err(LayoutError::TraversalLimitExceeded {
                    kind: LimitKind::EntriesPerDay,
                    limit: state.limits.max_entries_per_day,
                });
            }
            state.account(name_bytes.len())?;
            let stat = stat_no_follow_name(day_directory, name)?;
            let file_type = FileType::from_raw_mode(stat.st_mode);
            let parent = EntryParent::Day(day);
            let Some(name_string) = ascii_name(name_bytes) else {
                state.record_foreign(parent, name, &stat, ForeignEntryReason::NonAsciiName)?;
                continue;
            };
            if file_type != FileType::RegularFile {
                state.record_foreign(parent, name, &stat, foreign_type_reason(file_type))?;
                continue;
            }
            let parsed = match parse_leaf(name_string, day) {
                Ok(parsed) => parsed,
                Err(error) => {
                    state.record_foreign(
                        parent,
                        name,
                        &stat,
                        if matches!(error, LayoutError::MisbucketedSegment { .. }) {
                            ForeignEntryReason::MisbucketedSegment
                        } else {
                            ForeignEntryReason::UnsupportedName
                        },
                    )?;
                    continue;
                }
            };
            let identity = FileIdentity::from_stat(&stat);
            let bytes = identity.len;
            match parsed {
                ParsedLeaf::Zms(address) => {
                    finals.entry(address.id).or_default().zms = Some(identity);
                }
                ParsedLeaf::Idx(address) => {
                    finals.entry(address.id).or_default().idx = Some(bytes);
                }
                ParsedLeaf::Temporary(address, kind) => {
                    state.account_metadata(ENTRY_METADATA_BYTES)?;
                    state.temporaries.push(TemporaryObject {
                        address,
                        kind,
                        identity,
                        file_name: name_string.to_owned(),
                    });
                }
            }
        }

        for (id, artifacts) in finals {
            let address = SegmentAddress::in_day(id, day)?;
            if let Some(zms_identity) = artifacts.zms {
                state.account_segment()?;
                state.segments.push(SegmentArtifacts {
                    address,
                    zms_identity,
                    zms_bytes: zms_identity.len,
                    idx_bytes: artifacts.idx,
                });
            } else if artifacts.idx.is_some() {
                state.account_metadata(size_of::<SegmentAddress>())?;
                state.orphan_indexs.push(address);
            }
        }
        Ok(())
    }

    /// Opens a verified ZMS relative to the root descriptor.
    ///
    /// # Errors
    ///
    /// Returns an error if a calendar component or final file is missing,
    /// replaced by a symbolic link, or has the wrong type.
    pub fn open_zms(&self, address: SegmentAddress) -> Result<File, LayoutError> {
        self.open_final(address, FileKind::Zms)
    }

    /// Opens a verified IDX relative to the root descriptor when it exists.
    ///
    /// # Errors
    ///
    /// Returns an error if a component is unsafe or the existing final object
    /// is not a regular file.
    pub fn open_idx(&self, address: SegmentAddress) -> Result<Option<File>, LayoutError> {
        let day = self.open_day(address.day)?;
        let name = address.idx_name();
        match open_regular_at(&day, &name, OFlags::RDONLY) {
            Ok(file) => Ok(Some(file)),
            Err(LayoutError::Io(error)) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Opens a temporary object returned by [`scan`](Self::scan).
    ///
    /// The verified leaf name and its typed day address are used directly;
    /// callers cannot substitute an arbitrary relative path.
    ///
    /// # Errors
    ///
    /// Returns an error if the object disappeared, changed type, or any
    /// calendar component became unsafe.
    pub fn open_temporary(&self, temporary: &TemporaryObject) -> Result<File, LayoutError> {
        let day = self.open_day(temporary.address.day)?;
        open_regular_at(&day, temporary.file_name(), OFlags::RDONLY)
    }

    /// Opens the active journal for reading when it exists.
    ///
    /// # Errors
    ///
    /// Returns an error when the existing entry is unsafe or unreadable.
    pub fn open_active_journal(&self) -> Result<Option<File>, LayoutError> {
        match open_regular_at(&self.directory, ACTIVE_JOURNAL_NAME, OFlags::RDONLY) {
            Ok(file) => Ok(Some(file)),
            Err(LayoutError::Io(error)) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Reads the byte occupancy of the partition backing this root.
    ///
    /// The measurement is a single `statvfs` of the already-open root
    /// descriptor, so no path is re-resolved and foreign data on a shared
    /// partition is included in [`FilesystemUsage::used_bytes`].
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the descriptor cannot be queried.
    pub fn filesystem_usage(&self) -> Result<FilesystemUsage, LayoutError> {
        let stat = rustix::fs::fstatvfs(&*self.directory).map_err(errno_to_layout)?;
        let block_bytes = stat.f_frsize;
        let total_bytes = stat.f_blocks.saturating_mul(block_bytes);
        let free_bytes = stat.f_bfree.saturating_mul(block_bytes);
        Ok(FilesystemUsage {
            total_bytes,
            used_bytes: total_bytes.saturating_sub(free_bytes),
            available_bytes: stat.f_bavail.saturating_mul(block_bytes),
        })
    }

    /// Establishes lifetime-exclusive writer ownership after two strict scans.
    ///
    /// # Errors
    ///
    /// Returns a structural error, I/O error, or
    /// [`LayoutError::OwnerContended`].
    pub fn acquire_writer(&self, limits: LayoutLimits) -> Result<WriterOwner, LayoutError> {
        self.scan(limits)?;
        let root_lock = self.acquire_writer_root_lock()?;
        let lock = match self.acquire_writer_lock() {
            Ok(lock) => lock,
            Err(_error) if writer_lock_is_poisoned(self) => root_lock.try_clone()?,
            Err(error) => return Err(error),
        };
        self.scan(limits)?;
        Ok(WriterOwner {
            root: self.clone(),
            owner_lock: lock,
            _root_lock: root_lock,
        })
    }

    /// Establishes lifetime-exclusive index ownership after two strict scans.
    ///
    /// # Errors
    ///
    /// Returns a structural error, I/O error, or
    /// [`LayoutError::OwnerContended`].
    pub fn acquire_index(&self, limits: LayoutLimits) -> Result<IndexOwner, LayoutError> {
        self.scan(limits)?;
        let lock = self.acquire_lock(OVERVIEW_OWNER_LOCK_NAME, OwnerKind::Index)?;
        self.scan(limits)?;
        Ok(IndexOwner {
            root: self.clone(),
            _lock: lock,
        })
    }

    fn acquire_lock(&self, name: &str, owner: OwnerKind) -> Result<File, LayoutError> {
        let (lock, _created) =
            open_or_create_regular(&self.directory, name, OFlags::RDWR, LOCK_FILE_MODE)?;
        rustix::fs::fchmod(&lock, LOCK_FILE_MODE)
            .map_err(errno_to_io)
            .map_err(LayoutError::Io)?;
        lock.sync_all()?;
        // A previous creator may have initialized the inode and then failed
        // to synchronize its root entry. EEXIST alone does not mean durable.
        self.directory.sync_all()?;
        match rustix::fs::flock(&lock, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Ok(lock),
            Err(error) if error == rustix::io::Errno::WOULDBLOCK => {
                Err(LayoutError::OwnerContended { owner })
            }
            Err(error) => Err(LayoutError::Io(errno_to_io(error))),
        }
    }

    fn acquire_writer_lock(&self) -> Result<File, LayoutError> {
        let started = Instant::now();
        loop {
            match self.acquire_lock(WRITER_OWNER_LOCK_NAME, OwnerKind::Writer) {
                Err(LayoutError::OwnerContended {
                    owner: OwnerKind::Writer,
                }) if started.elapsed() < WRITER_LOCK_HANDOFF_TIMEOUT => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                result => return result,
            }
        }
    }

    fn acquire_writer_root_lock(&self) -> Result<File, LayoutError> {
        let lock = rustix::fs::openat(
            &*self.directory,
            ".",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map(File::from)
        .map_err(errno_to_io)
        .map_err(LayoutError::Io)?;
        let started = Instant::now();
        loop {
            match rustix::fs::flock(&lock, FlockOperation::NonBlockingLockExclusive) {
                Ok(()) => return Ok(lock),
                Err(error)
                    if error == rustix::io::Errno::WOULDBLOCK
                        && started.elapsed() < WRITER_LOCK_HANDOFF_TIMEOUT =>
                {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(error) if error == rustix::io::Errno::WOULDBLOCK => {
                    return Err(LayoutError::OwnerContended {
                        owner: OwnerKind::Writer,
                    });
                }
                Err(error) => return Err(LayoutError::Io(errno_to_io(error))),
            }
        }
    }

    fn open_final(&self, address: SegmentAddress, kind: FileKind) -> Result<File, LayoutError> {
        let day = self.open_day(address.day)?;
        let name = match kind {
            FileKind::Zms => address.zms_name(),
            FileKind::Idx => address.idx_name(),
        };
        open_regular_at(&day, &name, OFlags::RDONLY)
    }

    fn open_day(&self, day: UtcDay) -> Result<File, LayoutError> {
        let year = open_directory_at(&self.directory, &day.year_component())?;
        let month = open_directory_at(&year, &day.month_component())?;
        open_directory_at(&month, &day.day_component())
    }

    fn ensure_day(&self, day: UtcDay) -> Result<File, LayoutError> {
        let year = ensure_directory_at(&self.directory, &day.year_component())?;
        let month = ensure_directory_at(&year, &day.month_component())?;
        ensure_directory_at(&month, &day.day_component())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
/// Filesystem identity used to pin an immutable segment between discovery and
/// opening its contents.
pub struct FileIdentity {
    /// Filesystem device number.
    pub device: u64,
    /// Inode number on the device.
    pub inode: u64,
    /// File length in bytes.
    pub len: u64,
    /// Modification time, whole seconds since the Unix epoch.
    pub mtime_seconds: i64,
    /// Nanosecond part of the modification time.
    pub mtime_nanoseconds: i64,
    /// Metadata-change time, whole seconds since the Unix epoch.
    pub ctime_seconds: i64,
    /// Nanosecond part of the metadata-change time.
    pub ctime_nanoseconds: i64,
}

impl FileIdentity {
    #[allow(
        clippy::useless_conversion,
        reason = "rustix Stat integer field types differ across supported Unix targets"
    )]
    fn from_stat(stat: &rustix::fs::Stat) -> Self {
        Self {
            device: u64::try_from(stat.st_dev).unwrap_or(u64::MAX),
            inode: u64::try_from(stat.st_ino).unwrap_or(u64::MAX),
            len: u64::try_from(stat.st_size).unwrap_or(u64::MAX),
            mtime_seconds: stat.st_mtime,
            mtime_nanoseconds: i64::try_from(stat.st_mtime_nsec).unwrap_or(i64::MAX),
            ctime_seconds: stat.st_ctime,
            ctime_nanoseconds: i64::try_from(stat.st_ctime_nsec).unwrap_or(i64::MAX),
        }
    }

    /// Reads the identity from an already-open file descriptor.
    ///
    /// # Errors
    ///
    /// Returns the underlying `fstat` error.
    pub fn from_file(file: &File) -> io::Result<Self> {
        let metadata = file.metadata()?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            len: metadata.len(),
            mtime_seconds: metadata.mtime(),
            mtime_nanoseconds: metadata.mtime_nsec(),
            ctime_seconds: metadata.ctime(),
            ctime_nanoseconds: metadata.ctime_nsec(),
        })
    }

    const fn same_named_object(self, other: Self) -> bool {
        self.device == other.device
            && self.inode == other.inode
            && self.len == other.len
            && self.mtime_seconds == other.mtime_seconds
            && self.mtime_nanoseconds == other.mtime_nanoseconds
            && self.ctime_seconds == other.ctime_seconds
            && self.ctime_nanoseconds == other.ctime_nanoseconds
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{FileTimes, OpenOptions};
    use std::io::Write as _;
    use std::os::unix::fs::{FileExt as _, symlink};
    use std::time::SystemTime;

    use super::*;

    fn address(value: i64) -> SegmentAddress {
        SegmentAddress::new(SegmentId::new(value).unwrap()).unwrap()
    }

    fn rewrite_same_inode_with_restored_mtime(
        path: &Path,
        prepared_identity: FileIdentity,
        prepared_mtime: SystemTime,
        replacement: &[u8],
    ) -> FileIdentity {
        assert_eq!(replacement.len() as u64, prepared_identity.len);
        let rewritten = OpenOptions::new().write(true).open(path).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            rewritten.write_all_at(replacement, 0).unwrap();
            rewritten
                .set_times(FileTimes::new().set_modified(prepared_mtime))
                .unwrap();
            rewritten.sync_all().unwrap();
            let identity = FileIdentity::from_file(&rewritten).unwrap();
            if (identity.ctime_seconds, identity.ctime_nanoseconds)
                != (
                    prepared_identity.ctime_seconds,
                    prepared_identity.ctime_nanoseconds,
                )
            {
                return identity;
            }
            assert!(
                Instant::now() < deadline,
                "the filesystem did not expose the same-inode rewrite through ctime"
            );
            std::thread::yield_now();
        }
    }

    #[test]
    fn strict_scan_sorts_numeric_ids_and_associates_indexs() {
        let directory = tempfile::tempdir().unwrap();
        let root = DataRoot::open(directory.path()).unwrap();
        let owner = root.acquire_writer(LayoutLimits::default()).unwrap();
        let later = address(1_709_164_802_000_000);
        let earlier = address(1_709_164_801_000_000);
        for item in [later, earlier] {
            let mut temp = owner.create_zms_temp(item).unwrap();
            temp.file_mut().write_all(b"ZMS").unwrap();
            temp.publish().unwrap();
        }

        let snapshot = root.scan(LayoutLimits::default()).unwrap();
        assert_eq!(
            snapshot
                .segments
                .iter()
                .map(|segment| segment.address.id)
                .collect::<Vec<_>>(),
            vec![earlier.id, later.id]
        );
    }

    #[test]
    fn remove_finished_segment_unlinks_the_zms_and_reports_freed_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let root = DataRoot::open(directory.path()).unwrap();
        let older = address(1_709_164_801_000_000);
        let newer = address(1_709_164_802_000_000);
        let writer = root.acquire_writer(LayoutLimits::default()).unwrap();
        for item in [older, newer] {
            let mut temp = writer.create_zms_temp(item).unwrap();
            temp.file_mut().write_all(b"ZMSBODY").unwrap();
            temp.publish().unwrap();
        }

        let removal = writer.remove_finished_segment(older).unwrap();
        assert_eq!(removal.zms_bytes, b"ZMSBODY".len() as u64);
        assert_eq!(removal.idx_bytes, None, "no sibling index was present");

        let snapshot = root.scan(LayoutLimits::default()).unwrap();
        assert_eq!(
            snapshot
                .segments
                .iter()
                .map(|segment| segment.address.id)
                .collect::<Vec<_>>(),
            vec![newer.id],
            "only the newer segment survives"
        );
    }

    #[test]
    fn remove_finished_segment_frees_nothing_when_it_is_already_gone() {
        let directory = tempfile::tempdir().unwrap();
        let root = DataRoot::open(directory.path()).unwrap();
        let older = address(1_709_164_801_000_000);
        let keeper = address(1_709_164_802_000_000);
        let writer = root.acquire_writer(LayoutLimits::default()).unwrap();
        for item in [older, keeper] {
            let mut temp = writer.create_zms_temp(item).unwrap();
            temp.file_mut().write_all(b"ZMS").unwrap();
            temp.publish().unwrap();
        }
        writer.remove_finished_segment(older).unwrap();

        let second = writer.remove_finished_segment(older).unwrap();
        assert_eq!(second.total_bytes(), 0, "a repeated removal frees nothing");
    }

    #[test]
    fn segment_removal_total_sums_the_zms_and_index() {
        assert_eq!(
            SegmentRemoval {
                zms_bytes: 100,
                idx_bytes: Some(30),
            }
            .total_bytes(),
            130
        );
        assert_eq!(
            SegmentRemoval {
                zms_bytes: 100,
                idx_bytes: None,
            }
            .total_bytes(),
            100
        );
    }

    #[test]
    fn index_owner_prunes_empty_calendar_ancestors_bottom_up() {
        let directory = tempfile::tempdir().unwrap();
        let root = DataRoot::open(directory.path()).unwrap();
        let address = address(1_709_164_801_000_000);
        let writer = root.acquire_writer(LayoutLimits::default()).unwrap();
        let temp = writer.create_zms_temp(address).unwrap();
        temp.discard().unwrap();
        drop(writer);
        assert!(directory.path().join(address.day.year_component()).is_dir());

        let index = root.acquire_index(LayoutLimits::default()).unwrap();
        index.prune_empty_day(address.day).unwrap();

        assert!(!directory.path().join(address.day.year_component()).exists());
    }

    #[test]
    fn index_does_not_prune_a_day_while_the_writer_owns_the_root() {
        let directory = tempfile::tempdir().unwrap();
        let root = DataRoot::open(directory.path()).unwrap();
        let address = address(1_709_164_801_000_000);
        let writer = root.acquire_writer(LayoutLimits::default()).unwrap();
        let temp = writer.create_zms_temp(address).unwrap();
        temp.discard().unwrap();
        let index = root.acquire_index(LayoutLimits::default()).unwrap();

        index.prune_empty_day(address.day).unwrap();

        assert!(directory.path().join(address.day.year_component()).is_dir());
        drop(writer);
        index.prune_empty_day(address.day).unwrap();
        assert!(!directory.path().join(address.day.year_component()).exists());
    }

    #[test]
    fn flat_segment_is_excluded_without_reading_it() {
        for name in ["1000.zms", "1000.idx"] {
            let directory = tempfile::tempdir().unwrap();
            std::fs::write(directory.path().join(name), b"not a container").unwrap();
            let root = DataRoot::open(directory.path()).unwrap();
            let snapshot = root.scan(LayoutLimits::default()).unwrap();
            assert!(snapshot.segments.is_empty());
            assert_eq!(snapshot.foreign_entries.len(), 1);
            assert_eq!(
                snapshot.foreign_entries[0].diagnostic().reason,
                ForeignEntryReason::UnsupportedFlatArtifact
            );
        }
    }

    #[test]
    fn symlinked_calendar_component_is_excluded_without_following() {
        let directory = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        symlink(target.path(), directory.path().join("2024")).unwrap();
        let root = DataRoot::open(directory.path()).unwrap();
        let snapshot = root.scan(LayoutLimits::default()).unwrap();
        assert!(snapshot.days.is_empty());
        assert_eq!(snapshot.foreign_entries.len(), 1);
        assert_eq!(
            snapshot.foreign_entries[0].diagnostic().reason,
            ForeignEntryReason::SymbolicLink
        );
    }

    #[test]
    fn symlinks_are_excluded_at_month_day_and_leaf_levels() {
        for level in ["month", "day", "leaf"] {
            let directory = tempfile::tempdir().unwrap();
            let target = tempfile::tempdir().unwrap();
            match level {
                "month" => {
                    std::fs::create_dir(directory.path().join("2024")).unwrap();
                    symlink(target.path(), directory.path().join("2024/02")).unwrap();
                }
                "day" => {
                    std::fs::create_dir_all(directory.path().join("2024/02")).unwrap();
                    symlink(target.path(), directory.path().join("2024/02/29")).unwrap();
                }
                "leaf" => {
                    let day = directory.path().join("2024/02/29");
                    std::fs::create_dir_all(&day).unwrap();
                    let target_file = target.path().join("segment");
                    std::fs::write(&target_file, b"ZMS").unwrap();
                    symlink(&target_file, day.join("1709164800000000.zms")).unwrap();
                }
                _ => unreachable!(),
            }
            let root = DataRoot::open(directory.path()).unwrap();
            let snapshot = root.scan(LayoutLimits::default()).unwrap();
            assert!(
                snapshot.segments.is_empty(),
                "{level} symlink must not become a segment"
            );
            assert_eq!(snapshot.foreign_entries.len(), 1);
            assert_eq!(
                snapshot.foreign_entries[0].diagnostic().reason,
                ForeignEntryReason::SymbolicLink
            );
        }
    }

    #[test]
    fn noncanonical_segment_names_are_excluded() {
        for name in ["+1.zms", "01.zms", "-0.zms"] {
            let directory = tempfile::tempdir().unwrap();
            let day = directory.path().join("1970/01/01");
            std::fs::create_dir_all(&day).unwrap();
            std::fs::write(day.join(name), b"ZMS").unwrap();
            let root = DataRoot::open(directory.path()).unwrap();
            let snapshot = root.scan(LayoutLimits::default()).unwrap();
            assert!(snapshot.segments.is_empty());
            assert_eq!(snapshot.foreign_entries.len(), 1);
            assert_eq!(
                snapshot.foreign_entries[0].diagnostic().reason,
                ForeignEntryReason::UnsupportedName,
                "{name} must not alias a canonical SegmentId"
            );
        }
    }

    #[test]
    fn a_day_with_more_than_192_segments_is_valid_when_within_explicit_limits() {
        let directory = tempfile::tempdir().unwrap();
        let day = directory.path().join("2024/02/29");
        std::fs::create_dir_all(&day).unwrap();
        let midnight = 1_709_164_800_000_000_i64;
        for offset in 0..256_i64 {
            std::fs::write(day.join(format!("{}.zms", midnight + offset)), b"ZMS").unwrap();
        }

        let snapshot = DataRoot::open(directory.path())
            .unwrap()
            .scan(LayoutLimits::default())
            .unwrap();
        assert_eq!(snapshot.segments.len(), 256);
        assert_eq!(snapshot.days, vec![UtcDay::new(2024, 2, 29).unwrap()]);
    }

    #[test]
    fn misbucketed_segment_is_excluded() {
        let directory = tempfile::tempdir().unwrap();
        let day = directory.path().join("2024/02/28");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(day.join("1709164800000000.zms"), b"ZMS").unwrap();
        let root = DataRoot::open(directory.path()).unwrap();
        let snapshot = root.scan(LayoutLimits::default()).unwrap();
        assert!(snapshot.segments.is_empty());
        assert_eq!(snapshot.foreign_entries.len(), 1);
        assert_eq!(
            snapshot.foreign_entries[0].diagnostic().reason,
            ForeignEntryReason::MisbucketedSegment
        );
    }

    #[test]
    fn traversal_returns_no_partial_result_at_a_limit() {
        let directory = tempfile::tempdir().unwrap();
        let root = DataRoot::open(directory.path()).unwrap();
        let owner = root.acquire_writer(LayoutLimits::default()).unwrap();
        for value in [1_709_164_801_000_000, 1_709_164_802_000_000] {
            let mut temp = owner.create_zms_temp(address(value)).unwrap();
            temp.file_mut().write_all(b"ZMS").unwrap();
            temp.publish().unwrap();
        }
        let limits = LayoutLimits {
            max_segments: 1,
            ..LayoutLimits::default()
        };
        assert!(matches!(
            root.scan(limits),
            Err(LayoutError::TraversalLimitExceeded {
                kind: LimitKind::Segments,
                ..
            })
        ));
    }

    #[test]
    fn visited_entry_limit_accepts_the_boundary_and_rejects_the_next_entry() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join("2024/01/01")).unwrap();
        let root = DataRoot::open(directory.path()).unwrap();
        let exact = LayoutLimits {
            max_visited_entries: 3,
            ..LayoutLimits::default()
        };
        assert_eq!(root.scan(exact).unwrap().visited_entries, 3);

        let below = LayoutLimits {
            max_visited_entries: 2,
            ..LayoutLimits::default()
        };
        assert!(matches!(
            root.scan(below),
            Err(LayoutError::TraversalLimitExceeded {
                kind: LimitKind::VisitedEntries,
                limit: 2,
            })
        ));
    }

    #[test]
    fn entries_per_day_limit_accepts_the_boundary_and_rejects_the_next_entry() {
        let directory = tempfile::tempdir().unwrap();
        let day = directory.path().join("2024/02/29");
        std::fs::create_dir_all(&day).unwrap();
        let midnight = 1_709_164_800_000_000_i64;
        for offset in 0..2_i64 {
            std::fs::write(day.join(format!("{}.zms", midnight + offset)), b"ZMS").unwrap();
        }
        let root = DataRoot::open(directory.path()).unwrap();
        let exact = LayoutLimits {
            max_entries_per_day: 2,
            ..LayoutLimits::default()
        };
        assert_eq!(root.scan(exact).unwrap().segments.len(), 2);

        let below = LayoutLimits {
            max_entries_per_day: 1,
            ..LayoutLimits::default()
        };
        assert!(matches!(
            root.scan(below),
            Err(LayoutError::TraversalLimitExceeded {
                kind: LimitKind::EntriesPerDay,
                limit: 1,
            })
        ));
    }

    #[test]
    fn metadata_limit_accepts_the_boundary_and_rejects_the_next_byte() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("2024")).unwrap();
        let root = DataRoot::open(directory.path()).unwrap();
        let exact_bytes = ENTRY_METADATA_BYTES + "2024".len();
        let exact = LayoutLimits {
            max_metadata_bytes: exact_bytes,
            ..LayoutLimits::default()
        };
        assert_eq!(root.scan(exact).unwrap().metadata_bytes, exact_bytes);

        let below = LayoutLimits {
            max_metadata_bytes: exact_bytes - 1,
            ..LayoutLimits::default()
        };
        assert!(matches!(
            root.scan(below),
            Err(LayoutError::TraversalLimitExceeded {
                kind: LimitKind::MetadataBytes,
                limit,
            }) if limit == exact_bytes - 1
        ));
    }

    #[test]
    fn one_writer_owner_is_enforced() {
        let directory = tempfile::tempdir().unwrap();
        let first_root = DataRoot::open(directory.path()).unwrap();
        let second_root = DataRoot::open(directory.path()).unwrap();
        let _first = first_root.acquire_writer(LayoutLimits::default()).unwrap();
        assert!(matches!(
            second_root.acquire_writer(LayoutLimits::default()),
            Err(LayoutError::OwnerContended {
                owner: OwnerKind::Writer
            })
        ));
    }

    #[test]
    fn cloned_writer_lease_keeps_exclusive_ownership() {
        let directory = tempfile::tempdir().unwrap();
        let first_root = DataRoot::open(directory.path()).unwrap();
        let second_root = DataRoot::open(directory.path()).unwrap();
        let owner = first_root.acquire_writer(LayoutLimits::default()).unwrap();
        let lease = owner.try_clone_lease().unwrap();
        drop(owner);

        assert!(matches!(
            second_root.acquire_writer(LayoutLimits::default()),
            Err(LayoutError::OwnerContended {
                owner: OwnerKind::Writer
            })
        ));

        drop(lease);
        second_root.acquire_writer(LayoutLimits::default()).unwrap();
    }

    #[test]
    fn direct_open_rejects_a_fifo_without_blocking() {
        let directory = tempfile::tempdir().unwrap();
        let root = DataRoot::open(directory.path()).unwrap();
        let address = address(1_709_164_801_000_000);
        let day = directory
            .path()
            .join(address.day.year_component())
            .join(address.day.month_component())
            .join(address.day.day_component());
        std::fs::create_dir_all(&day).unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(day.join(address.zms_name()))
                .status()
                .unwrap()
                .success()
        );

        assert!(matches!(
            root.open_zms(address),
            Err(LayoutError::UnexpectedLeafEntryType { .. })
        ));
    }

    #[test]
    fn zms_publication_rejects_a_replaced_temporary_name() {
        let directory = tempfile::tempdir().unwrap();
        let root = DataRoot::open(directory.path()).unwrap();
        let owner = root.acquire_writer(LayoutLimits::default()).unwrap();
        let address = address(1_709_164_801_000_000);
        let mut temporary = owner.create_zms_temp(address).unwrap();
        temporary.file_mut().write_all(b"expected ZMS").unwrap();
        let day = directory
            .path()
            .join(address.day.year_component())
            .join(address.day.month_component())
            .join(address.day.day_component());
        let temporary_name = temporary.temp_name.clone();
        std::fs::remove_file(day.join(&temporary_name)).unwrap();
        std::fs::write(day.join(&temporary_name), b"replacement").unwrap();

        assert!(matches!(
            temporary.publish(),
            Err(LayoutError::TemporaryChanged { .. })
        ));
        drop(temporary);
        assert!(!day.join(address.zms_name()).exists());
        assert_eq!(
            std::fs::read(day.join(&temporary_name)).unwrap(),
            b"replacement"
        );
    }

    #[test]
    fn zms_publication_rejects_same_inode_rewrite_with_restored_mtime() {
        let directory = tempfile::tempdir().unwrap();
        let root = DataRoot::open(directory.path()).unwrap();
        let owner = root.acquire_writer(LayoutLimits::default()).unwrap();
        let address = address(1_709_164_801_000_000);
        let mut temporary = owner.create_zms_temp(address).unwrap();
        temporary.file_mut().write_all(b"expected ZMS").unwrap();
        let prepared = temporary.try_clone_file().unwrap();
        let prepared_identity = FileIdentity::from_file(&prepared).unwrap();
        let prepared_mtime = prepared.metadata().unwrap().modified().unwrap();
        drop(prepared);

        let day = directory
            .path()
            .join(address.day.year_component())
            .join(address.day.month_component())
            .join(address.day.day_component());
        let temporary_path = day.join(&temporary.temp_name);
        let rewritten_identity = rewrite_same_inode_with_restored_mtime(
            &temporary_path,
            prepared_identity,
            prepared_mtime,
            b"tampered ZMS",
        );
        assert_eq!(rewritten_identity.device, prepared_identity.device);
        assert_eq!(rewritten_identity.inode, prepared_identity.inode);
        assert_eq!(rewritten_identity.len, prepared_identity.len);
        assert_eq!(
            (
                rewritten_identity.mtime_seconds,
                rewritten_identity.mtime_nanoseconds
            ),
            (
                prepared_identity.mtime_seconds,
                prepared_identity.mtime_nanoseconds
            )
        );

        assert!(matches!(
            temporary.publish(),
            Err(LayoutError::TemporaryChanged { .. })
        ));
        assert!(!day.join(address.zms_name()).exists());
        assert_eq!(std::fs::read(temporary_path).unwrap(), b"tampered ZMS");
    }

    #[test]
    fn prepared_idx_publishes_under_its_post_rename_identity() {
        let directory = tempfile::tempdir().unwrap();
        let root = DataRoot::open(directory.path()).unwrap();
        let address = address(1_709_164_801_000_000);
        let writer = root.acquire_writer(LayoutLimits::default()).unwrap();
        let mut zms = writer.create_zms_temp(address).unwrap();
        zms.file_mut().write_all(b"source ZMS").unwrap();
        zms.publish().unwrap();
        drop(zms);
        drop(writer);

        let owner = root.acquire_index(LayoutLimits::default()).unwrap();
        let mut temporary = owner.create_idx_temp(address).unwrap();
        temporary.file_mut().write_all(b"expected IDX").unwrap();
        drop(temporary.try_clone_file().unwrap());
        temporary.publish().unwrap();

        assert_eq!(
            std::fs::read(root.diagnostic_file_path(address, FileKind::Idx)).unwrap(),
            b"expected IDX"
        );
    }

    #[test]
    fn idx_publication_rejects_same_inode_rewrite_with_restored_mtime() {
        let directory = tempfile::tempdir().unwrap();
        let root = DataRoot::open(directory.path()).unwrap();
        let address = address(1_709_164_801_000_000);
        let writer = root.acquire_writer(LayoutLimits::default()).unwrap();
        let mut zms = writer.create_zms_temp(address).unwrap();
        zms.file_mut().write_all(b"source ZMS").unwrap();
        zms.publish().unwrap();
        drop(zms);
        drop(writer);

        let owner = root.acquire_index(LayoutLimits::default()).unwrap();
        let mut temporary = owner.create_idx_temp(address).unwrap();
        temporary.file_mut().write_all(b"expected IDX").unwrap();
        let prepared = temporary.try_clone_file().unwrap();
        let prepared_identity = FileIdentity::from_file(&prepared).unwrap();
        let prepared_mtime = prepared.metadata().unwrap().modified().unwrap();
        drop(prepared);

        let day = directory
            .path()
            .join(address.day.year_component())
            .join(address.day.month_component())
            .join(address.day.day_component());
        let temporary_path = day.join(&temporary.temp_name);
        let rewritten_identity = rewrite_same_inode_with_restored_mtime(
            &temporary_path,
            prepared_identity,
            prepared_mtime,
            b"tampered IDX",
        );
        assert_eq!(rewritten_identity.device, prepared_identity.device);
        assert_eq!(rewritten_identity.inode, prepared_identity.inode);
        assert_eq!(rewritten_identity.len, prepared_identity.len);
        assert_eq!(
            (
                rewritten_identity.mtime_seconds,
                rewritten_identity.mtime_nanoseconds
            ),
            (
                prepared_identity.mtime_seconds,
                prepared_identity.mtime_nanoseconds
            )
        );

        assert!(matches!(
            temporary.publish(),
            Err(LayoutError::TemporaryChanged { .. })
        ));
        assert!(!day.join(address.idx_name()).exists());
        assert_eq!(
            std::fs::read(day.join(address.zms_name())).unwrap(),
            b"source ZMS"
        );
    }

    #[test]
    fn idx_publication_rejects_a_replaced_temporary_name() {
        let directory = tempfile::tempdir().unwrap();
        let root = DataRoot::open(directory.path()).unwrap();
        let address = address(1_709_164_801_000_000);
        let writer = root.acquire_writer(LayoutLimits::default()).unwrap();
        let mut zms = writer.create_zms_temp(address).unwrap();
        zms.file_mut().write_all(b"source ZMS").unwrap();
        zms.publish().unwrap();
        drop(zms);
        drop(writer);

        let owner = root.acquire_index(LayoutLimits::default()).unwrap();
        let mut temporary = owner.create_idx_temp(address).unwrap();
        temporary.file_mut().write_all(b"expected IDX").unwrap();
        let day = directory
            .path()
            .join(address.day.year_component())
            .join(address.day.month_component())
            .join(address.day.day_component());
        let temporary_name = temporary.temp_name.clone();
        std::fs::remove_file(day.join(&temporary_name)).unwrap();
        std::fs::write(day.join(&temporary_name), b"replacement").unwrap();

        assert!(matches!(
            temporary.publish(),
            Err(LayoutError::TemporaryChanged { .. })
        ));
        assert!(!day.join(address.idx_name()).exists());
        assert_eq!(
            std::fs::read(day.join(temporary_name)).unwrap(),
            b"replacement"
        );
    }
}
