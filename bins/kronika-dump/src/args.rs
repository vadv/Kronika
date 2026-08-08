//! What the command line asked for.

use std::path::PathBuf;

/// The usage line, printed when the arguments do not parse.
pub(crate) const USAGE: &str = "\
usage: kronika-dump <data-dir> [--section <type_id>] [--index] [--json]
                    [--limit <rows>] [--from <ts>] [--to <ts>]

  no flag        every segment with what each section costs
  --section ID   the rows of one section, dictionary ids resolved
  --index        health points, built from the segment
  --json         machine-readable instead of a table
  --limit N      stop after N rows per segment (default 20, 0 means all)
  --from TS      skip segments ending before this unix microsecond
  --to TS        skip segments starting after it
";

/// What to print.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Want {
    /// Sections and their sizes.
    Sizes,
    /// The rows of one section.
    Section(u32),
    /// Health points.
    Index,
}

/// A parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Args {
    /// The data directory to read.
    pub(crate) root: PathBuf,
    /// What to print.
    pub(crate) want: Want,
    /// Print JSON rather than a table.
    pub(crate) json: bool,
    /// Rows per segment, `0` for all of them.
    pub(crate) limit: usize,
    /// Earliest timestamp of interest, unix microseconds.
    pub(crate) from: Option<i64>,
    /// Latest timestamp of interest, unix microseconds.
    pub(crate) to: Option<i64>,
}

/// Parse arguments, without the program name.
///
/// # Errors
///
/// Returns what was wrong with them, ready to print.
pub(crate) fn parse<I: IntoIterator<Item = String>>(arguments: I) -> Result<Args, String> {
    let mut root = None;
    let mut want = Want::Sizes;
    let mut json = false;
    let mut limit = 20;
    let mut from = None;
    let mut to = None;
    let mut rest = arguments.into_iter();
    while let Some(argument) = rest.next() {
        match argument.as_str() {
            "--json" => json = true,
            "--index" => want = Want::Index,
            "--section" => {
                let value = rest.next().ok_or("--section needs a type_id")?;
                let type_id = value
                    .parse()
                    .map_err(|_bad| format!("{value:?} is not a type_id"))?;
                want = Want::Section(type_id);
            }
            "--limit" => {
                let value = rest.next().ok_or("--limit needs a row count")?;
                limit = value
                    .parse()
                    .map_err(|_bad| format!("{value:?} is not a row count"))?;
            }
            "--from" | "--to" => {
                let value = rest
                    .next()
                    .ok_or_else(|| format!("{argument} needs a timestamp"))?;
                let ts = value
                    .parse()
                    .map_err(|_bad| format!("{value:?} is not a timestamp"))?;
                if argument == "--from" {
                    from = Some(ts);
                } else {
                    to = Some(ts);
                }
            }
            flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}")),
            path if root.is_none() => root = Some(PathBuf::from(path)),
            extra => return Err(format!("unexpected argument {extra:?}")),
        }
    }
    Ok(Args {
        root: root.ok_or("a data directory is required")?,
        want,
        json,
        limit,
        from,
        to,
    })
}

#[cfg(test)]
mod tests;
