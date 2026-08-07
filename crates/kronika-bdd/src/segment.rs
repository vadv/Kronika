//! Reading a finished `.zms` back, the way a reader would.
//!
//! Opens the segment from its tail index, decodes the catalog, and hands out
//! sections as column-addressable rows. Nothing here goes through
//! `kronika-store`: a scenario asserts on the file the collector wrote.

use anyhow::{Context, Result, bail};
use kronika_format::{Catalog, TAIL_INDEX_LEN, TailIndex, crc32c};
use kronika_registry::{Bytes, Row, VerifiedSection, decode_rows};
use std::path::{Path, PathBuf};

/// One finished segment, held in memory.
#[derive(Debug)]
pub(crate) struct Segment {
    /// Where the file was read from, for assertion messages.
    pub(crate) path: PathBuf,
    /// The whole file.
    bytes: Vec<u8>,
    /// The decoded end catalog.
    pub(crate) catalog: Catalog,
}

impl Segment {
    /// Read and decode the segment at `path`.
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        if bytes.len() < TAIL_INDEX_LEN {
            bail!("{} is too short to be a segment", path.display());
        }
        let mut tail = [0_u8; TAIL_INDEX_LEN];
        tail.copy_from_slice(&bytes[bytes.len() - TAIL_INDEX_LEN..]);
        let index = TailIndex::decode(tail)
            .map_err(|error| anyhow::anyhow!("{}: {error:?}", path.display()))?;
        let catalog_len = index.catalog_len as usize;
        let catalog_at = bytes
            .len()
            .checked_sub(TAIL_INDEX_LEN + catalog_len)
            .with_context(|| format!("{} claims a catalog longer than itself", path.display()))?;
        let catalog = Catalog::decode(&bytes[catalog_at..bytes.len() - TAIL_INDEX_LEN])
            .map_err(|error| anyhow::anyhow!("{}: {error:?}", path.display()))?;
        Ok(Self {
            path: path.to_path_buf(),
            bytes,
            catalog,
        })
    }

    /// Rows recorded for `type_id`, or `None` when the section is absent.
    pub(crate) fn rows_of(&self, type_id: u32) -> Option<u32> {
        self.catalog
            .entries
            .iter()
            .find(|entry| entry.type_id == type_id)
            .map(|entry| entry.rows)
    }

    /// Decode a section into column-addressable rows.
    ///
    /// Returns an empty vector when the segment carries no such section.
    pub(crate) fn decode(&self, type_id: u32) -> Result<Vec<Row>> {
        let Some(entry) = self
            .catalog
            .entries
            .iter()
            .find(|entry| entry.type_id == type_id)
        else {
            return Ok(Vec::new());
        };
        let at = usize::try_from(entry.offset).context("section offset exceeds usize")?;
        let len = usize::try_from(entry.len).context("section length exceeds usize")?;
        let body = Bytes::copy_from_slice(&self.bytes[at..at + len]);
        let verified = VerifiedSection::verify(body, entry.crc32c, crc32c)
            .map_err(|error| anyhow::anyhow!("{}: {error}", self.path.display()))?;
        decode_rows(type_id, verified)
            .map_err(|error| anyhow::anyhow!("{} section {type_id}: {error}", self.path.display()))
    }
}

/// Every `.zms` under `root`, oldest first by file name.
///
/// Segment file names are the collection timestamp, so name order is time
/// order.
pub(crate) fn segments_under(root: &Path) -> Result<Vec<Segment>> {
    let mut paths = Vec::new();
    collect_zms(root, &mut paths);
    paths.sort();
    if paths.is_empty() {
        bail!("no segment was written under {}", root.display());
    }
    paths.iter().map(|path| Segment::open(path)).collect()
}

fn collect_zms(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_zms(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "zms") {
            found.push(path);
        }
    }
}
