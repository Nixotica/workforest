//! Where forests live, and how build caches are grafted into them.

use std::env;
use std::path::PathBuf;

use crate::error::{Result, bail};

pub struct Config {
    /// Directory holding one subdirectory per forest.
    pub forest_root: PathBuf,
}

impl Config {
    /// `$WORKFOREST_ROOT`, defaulting to `~/.workforest`.
    pub fn from_env() -> Result<Config> {
        let forest_root = match env::var_os("WORKFOREST_ROOT") {
            Some(dir) if !dir.is_empty() => PathBuf::from(dir),
            _ => home()?.join(".workforest"),
        };
        Ok(Config { forest_root })
    }
}

/// `$HOME`, which must be set.
fn home() -> Result<PathBuf> {
    match env::var_os("HOME") {
        Some(home) if !home.is_empty() => Ok(PathBuf::from(home)),
        _ => bail!("HOME is not set"),
    }
}

/// `$WORKFOREST_CACHE_LINK_MIN`, defaulting to 64 KiB: the size in bytes from
/// which a grafted cache file is hardlinked rather than copied. Only commands
/// that graft read it, so a bad value can't stop the others.
pub fn link_min() -> Result<u64> {
    match env::var_os("WORKFOREST_CACHE_LINK_MIN") {
        Some(value) if !value.is_empty() => {
            let value = value.to_string_lossy();
            match value.parse() {
                Ok(bytes) => Ok(bytes),
                Err(_) => {
                    bail!("WORKFOREST_CACHE_LINK_MIN must be a number of bytes, not '{value}'")
                }
            }
        }
        _ => Ok(64 * 1024),
    }
}
