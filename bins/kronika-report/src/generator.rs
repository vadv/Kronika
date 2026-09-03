//! Atomic assembly of one self-contained report document.

use std::ffi::OsStr;
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD;
use base64::write::EncoderWriter;
use flate2::read::GzDecoder;
use kronika_index::{Index, SeriesKey};
use kronika_layout::{LayoutError, SegmentAddress, SegmentId};
use kronika_query::{SOURCE_OS, SOURCE_POSTGRESQL};
use kronika_reader::{FinishedReader, ReaderError};
use kronika_store::{EmbeddedSource, ResourceError};

const SHELL_GZIP: &[u8] = include_bytes!("../assets/kronika-report-shell.html.gz");
const WASM_GLUE: &[u8] = include_bytes!("../assets/kronika-report-wasm.js");
#[allow(
    clippy::large_include_file,
    reason = "the generator embeds the complete compressed query module in its HTML output"
)]
const WASM_GZIP: &[u8] = include_bytes!("../assets/kronika-report-wasm.wasm.gz");
const RUNTIME_MARKER: &[u8] = b"/*KRONIKA_REPORT_RUNTIME*/";
const TEMP_PREFIX: &str = ".kronika-report-";
const RUNTIME_START: &[u8] = br#";(()=>{const b=s=>Uint8Array.from(atob(s),c=>c.charCodeAt(0));globalThis.__KRONIKA_REPORT_RUNTIME__={ready:(async()=>{const z=b(""#;
const RUNTIME_INDEX: &[u8] = br#""),i=b(""#;
const RUNTIME_WASM: &[u8] = br#""),g=b(""#;
const RUNTIME_ID: &[u8] = br#"");const r=new Uint8Array(await new Response(new Blob([g]).stream().pipeThrough(new DecompressionStream("gzip"))).arrayBuffer()),m=await WebAssembly.compile(r);await KronikaReportWasm.initEmbedded(m);return new KronikaReportWasm.ReportSession(""#;
const RUNTIME_SOURCES: &[u8] = br#"",z,i,"#;
const RUNTIME_LENGTH: &[u8] = br#",BigInt(""#;
const RUNTIME_END: &[u8] = br#""));})()};})();"#;

/// Failure while validating inputs, deriving the index, or publishing HTML.
#[derive(Debug)]
pub(crate) enum GenerateError {
    InvalidInputName(PathBuf),
    InvalidOutputName(PathBuf),
    InputTooLarge(PathBuf),
    Input { path: PathBuf, source: io::Error },
    Layout(LayoutError),
    Resource(ResourceError),
    Reader(ReaderError),
    Build(kronika_index::BuildError),
    Index(kronika_index::IndexError),
    Asset(io::Error),
    InvalidAsset(&'static str),
    InvalidResourceCount(usize),
    Output { path: PathBuf, source: io::Error },
}

impl std::fmt::Display for GenerateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInputName(path) => write!(
                f,
                "{} is not a canonical signed-decimal .zms path",
                path.display()
            ),
            Self::InvalidOutputName(path) => {
                write!(f, "{} is not an .html path", path.display())
            }
            Self::InputTooLarge(path) => {
                write!(f, "{} is too large for the report ABI", path.display())
            }
            Self::Input { path, source } => {
                write!(f, "read {}: {source}", path.display())
            }
            Self::Layout(source) => source.fmt(f),
            Self::Resource(source) => source.fmt(f),
            Self::Reader(source) => source.fmt(f),
            Self::Build(source) => source.fmt(f),
            Self::Index(source) => source.fmt(f),
            Self::Asset(source) => write!(f, "read the embedded report shell: {source}"),
            Self::InvalidAsset(message) => {
                write!(f, "invalid embedded report asset: {message}")
            }
            Self::InvalidResourceCount(count) => {
                write!(f, "the embedded ZMS produced {count} resources")
            }
            Self::Output { path, source } => {
                write!(f, "write {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for GenerateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Input { source, .. } | Self::Output { source, .. } | Self::Asset(source) => {
                Some(source)
            }
            Self::Layout(source) => Some(source),
            Self::Resource(source) => Some(source),
            Self::Reader(source) => Some(source),
            Self::Build(source) => Some(source),
            Self::Index(source) => Some(source),
            Self::InvalidInputName(_)
            | Self::InvalidOutputName(_)
            | Self::InputTooLarge(_)
            | Self::InvalidAsset(_)
            | Self::InvalidResourceCount(_) => None,
        }
    }
}

impl From<LayoutError> for GenerateError {
    fn from(source: LayoutError) -> Self {
        Self::Layout(source)
    }
}

impl From<ResourceError> for GenerateError {
    fn from(source: ResourceError) -> Self {
        Self::Resource(source)
    }
}

impl From<ReaderError> for GenerateError {
    fn from(source: ReaderError) -> Self {
        Self::Reader(source)
    }
}

impl From<kronika_index::BuildError> for GenerateError {
    fn from(source: kronika_index::BuildError) -> Self {
        Self::Build(source)
    }
}

impl From<kronika_index::IndexError> for GenerateError {
    fn from(source: kronika_index::IndexError) -> Self {
        Self::Index(source)
    }
}

/// Generate and atomically replace one standalone report.
pub(crate) fn generate(input: &Path, output: &Path) -> Result<(), GenerateError> {
    let segment_id = segment_id(input)?;
    if output.extension() != Some(OsStr::new("html")) {
        return Err(GenerateError::InvalidOutputName(output.to_path_buf()));
    }
    let zms = std::fs::read(input).map_err(|source| GenerateError::Input {
        path: input.to_path_buf(),
        source,
    })?;
    let zms_len = u64::try_from(zms.len())
        .map_err(|_overflow| GenerateError::InputTooLarge(input.to_path_buf()))?;
    let shell = shell()?;
    let marker = marker(&shell)?;
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::Builder::new()
        .prefix(TEMP_PREFIX)
        .tempfile_in(parent)
        .map_err(|source| output_error(output, source))?;

    write_output(&mut temporary, output, &shell[..marker])?;
    write_output(&mut temporary, output, WASM_GLUE)?;
    write_output(&mut temporary, output, RUNTIME_START)?;
    write_base64(&mut temporary, output, &zms)?;
    write_output(&mut temporary, output, RUNTIME_INDEX)?;

    let (idx, configured_sources) = isolated_index(segment_id, zms, zms_len)?;
    write_base64(&mut temporary, output, &idx)?;
    write_output(&mut temporary, output, RUNTIME_WASM)?;
    write_base64(&mut temporary, output, WASM_GZIP)?;
    write_output(&mut temporary, output, RUNTIME_ID)?;
    write!(temporary, "{segment_id}").map_err(|source| output_error(output, source))?;
    write_output(&mut temporary, output, RUNTIME_SOURCES)?;
    write!(temporary, "{configured_sources}").map_err(|source| output_error(output, source))?;
    write_output(&mut temporary, output, RUNTIME_LENGTH)?;
    write!(temporary, "{zms_len}").map_err(|source| output_error(output, source))?;
    write_output(&mut temporary, output, RUNTIME_END)?;
    write_output(
        &mut temporary,
        output,
        &shell[marker + RUNTIME_MARKER.len()..],
    )?;
    temporary
        .flush()
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|source| output_error(output, source))?;
    temporary
        .persist(output)
        .map_err(|error| output_error(output, error.error))?;
    Ok(())
}

fn segment_id(input: &Path) -> Result<SegmentId, GenerateError> {
    let Some(file_name) = input.file_name().and_then(OsStr::to_str) else {
        return Err(GenerateError::InvalidInputName(input.to_path_buf()));
    };
    let Some(decimal) = file_name.strip_suffix(".zms") else {
        return Err(GenerateError::InvalidInputName(input.to_path_buf()));
    };
    let raw = decimal
        .parse::<i64>()
        .map_err(|_error| GenerateError::InvalidInputName(input.to_path_buf()))?;
    let segment_id = SegmentId::new(raw)?;
    if SegmentAddress::new(segment_id)?.zms_name() != file_name {
        return Err(GenerateError::InvalidInputName(input.to_path_buf()));
    }
    Ok(segment_id)
}

fn isolated_index(
    segment_id: SegmentId,
    zms: Vec<u8>,
    zms_len: u64,
) -> Result<(Vec<u8>, u32), GenerateError> {
    let source = EmbeddedSource::from_owned(segment_id, zms, zms_len)?;
    let reader = FinishedReader::new(source);
    let listing = reader.resources()?;
    let [resource] = listing.resources.as_slice() else {
        return Err(GenerateError::InvalidResourceCount(listing.resources.len()));
    };
    let segment = reader.open_segment(resource)?;
    let index = kronika_index::build(&segment)?;
    let configured_sources = configured_sources(&index);
    Ok((index.encode()?, configured_sources))
}

fn configured_sources(index: &Index) -> u32 {
    let postgresql = index
        .blocks
        .iter()
        .any(|block| block.key() == SeriesKey::POSTGRES_HEALTH);
    if postgresql {
        SOURCE_OS | SOURCE_POSTGRESQL
    } else {
        SOURCE_OS
    }
}

fn shell() -> Result<Vec<u8>, GenerateError> {
    let mut shell = Vec::new();
    GzDecoder::new(SHELL_GZIP)
        .read_to_end(&mut shell)
        .map_err(GenerateError::Asset)?;
    Ok(shell)
}

fn marker(shell: &[u8]) -> Result<usize, GenerateError> {
    let mut markers = shell
        .windows(RUNTIME_MARKER.len())
        .enumerate()
        .filter_map(|(offset, bytes)| (bytes == RUNTIME_MARKER).then_some(offset));
    let Some(marker) = markers.next() else {
        return Err(GenerateError::InvalidAsset("the runtime marker is absent"));
    };
    if markers.next().is_some() {
        return Err(GenerateError::InvalidAsset(
            "the runtime marker occurs more than once",
        ));
    }
    Ok(marker)
}

fn write_base64(
    output: &mut impl io::Write,
    path: &Path,
    bytes: &[u8],
) -> Result<(), GenerateError> {
    let mut encoder = EncoderWriter::new(output, &STANDARD);
    encoder
        .write_all(bytes)
        .and_then(|()| encoder.finish().map(|_output| ()))
        .map_err(|source| output_error(path, source))
}

fn write_output(
    output: &mut impl io::Write,
    path: &Path,
    bytes: &[u8],
) -> Result<(), GenerateError> {
    output
        .write_all(bytes)
        .map_err(|source| output_error(path, source))
}

fn output_error(path: &Path, source: io::Error) -> GenerateError {
    GenerateError::Output {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
#[path = "generator_tests.rs"]
mod tests;
