//! POSIX adapter for immutable finished-segment discovery and bytes.

use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::Arc;

use kronika_format::ReadAt;

use crate::source::FinalUnit;
use crate::{
    ImmutableSegmentSource, LocalDir, ResourceCatalog, ResourceIdentity, ResourceListing,
    ResourceWarning, SegmentResource, StoreError,
};

/// POSIX-backed catalog and immutable segment source.
///
/// Filesystem traversal and local handles remain private to this adapter. The
/// resource and byte types exposed through the storage traits are opaque.
#[derive(Clone)]
pub struct PosixSource {
    dir: LocalDir,
    source_id: Arc<()>,
}

impl PosixSource {
    /// Open a POSIX data directory as an immutable segment source.
    ///
    /// This opens only the root directory descriptor. Catalog discovery is
    /// deferred until [`ResourceCatalog::resources`] is called.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if `root` is not a directory or cannot be opened.
    pub fn open(root: &Path) -> io::Result<Self> {
        Ok(Self {
            dir: LocalDir::open(root)?,
            source_id: Arc::new(()),
        })
    }

    /// Complete segment payload bytes retained in memory by this source.
    #[must_use]
    #[expect(
        clippy::unused_self,
        reason = "the instance metric matches EmbeddedSource copy accounting"
    )]
    pub const fn retained_segment_bytes(&self) -> usize {
        0
    }

    fn checked_resource<'a>(
        &self,
        resource: &'a SegmentResource<PosixResource>,
    ) -> Result<&'a FinalUnit, StoreError> {
        let handle = resource.handle();
        if !Arc::ptr_eq(&handle.source_id, &self.source_id)
            || resource.identity() != ResourceIdentity::finished(handle.unit.address.id)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "POSIX resource belongs to another source",
            )
            .into());
        }
        Ok(&handle.unit)
    }
}

impl std::fmt::Debug for PosixSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PosixSource").finish_non_exhaustive()
    }
}

/// Opaque token for one finished segment listed by a [`PosixSource`].
#[derive(Clone)]
pub struct PosixResource {
    unit: FinalUnit,
    source_id: Arc<()>,
}

impl std::fmt::Debug for PosixResource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PosixResource")
            .field(
                "identity",
                &ResourceIdentity::finished(self.unit.address.id),
            )
            .finish_non_exhaustive()
    }
}

/// Opaque positional byte reader for one opened POSIX segment.
pub struct PosixSegmentBytes {
    file: File,
    source_id: Arc<()>,
}

impl PosixSegmentBytes {
    /// Complete segment payload bytes retained in memory by this reader.
    #[must_use]
    #[expect(
        clippy::unused_self,
        reason = "the instance metric matches embedded byte-reader copy accounting"
    )]
    pub const fn retained_segment_bytes(&self) -> usize {
        0
    }
}

impl std::fmt::Debug for PosixSegmentBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PosixSegmentBytes").finish_non_exhaustive()
    }
}

impl ReadAt for PosixSegmentBytes {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        self.file.read_exact_at(buf, offset)
    }

    fn byte_len(&self) -> io::Result<u64> {
        self.file.byte_len()
    }
}

impl ResourceCatalog for PosixSource {
    type Resource = PosixResource;

    fn resources(&self) -> Result<ResourceListing<Self::Resource>, StoreError> {
        let scan = self.dir.scan_catalogs()?;
        let resources = scan
            .finished
            .iter()
            .cloned()
            .map(|unit| {
                SegmentResource::new(
                    ResourceIdentity::finished(unit.address.id),
                    unit.identity.len,
                    Arc::clone(&unit.summary),
                    PosixResource {
                        unit,
                        source_id: Arc::clone(&self.source_id),
                    },
                )
            })
            .collect();
        let warnings = scan
            .warnings
            .into_iter()
            .map(ResourceWarning::from_store)
            .collect();
        Ok(ResourceListing {
            resources,
            warnings,
        })
    }
}

impl ImmutableSegmentSource for PosixSource {
    type Bytes = PosixSegmentBytes;

    fn open_resource(
        &self,
        resource: &SegmentResource<Self::Resource>,
    ) -> Result<Self::Bytes, StoreError> {
        let unit = self.checked_resource(resource)?;
        Ok(PosixSegmentBytes {
            file: self.dir.open_finished(unit)?,
            source_id: Arc::clone(&self.source_id),
        })
    }

    fn validate_opened(
        &self,
        resource: &SegmentResource<Self::Resource>,
        bytes: &Self::Bytes,
    ) -> Result<(), StoreError> {
        let unit = self.checked_resource(resource)?;
        if !Arc::ptr_eq(&bytes.source_id, &self.source_id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "POSIX bytes belong to another source",
            )
            .into());
        }
        self.dir
            .validate_finished_file(&bytes.file, unit)
            .map_err(StoreError::from)
    }
}

#[cfg(test)]
#[path = "posix/tests.rs"]
mod tests;
