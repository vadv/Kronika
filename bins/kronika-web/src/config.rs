//! Environment-only configuration, validated before the listener starts.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context as _, Result};

const DEFAULT_LISTEN: &str = "127.0.0.1:8080";

/// The validated server contract.
#[derive(Debug, Clone)]
pub(crate) struct Config {
    /// Data root containing journal, segments, and derived indexes.
    pub(crate) data_root: PathBuf,
    /// Address to listen on.
    pub(crate) listen: SocketAddr,
    /// Required account admitted by centralized Basic auth.
    pub(crate) account: Account,
    /// Explicit source bitset persisted in every derived index.
    pub(crate) sources: u32,
}

/// Credentials every request has to carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Account {
    /// User name.
    pub(crate) user: String,
    /// Password.
    pub(crate) password: String,
}

impl Config {
    /// Read and validate the environment contract.
    ///
    /// # Errors
    ///
    /// Returns an error when the data root, either credential, or the source
    /// bitset is absent, or when a configured value is invalid.
    pub(crate) fn from_env() -> Result<Self> {
        let data_root: PathBuf = std::env::var("KRONIKA_OUT_DIR")
            .context("KRONIKA_OUT_DIR is not set")?
            .into();
        let raw_listen =
            std::env::var("KRONIKA_WEB_LISTEN").unwrap_or_else(|_unset| DEFAULT_LISTEN.to_owned());
        let listen = raw_listen.parse().with_context(|| {
            format!("KRONIKA_WEB_LISTEN={raw_listen:?} is not an address and port")
        })?;
        let account = account(
            std::env::var("KRONIKA_WEB_USER").ok(),
            std::env::var("KRONIKA_WEB_PASSWORD").ok(),
        )?;
        let sources = source_set(std::env::var("KRONIKA_WEB_SOURCES").ok())?;
        Ok(Self {
            data_root,
            listen,
            account,
            sources,
        })
    }
}

fn account(user: Option<String>, password: Option<String>) -> Result<Account> {
    let user = user.context("KRONIKA_WEB_USER is not set")?;
    let password = password.context("KRONIKA_WEB_PASSWORD is not set")?;
    if user.is_empty() {
        anyhow::bail!("KRONIKA_WEB_USER is empty");
    }
    if password.is_empty() {
        anyhow::bail!("KRONIKA_WEB_PASSWORD is empty");
    }
    Ok(Account { user, password })
}

fn source_set(raw: Option<String>) -> Result<u32> {
    let raw = raw.context("KRONIKA_WEB_SOURCES is not set")?;
    raw.parse::<u32>()
        .with_context(|| format!("KRONIKA_WEB_SOURCES={raw:?} is not a u32 bitset"))
}

#[cfg(test)]
mod tests;
