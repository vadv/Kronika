//! The one check every request passes, in one place.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use crate::config::Account;

/// Whether `header` carries the account's credentials.
///
/// `None` for the account means the server was started without one and lets
/// every request through.
pub(crate) fn admits(account: Option<&Account>, header: Option<&str>) -> bool {
    let Some(account) = account else {
        return true;
    };
    let Some(offered) = credentials(header) else {
        return false;
    };
    let expected = format!("{}:{}", account.user, account.password);
    same(offered.as_bytes(), expected.as_bytes())
}

/// The `user:password` an `Authorization` header carries, if it carries one.
fn credentials(header: Option<&str>) -> Option<String> {
    let value = header?.strip_prefix("Basic ")?;
    let raw = STANDARD.decode(value.trim()).ok()?;
    String::from_utf8(raw).ok()
}

/// Compare without returning early, so the answer takes the same time whatever
/// the first wrong byte was.
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
