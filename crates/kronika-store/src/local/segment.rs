//! Reading a finished segment: its catalog, its checksums, and why one is
//! rejected.

#![allow(unreachable_pub, reason = "used through the parent module")]

use std::fs::File;
use std::io;

use kronika_format::{
    Catalog, Crc32c, Entry, FORMAT_VERSION, MAGIC, ReadAt, TAIL_INDEX_LEN, TailIndex,
    validate_catalog_layout,
};
use kronika_layout::FileIdentity;

use crate::catalog_summary::CatalogSummary;
use crate::source::{
    InvalidZmsReason, StoreError, StoreIoFailure, StoreIoOperation, StoreObject, StoreWarning,
    StoreWarningReason,
};

#[cfg(test)]
use super::CATALOG_SUMMARY_READS;
use super::budget::{metadata_limit_store, store_io};
use super::{MAX_CATALOG_BYTES, ZMS_CRC_CHUNK_BYTES};

pub(super) enum ZmsOpen {
    Open(File),
    Invalid(StoreIoFailure),
}

#[derive(Debug, Clone, Copy)]
pub(super) enum FinishedValidation {
    Catalog,
    Complete,
}

pub(super) const fn invalid_zms_warning(
    address: kronika_layout::SegmentAddress,
    identity: FileIdentity,
    reason: InvalidZmsReason,
    failure: Option<StoreIoFailure>,
) -> StoreWarning {
    StoreWarning {
        affected: StoreObject::Segment(address),
        reason: StoreWarningReason::InvalidZms(reason),
        identity: Some(identity),
        failure,
    }
}

pub(super) fn stale_finished_zms(
    address: kronika_layout::SegmentAddress,
    phase: &str,
) -> io::Error {
    io::Error::new(
        io::ErrorKind::Interrupted,
        format!(
            "finished segment {} changed {phase}; retry the scan",
            address.zms_name()
        ),
    )
}

pub(super) struct EncodedCatalog {
    pub(super) bytes: Vec<u8>,
    pub(super) body_end: u64,
}

pub(super) fn read_encoded_catalog<R: ReadAt>(
    reader: &R,
    metadata_budget: Option<(usize, usize)>,
) -> Result<EncodedCatalog, StoreError> {
    let len = reader.byte_len()?;

    let tail_at = len
        .checked_sub(TAIL_INDEX_LEN as u64)
        .ok_or(StoreError::TooSmall)?;

    let mut tail_bytes = [0_u8; TAIL_INDEX_LEN];
    reader.read_exact_at(&mut tail_bytes, tail_at)?;
    let tail = TailIndex::decode(tail_bytes).map_err(StoreError::TailIndex)?;

    let catalog_len = u64::from(tail.catalog_len);
    if catalog_len > MAX_CATALOG_BYTES {
        return Err(StoreError::BadCatalogLen);
    }
    let catalog_at = tail_at
        .checked_sub(catalog_len)
        .ok_or(StoreError::BadCatalogLen)?;
    if catalog_at < MAGIC.len() as u64 {
        return Err(StoreError::BadCatalogLen);
    }
    if let Some((retained, limit)) = metadata_budget {
        let catalog_bytes =
            usize::try_from(catalog_len).map_err(|_overflow| StoreError::BadCatalogLen)?;
        if retained
            .checked_add(catalog_bytes)
            .is_none_or(|peak| peak > limit)
        {
            return Err(metadata_limit_store(limit));
        }
    }

    let mut buf = vec![0_u8; tail.catalog_len as usize];
    reader.read_exact_at(&mut buf, catalog_at)?;
    Ok(EncodedCatalog {
        bytes: buf,
        body_end: catalog_at,
    })
}

#[cfg(test)]
pub(super) fn read_validated_zms_summary<R: ReadAt>(
    reader: &R,
    retained_metadata: usize,
    metadata_limit: usize,
) -> Result<CatalogSummary, StoreError> {
    read_zms_summary(
        reader,
        retained_metadata,
        metadata_limit,
        FinishedValidation::Complete,
    )
}

pub(super) fn read_zms_summary<R: ReadAt>(
    reader: &R,
    retained_metadata: usize,
    metadata_limit: usize,
    validation: FinishedValidation,
) -> Result<CatalogSummary, StoreError> {
    #[cfg(test)]
    CATALOG_SUMMARY_READS.with(|reads| reads.set(reads.get().saturating_add(1)));

    let encoded = read_encoded_catalog(reader, Some((retained_metadata, metadata_limit)))?;
    verify_zms_magic(reader)?;
    let view = Catalog::view(&encoded.bytes).map_err(StoreError::Catalog)?;
    let entry_allocation = view
        .entries()
        .len()
        .checked_mul(size_of::<Entry>())
        .ok_or(StoreError::BadCatalogLen)?;
    let transient = retained_metadata
        .checked_add(encoded.bytes.len())
        .and_then(|bytes| bytes.checked_add(entry_allocation))
        .ok_or_else(|| metadata_limit_store(metadata_limit))?;
    if transient > metadata_limit {
        return Err(metadata_limit_store(metadata_limit));
    }
    let catalog = Catalog {
        entries: view.entries().collect(),
        min_ts: view.min_ts,
        max_ts: view.max_ts,
        format_version: view.format_version,
        window_count: view.window_count,
    };
    if catalog.format_version != FORMAT_VERSION {
        return Err(StoreError::UnsupportedFormat {
            version: catalog.format_version,
        });
    }
    validate_catalog_layout(&catalog, encoded.body_end).map_err(StoreError::SectionLayout)?;
    if matches!(validation, FinishedValidation::Complete) {
        validate_section_checksums(reader, &catalog)?;
    }
    let catalog_len =
        u32::try_from(encoded.bytes.len()).map_err(|_overflow| StoreError::BadCatalogLen)?;
    Ok(CatalogSummary::from_catalog(&catalog, catalog_len))
}

pub(super) fn validate_section_checksums<R: ReadAt>(
    reader: &R,
    catalog: &Catalog,
) -> Result<(), StoreError> {
    let mut buffer = [0_u8; ZMS_CRC_CHUNK_BYTES];
    for entry in &catalog.entries {
        let mut checksum = Crc32c::new();
        let mut offset = entry.offset;
        let mut remaining = entry.len;
        while remaining != 0 {
            let chunk_len = usize::try_from(remaining.min(ZMS_CRC_CHUNK_BYTES as u64)).map_err(
                |_overflow| {
                    StoreError::Io(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "ZMS checksum chunk length overflow",
                    ))
                },
            )?;
            reader.read_exact_at(&mut buffer[..chunk_len], offset)?;
            checksum.update(&buffer[..chunk_len]);
            offset = offset.checked_add(chunk_len as u64).ok_or_else(|| {
                StoreError::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "ZMS checksum offset overflow",
                ))
            })?;
            remaining -= chunk_len as u64;
        }
        if checksum.finalize() != entry.crc32c {
            return Err(StoreError::SectionChecksum {
                type_id: entry.type_id,
            });
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ZmsInvalid {
    pub(super) reason: InvalidZmsReason,
    pub(super) failure: Option<StoreIoFailure>,
}

pub(super) fn classify_zms_validation(
    file: &File,
    expected: FileIdentity,
    address: kronika_layout::SegmentAddress,
    validation: Result<CatalogSummary, StoreError>,
) -> io::Result<Result<CatalogSummary, ZmsInvalid>> {
    // Identity instability always wins over a stable-invalid verdict. A
    // replacement or in-place rewrite must be retried from discovery instead
    // of excluding whichever generation happened to be read.
    match FileIdentity::from_file(file) {
        Ok(actual) if actual == expected => {}
        Ok(_changed) => {
            return Err(stale_finished_zms(address, "during complete validation"));
        }
        Err(source)
            if matches!(
                source.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::UnexpectedEof
            ) =>
        {
            return Err(stale_finished_zms(address, "during complete validation"));
        }
        Err(source) if source.kind() == io::ErrorKind::OutOfMemory => return Err(source),
        Err(source) => {
            return Ok(Err(ZmsInvalid {
                reason: InvalidZmsReason::Io,
                failure: Some(StoreIoFailure::from_error(
                    StoreIoOperation::Metadata,
                    &source,
                )),
            }));
        }
    }
    match validation {
        Ok(summary) => Ok(Ok(summary)),
        Err(StoreError::Io(source)) if source.kind() == io::ErrorKind::UnexpectedEof => {
            Ok(Err(ZmsInvalid {
                reason: InvalidZmsReason::Truncated,
                failure: Some(StoreIoFailure::from_error(StoreIoOperation::Read, &source)),
            }))
        }
        Err(StoreError::Io(source)) if source.kind() == io::ErrorKind::OutOfMemory => Err(source),
        Err(StoreError::Io(source)) => Ok(Err(ZmsInvalid {
            reason: InvalidZmsReason::Io,
            failure: Some(StoreIoFailure::from_error(StoreIoOperation::Read, &source)),
        })),
        Err(StoreError::TooSmall) => Ok(invalid_zms(InvalidZmsReason::TooSmall)),
        Err(StoreError::BadMagic) => Ok(invalid_zms(InvalidZmsReason::BadMagic)),
        Err(StoreError::TailIndex(_)) => Ok(invalid_zms(InvalidZmsReason::TailIndex)),
        Err(StoreError::UnsupportedFormat { .. }) => {
            Ok(invalid_zms(InvalidZmsReason::UnsupportedFormat))
        }
        Err(StoreError::BadCatalogLen) => Ok(invalid_zms(InvalidZmsReason::BadCatalogLength)),
        Err(StoreError::Catalog(_)) => Ok(invalid_zms(InvalidZmsReason::Catalog)),
        Err(StoreError::SectionLayout(_) | StoreError::OutOfBounds) => {
            Ok(invalid_zms(InvalidZmsReason::CanonicalLayout))
        }
        Err(StoreError::SectionChecksum { .. }) => {
            Ok(invalid_zms(InvalidZmsReason::SectionChecksum))
        }
        Err(error @ (StoreError::Layout(_) | StoreError::ActivePartTooLarge { .. })) => {
            Err(store_io(error))
        }
    }
}

pub(super) const fn invalid_zms(reason: InvalidZmsReason) -> Result<CatalogSummary, ZmsInvalid> {
    Err(ZmsInvalid {
        reason,
        failure: None,
    })
}

pub(super) fn verify_zms_magic<R: ReadAt>(reader: &R) -> Result<(), StoreError> {
    let mut magic = [0_u8; MAGIC.len()];
    reader.read_exact_at(&mut magic, 0)?;
    if magic == MAGIC {
        Ok(())
    } else {
        Err(StoreError::BadMagic)
    }
}

/// Read and decode the end catalog from any [`ReadAt`] source.
///
/// Reads only the tail index and catalog block; no section bodies are loaded.
/// Directory discovery uses a compact summary instead; this function remains
/// available for callers that explicitly need an owned catalog.
///
/// # Errors
///
/// Returns [`StoreError`] when the source is too small, the magic bytes are
/// wrong, the format version is unsupported, the catalog block cannot be
/// located, the catalog bytes are corrupt, or a catalog entry points outside
/// the section area.
pub fn read_catalog<R: ReadAt>(reader: &R) -> Result<Catalog, StoreError> {
    let encoded = read_encoded_catalog(reader, None)?;
    let catalog = Catalog::decode(&encoded.bytes).map_err(StoreError::Catalog)?;
    verify_zms_magic(reader)?;
    if catalog.format_version != FORMAT_VERSION {
        return Err(StoreError::UnsupportedFormat {
            version: catalog.format_version,
        });
    }

    validate_catalog_layout(&catalog, encoded.body_end).map_err(StoreError::SectionLayout)?;

    Ok(catalog)
}
