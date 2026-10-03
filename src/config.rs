//! Where forests and shared caches live, and how caches are grafted.

use std::env;
use std::path::PathBuf;

use crate::error::{Result, bail};

pub struct Config {
    /// Directory holding one subdirectory per forest.
    pub forest_root: PathBuf,
    /// Directory holding the caches that `share` mode links into trees.
    pub cache_dir: PathBuf,
    /// Size in bytes from which a grafted cache file is hardlinked rather than
    /// copied.
    pub link_min: u64,
}

impl Config {
    /// `$WORKFOREST_ROOT`, defaulting to `~/.workforest`; `$WORKFOREST_CACHE`,
    /// defaulting to `$XDG_CACHE_HOME/workforest`, else `~/.cache/workforest`;
    /// and `$WORKFOREST_CACHE_LINK_MIN`, defaulting to 64 KiB.
    pub fn from_env() -> Result<Config> {
        let forest_root = match env_path("WORKFOREST_ROOT") {
            Some(dir) => dir,
            None => home()?.join(".workforest"),
        };
        let cache_dir = match env_path("WORKFOREST_CACHE") {
            Some(dir) => dir,
            None => match env_path("XDG_CACHE_HOME") {
                Some(dir) if dir.is_absolute() => dir.join("workforest"),
                _ => home()?.join(".cache").join("workforest"),
            },
        };
        let link_min = match env::var_os("WORKFOREST_CACHE_LINK_MIN") {
            Some(value) if !value.is_empty() => {
                let value = value.to_string_lossy();
                match value.parse() {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        bail!("WORKFOREST_CACHE_LINK_MIN must be a number of bytes, not '{value}'")
                    }
                }
            }
            _ => 64 * 1024,
        };
        Ok(Config {
            forest_root,
            cache_dir,
            link_min,
        })
    }
}

/// The path in an environment variable that is set and not empty.
fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// `$HOME`, which must be set.
fn home() -> Result<PathBuf> {
    match env_path("HOME") {
        Some(home) => Ok(home),
        None => bail!("HOME is not set"),
    }
}
