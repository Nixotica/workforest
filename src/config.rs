//! Where forests live.

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
