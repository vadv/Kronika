//! Running the dumper and reading what it printed.
//!
//! Assertions reach a segment through the shipped binary rather than through
//! the reader, so a scenario checks the thing an operator runs.

use crate::BddWorld;
use anyhow::{Context as _, Result};
use std::path::PathBuf;
use std::process::Command;

/// The dumper under test, found the way the collector is.
fn binary() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("KRONIKA_DUMP_BIN") {
        return Ok(PathBuf::from(path));
    }
    let here = std::env::current_exe().context("locate the BDD binary")?;
    let dir = here
        .parent()
        .context("the BDD binary has no parent directory")?;
    Ok(dir.join("kronika-dump"))
}

/// Run the dumper over the run's data root and return what it printed.
pub(crate) fn dump(world: &BddWorld, flags: &[&str]) -> Result<String> {
    let run = world.run.as_ref().context("a collector was started")?;
    let root = run.out_dir();
    let output = Command::new(binary()?)
        .arg(&root)
        .args(flags)
        .output()
        .with_context(|| format!("run the dumper over {}", root.display()))?;
    anyhow::ensure!(
        output.status.success(),
        "the dumper failed over {}: {}",
        root.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// One line of `--json` output, as `(field, value)` pairs.
///
/// The dumper writes flat objects of scalars, so a split on the separators is
/// the whole parser. A nested document would need more; the dumper does not
/// produce one, and a scenario that needed one would be asserting on the wrong
/// thing.
#[derive(Debug, Clone, Default)]
pub(crate) struct Line {
    fields: Vec<(String, String)>,
}

impl Line {
    /// The value of `field`, quotes stripped, or `None` when the line lacks it.
    pub(crate) fn get(&self, field: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(name, _value)| name == field)
            .map(|(_name, value)| value.as_str())
    }

    /// Whether the line carries `field` equal to `value`.
    pub(crate) fn holds(&self, field: &str, value: &str) -> bool {
        self.get(field) == Some(value)
    }

    /// A numeric field.
    pub(crate) fn number(&self, field: &str) -> Option<i64> {
        self.get(field)?.parse().ok()
    }
}

/// Parse the dumper's `--json` output into lines of fields.
pub(crate) fn lines(printed: &str) -> Vec<Line> {
    printed.lines().map(parse_line).collect()
}

/// Every line whose `kind` is `kind`.
pub(crate) fn of_kind(printed: &str, kind: &str) -> Vec<Line> {
    lines(printed)
        .into_iter()
        .filter(|line| line.holds("kind", kind))
        .collect()
}

fn parse_line(raw: &str) -> Line {
    let body = raw.trim().trim_start_matches('{').trim_end_matches('}');
    let mut fields = Vec::new();
    let mut rest = body;
    while let Some(open) = rest.find('"') {
        let after_open = rest.get(open + 1..).unwrap_or_default();
        let Some(close) = unescaped_quote(after_open) else {
            break;
        };
        let name = unescape(after_open.get(..close).unwrap_or_default());
        let tail = after_open.get(close + 1..).unwrap_or_default();
        let Some(colon) = tail.find(':') else {
            break;
        };
        let value_text = tail.get(colon + 1..).unwrap_or_default();
        let (value, consumed) = read_value(value_text);
        fields.push((name, value));
        rest = value_text.get(consumed..).unwrap_or_default();
    }
    Line { fields }
}

/// A value: a quoted string with its escapes undone, or everything up to the
/// next comma.
fn read_value(text: &str) -> (String, usize) {
    if let Some(body) = text.strip_prefix('"')
        && let Some(close) = unescaped_quote(body)
    {
        return (unescape(body.get(..close).unwrap_or_default()), close + 2);
    }
    let end = text.find(',').unwrap_or(text.len());
    (text.get(..end).unwrap_or_default().to_owned(), end)
}

/// The first quote that is not escaped.
fn unescaped_quote(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 2,
            b'"' => return Some(at),
            _other => at += 1,
        }
    }
    None
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }
        match characters.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('u') => {
                let digits: String = characters.by_ref().take(4).collect();
                let point = u32::from_str_radix(&digits, 16).unwrap_or(0);
                out.push(char::from_u32(point).unwrap_or('\u{fffd}'));
            }
            Some(other) => out.push(other),
            None => break,
        }
    }
    out
}

#[cfg(test)]
mod tests;
