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

    let gzip_etag = etag(&compressed);
    let identity_etag = etag(html.as_bytes());
    let script_hashes = script_bodies(&html)?
        .into_iter()
        .map(|script| {
            format!(
                "'sha256-{}'",
                STANDARD.encode(Sha256::digest(script.as_bytes()))
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    let csp = format!(
        "default-src 'none'; script-src {script_hashes}; style-src 'unsafe-inline'; \
         font-src data:; img-src data:; connect-src 'self'; base-uri 'none'; \
         form-action 'none'; frame-ancestors 'none'; object-src 'none'"
    );

    println!("cargo:rustc-env=KRONIKA_UI_GZIP_ETAG={gzip_etag}");
    println!("cargo:rustc-env=KRONIKA_UI_IDENTITY_ETAG={identity_etag}");
    println!("cargo:rustc-env=KRONIKA_UI_CSP={csp}");
    println!("cargo:rustc-env=KRONIKA_UI_GZIP_LEN={}", compressed.len());
    println!("cargo:rustc-env=KRONIKA_UI_IDENTITY_LEN={}", html.len());
    Ok(())
}

fn etag(bytes: &[u8]) -> String {
    format!("\"{}\"", hex(&Sha256::digest(bytes)))
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
    if script_bodies(html)?.len() != 2 || html.matches("</script>").count() != 2 {
        return Err(invalid(format!(
            "{UI_GZIP} must contain two complete inline scripts"
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

fn script_bodies(html: &str) -> io::Result<Vec<&str>> {
    let mut bodies = Vec::new();
    let mut tail = html;
    while let Some(start) = tail.find("<script>") {
        let body = tail
            .get(start + "<script>".len()..)
            .ok_or_else(|| invalid(format!("{UI_GZIP} has an invalid script boundary")))?;
        let (script, rest) = body
            .split_once("</script>")
            .ok_or_else(|| invalid(format!("{UI_GZIP} has an incomplete inline script")))?;
        bodies.push(script);
        tail = rest;
    }
    Ok(bodies)
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
