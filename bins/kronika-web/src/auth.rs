//! The single authentication check every request passes.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use crate::config::Account;

/// Whether `header` carries the configured Basic credentials.
pub(crate) fn admits(account: &Account, header: Option<&str>) -> bool {
    let Some(offered) = credentials(header) else {
        return false;
    };
    let expected = format!("{}:{}", account.user, account.password);
    same(offered.as_bytes(), expected.as_bytes())
}

fn credentials(header: Option<&str>) -> Option<String> {
    let (scheme, value) = header?.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let raw = STANDARD.decode(value.trim()).ok()?;
    String::from_utf8(raw).ok()
}

fn same(offered: &[u8], expected: &[u8]) -> bool {
    if offered.len() != expected.len() {
        return false;
    }
    let mut difference = 0_u8;
    for (left, right) in offered.iter().zip(expected) {
        difference |= left ^ right;
    }
    difference == 0
}

#[cfg(test)]
mod tests;
