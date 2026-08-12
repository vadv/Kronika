//! Validates and fingerprints the generated forensic interface bundle.

use std::fs;
use std::io::Read as _;
use std::io::{self, ErrorKind};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use flate2::read::GzDecoder;
use sha2::{Digest as _, Sha256};

const UI_GZIP: &str = "ui/kronika-ui.html.gz";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed={UI_GZIP}");
    let compressed = fs::read(UI_GZIP)
        .map_err(|error| io::Error::new(error.kind(), format!("read {UI_GZIP}: {error}")))?;
    validate_header(&compressed)?;

    let mut html = String::new();
    GzDecoder::new(compressed.as_slice())
        .read_to_string(&mut html)
        .map_err(|error| io::Error::new(error.kind(), format!("decode {UI_GZIP}: {error}")))?;
    validate_html(&html)?;

    let etag = format!("\"{}\"", hex(&Sha256::digest(&compressed)));
    let script = between(&html, "<script>", "</script>")?;
    let script_hash = STANDARD.encode(Sha256::digest(script.as_bytes()));
    let csp = format!(
        "default-src 'none'; script-src 'sha256-{script_hash}'; style-src 'unsafe-inline'; \
         font-src data:; img-src data:; connect-src 'self'; base-uri 'none'; \
         form-action 'none'; frame-ancestors 'none'; object-src 'none'"
    );

    println!("cargo:rustc-env=KRONIKA_UI_ETAG={etag}");
    println!("cargo:rustc-env=KRONIKA_UI_CSP={csp}");
    println!("cargo:rustc-env=KRONIKA_UI_GZIP_LEN={}", compressed.len());
    Ok(())
}

fn validate_header(bytes: &[u8]) -> io::Result<()> {
    if bytes.get(..4) != Some(&[0x1f, 0x8b, 8, 0]) {
        return Err(invalid(format!(
            "{UI_GZIP} must be gzip deflate without optional header fields"
        )));
    }
    if bytes.get(4..8) != Some(&[0, 0, 0, 0]) {
        return Err(invalid(format!(
            "{UI_GZIP} must have zero modification time"
        )));
    }
    Ok(())
}

fn validate_html(html: &str) -> io::Result<()> {
    if !html.starts_with("<!doctype html>") {
        return Err(invalid(format!(
            "{UI_GZIP} does not contain the production HTML document"
        )));
    }
    if !html.contains("<script>") || html.matches("</script>").count() != 1 {
        return Err(invalid(format!(
            "{UI_GZIP} must contain one complete inline script"
        )));
    }
    if html.contains("sourceMappingURL") {
        return Err(invalid(format!(
            "{UI_GZIP} contains a source map reference"
        )));
    }
    for remote in [
        "src=\"http://",
        "src=\"https://",
        "href=\"http://",
        "href=\"https://",
        "url(http://",
        "url(https://",
    ] {
        if html.contains(remote) {
            return Err(invalid(format!("{UI_GZIP} contains an external asset URL")));
        }
    }
    Ok(())
}

fn between<'a>(text: &'a str, before: &str, after: &str) -> io::Result<&'a str> {
    let start = text
        .find(before)
        .map(|at| at + before.len())
        .ok_or_else(|| invalid(format!("{UI_GZIP} has no {before}")))?;
    let tail = text
        .get(start..)
        .ok_or_else(|| invalid(format!("{UI_GZIP} has an invalid {before} boundary")))?;
    let end = tail
        .find(after)
        .ok_or_else(|| invalid(format!("{UI_GZIP} has no {after}")))?;
    tail.get(..end)
        .ok_or_else(|| invalid(format!("{UI_GZIP} has an invalid {after} boundary")))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}

fn invalid(message: String) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, message)
}
