//! Following a growing file through a fixed buffer.

use std::fs::File;
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};

use memchr::memchr;

/// Bytes handed to the kernel per `read` call. The file itself can be any
/// size; this is all of it that is ever in memory at once.
const READ_BUF_BYTES: usize = 65_536;

/// Longest physical line kept whole. What follows the cut is dropped up to the
/// next newline.
pub const MAX_LINE_BYTES: usize = 65_536;

/// Most bytes one [`Tail::read`] takes from the file. A log that grows faster
/// than the collector reads it is read at this rate until it catches up.
pub const MAX_READ_BYTES: usize = 4 * 1_048_576;

/// Where a source left off in a file.
///
/// `dev` and `inode` identify the file the offset belongs to, so a rotated or
/// truncated file is read from its start instead of from a stale offset.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Position {
    /// Device id of the file the offset was taken in.
    pub dev: u64,
    /// Inode of the file the offset was taken in.
    pub inode: u64,
    /// Byte offset of the first unread line.
    pub offset: u64,
}

/// One logical record: the line that opens it and the lines that continue it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// The opening line and its continuations, in file order.
    pub lines: Vec<String>,
}

impl Record {
    /// The line that opens the record.
    #[must_use]
    pub fn first(&self) -> &str {
        self.lines.first().map_or("", String::as_str)
    }

    /// The continuation lines, in file order.
    #[must_use]
    pub fn rest(&self) -> &[String] {
        self.lines.get(1..).unwrap_or_default()
    }

    /// The whole record with its original newlines.
    #[must_use]
    pub fn joined(&self) -> String {
        self.lines.join("\n")
    }
}

/// Decides whether `line` continues the record read so far.
///
/// Framing is the parser's rule: `stderr` marks continuations with a tab,
/// `csvlog` continues while a quoted field is still open, and `jsonlog` never
/// continues at all.
pub type Continues = fn(open: &[String], line: &str) -> bool;

/// Follows one file, remembering where it left off.
///
/// The file is opened, read from the remembered offset, and closed on every
/// [`read`](Tail::read). A record left open at the end of a read is carried
/// into the next one, so a `DETAIL:` line written just after the read boundary
/// still reaches the record it belongs to.
#[derive(Debug)]
pub struct Tail {
    path: PathBuf,
    position: Position,
    open: Vec<String>,
}

impl Tail {
    /// Follow `path`, resuming from `position`.
    #[must_use]
    pub const fn new(path: PathBuf, position: Position) -> Self {
        Self {
            path,
            position,
            open: Vec::new(),
        }
    }

    /// The path being followed.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The offset to resume from, for persisting across restarts.
    #[must_use]
    pub const fn position(&self) -> Position {
        self.position
    }

    /// Read every complete record that arrived since the last call.
    ///
    /// # Errors
    ///
    /// Returns the operating system's error for opening, stating, seeking or
    /// reading the file, including the missing file a log that has not been
    /// created yet gives.
    pub fn read(&mut self, continues: Continues) -> io::Result<Vec<Record>> {
        let mut file = File::open(&self.path)?;
        let metadata = file.metadata()?;
        let (dev, inode) = identity(&metadata);
        let size = metadata.len();

        // A different file under the same name, or the same file cut back to a
        // shorter length: either way the remembered offset means nothing.
        if dev != self.position.dev || inode != self.position.inode || size < self.position.offset {
            self.position = Position {
                dev,
                inode,
                offset: 0,
            };
            self.open.clear();
        }

        file.seek(SeekFrom::Start(self.position.offset))?;
        let mut records = Vec::new();
        let lines = self.read_lines(&mut file, size)?;
        let read_any = !lines.is_empty();
        for line in lines {
            if self.open.is_empty() || !continues(&self.open, &line) {
                flush(&mut self.open, &mut records);
            }
            self.open.push(line);
        }
        // Nothing arrived for a whole interval, so nothing is going to continue
        // the record that is still open.
        if !read_any {
            flush(&mut self.open, &mut records);
        }
        Ok(records)
    }

    /// Read complete lines from the current offset, advancing it past each one.
    fn read_lines(&mut self, file: &mut File, size: u64) -> io::Result<Vec<String>> {
        let mut buf = vec![0_u8; READ_BUF_BYTES];
        let mut line: Vec<u8> = Vec::new();
        // Raw bytes of the line in progress, the ones a length cut dropped
        // included, so the offset can move past exactly what was read.
        let mut consumed = 0_usize;
        let mut dropping = false;
        let mut lines = Vec::new();
        let mut taken = 0_usize;

        while taken < MAX_READ_BYTES && self.position.offset + as_u64(consumed) < size {
            let want = READ_BUF_BYTES.min(MAX_READ_BYTES - taken);
            let read = file.read(&mut buf[..want])?;
            if read == 0 {
                break;
            }
            taken += read;
            let mut at = 0_usize;
            while at < read {
                let chunk = buf.get(at..read).unwrap_or_default();
                let Some(end) = memchr(b'\n', chunk) else {
                    keep(&mut line, chunk, &mut dropping);
                    consumed += chunk.len();
                    break;
                };
                keep(
                    &mut line,
                    chunk.get(..end).unwrap_or_default(),
                    &mut dropping,
                );
                consumed += end + 1;
                at += end + 1;
                // Only a line the newline closed is complete, so only then does
                // the offset move past it.
                self.position.offset += as_u64(consumed);
                consumed = 0;
                dropping = false;
                push_line(std::mem::take(&mut line), &mut lines);
            }
        }
        Ok(lines)
    }
}

fn as_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

/// Append what fits of `chunk`, dropping the rest of an over-long line.
fn keep(line: &mut Vec<u8>, chunk: &[u8], dropping: &mut bool) {
    if *dropping {
        return;
    }
    let room = MAX_LINE_BYTES - line.len();
    if chunk.len() <= room {
        line.extend_from_slice(chunk);
        return;
    }
    line.extend_from_slice(chunk.get(..room).unwrap_or_default());
    *dropping = true;
}

/// Turn a raw line into text, dropping what is not valid UTF-8.
fn push_line(mut raw: Vec<u8>, lines: &mut Vec<String>) {
    if raw.last() == Some(&b'\r') {
        raw.pop();
    }
    if raw.is_empty() {
        return;
    }
    if let Ok(text) = String::from_utf8(raw) {
        lines.push(text);
    }
}

fn flush(open: &mut Vec<String>, records: &mut Vec<Record>) {
    if !open.is_empty() {
        records.push(Record {
            lines: std::mem::take(open),
        });
    }
}

#[cfg(unix)]
fn identity(metadata: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt as _;
    (metadata.dev(), metadata.ino())
}

#[cfg(not(unix))]
const fn identity(_metadata: &std::fs::Metadata) -> (u64, u64) {
    (0, 0)
}
