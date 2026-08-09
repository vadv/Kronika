//! Environment-only configuration, read once before the first request.
//!
//! A value that does not parse stops the process instead of falling back to a
//! default: a dashboard bound to the wrong address is worse than one that did
//! not start.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context as _, Result};

/// Where the server listens when nothing says otherwise.
const DEFAULT_LISTEN: &str = "127.0.0.1:8080";

/// The validated contract.
#[derive(Debug, Clone)]
pub(crate) struct Config {
    /// Data root: the segments, the journal, and the index files.
    pub(crate) data_root: PathBuf,
    /// Address to listen on.
    pub(crate) listen: SocketAddr,
    /// The one account allowed in, if any.
    pub(crate) account: Option<Account>,
}

/// The credentials a request has to carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Account {
    /// User name.
    pub(crate) user: String,
    /// Password.
    pub(crate) password: String,
}

impl Config {
    /// Read and validate the contract from the environment.
    ///
    /// # Errors
    ///
    /// Returns an error when `KRONIKA_OUT_DIR` is unset, the listen address
    /// does not parse, or only one half of the account is set.
    pub(crate) fn from_env() -> Result<Self> {
        let data_root: PathBuf = std::env::var("KRONIKA_OUT_DIR")
            .context("KRONIKA_OUT_DIR is not set")?
            .into();
        let raw =
            std::env::var("KRONIKA_WEB_LISTEN").unwrap_or_else(|_unset| DEFAULT_LISTEN.to_owned());
        let listen = raw
            .parse()
            .with_context(|| format!("KRONIKA_WEB_LISTEN={raw:?} is not an address and port"))?;
        Ok(Self {
            data_root,
            listen,
            account: account(
                std::env::var("KRONIKA_WEB_USER").ok(),
                std::env::var("KRONIKA_WEB_PASSWORD").ok(),
            )?,
        })
    }
}

/// The account, or nothing when neither half is set.
///
/// Half an account is a typo, not a decision to serve without one.
fn account(user: Option<String>, password: Option<String>) -> Result<Option<Account>> {
    match (user, password) {
        (Some(user), Some(password)) => Ok(Some(Account { user, password })),
        (None, None) => Ok(None),
        (Some(_user), None) => {
            anyhow::bail!("KRONIKA_WEB_USER is set and KRONIKA_WEB_PASSWORD is not")
        }
        (None, Some(_password)) => {
            anyhow::bail!("KRONIKA_WEB_PASSWORD is set and KRONIKA_WEB_USER is not")
        }
    }
}

#[cfg(test)]
mod tests;
