//! Content-coding preferences for runtime API responses.

use hyper::HeaderMap;
use hyper::header::ACCEPT_ENCODING;

/// The response codings the request permits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AcceptedEncodings {
    gzip: bool,
    identity: bool,
}

/// The coding selected for one response body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContentCoding {
    Identity,
    Gzip,
}

impl Default for AcceptedEncodings {
    fn default() -> Self {
        Self {
            gzip: true,
            identity: true,
        }
    }
}

impl AcceptedEncodings {
    /// Parse every field line as one combined `Accept-Encoding` list.
    pub(crate) fn from_headers(headers: &HeaderMap) -> Option<Self> {
        let mut saw_header = false;
        let mut gzip = None;
        let mut identity = None;
        let mut wildcard = None;

        for value in headers.get_all(ACCEPT_ENCODING) {
            saw_header = true;
            let value = value.to_str().ok()?;
            for item in value.split(',') {
                let coding = item.split(';').next().unwrap_or_default().trim();
                if coding.is_empty() {
                    continue;
                }
                let quality = quality(item).unwrap_or(0);
                if coding.eq_ignore_ascii_case("gzip") {
                    keep_highest(&mut gzip, quality);
                } else if coding.eq_ignore_ascii_case("identity") {
                    keep_highest(&mut identity, quality);
                } else if coding == "*" {
                    keep_highest(&mut wildcard, quality);
                }
            }
        }

        if !saw_header {
            return Some(Self::default());
        }

        let gzip = gzip.or(wildcard).is_some_and(|quality| quality > 0);
        let identity = identity.map_or_else(|| wildcard != Some(0), |quality| quality > 0);
        (gzip || identity).then_some(Self { gzip, identity })
    }

    pub(super) const fn for_large(self) -> ContentCoding {
        if self.gzip {
            ContentCoding::Gzip
        } else {
            ContentCoding::Identity
        }
    }

    pub(super) const fn for_small(self) -> ContentCoding {
        if self.identity {
            ContentCoding::Identity
        } else {
            ContentCoding::Gzip
        }
    }
}

impl ContentCoding {
    pub(crate) const fn header(self) -> Option<&'static str> {
        match self {
            Self::Identity => None,
            Self::Gzip => Some("gzip"),
        }
    }
}

fn keep_highest(slot: &mut Option<u16>, quality: u16) {
    *slot = Some(slot.map_or(quality, |present| present.max(quality)));
}

fn quality(item: &str) -> Option<u16> {
    let mut quality = 1_000;
    let mut saw_quality = false;
    for parameter in item.split(';').skip(1) {
        let (name, value) = parameter.split_once('=')?;
        if saw_quality || !name.trim().eq_ignore_ascii_case("q") {
            return None;
        }
        quality = qvalue(value.trim())?;
        saw_quality = true;
    }
    Some(quality)
}

fn qvalue(value: &str) -> Option<u16> {
    let (whole, fraction) = value.split_once('.').map_or((value, ""), |parts| parts);
    if fraction.len() > 3 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    match whole {
        "0" => {
            let mut thousandths = 0_u16;
            for (index, byte) in fraction.bytes().enumerate() {
                let digit = u16::from(byte - b'0');
                thousandths += digit * [100, 10, 1][index];
            }
            Some(thousandths)
        }
        "1" if fraction.bytes().all(|byte| byte == b'0') => Some(1_000),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
