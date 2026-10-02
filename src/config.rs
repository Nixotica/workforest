//! Where forests live and where repos are found.

use std::env;
use std::path::PathBuf;

use crate::error::{Result, bail};

pub struct Config {
    /// Directory holding one subdirectory per forest.
    pub forest_root: PathBuf,
    /// Directory that bare repo names are resolved against.
    pub repos_root: PathBuf,
}

impl Config {
    /// `$WORKFOREST_ROOT` and `$WORKFOREST_REPOS`, defaulting to `~/.workforest` and `~/repos`.
    pub fn from_env() -> Result<Config> {
        Ok(Config {
            forest_root: dir_from_env("WORKFOREST_ROOT", ".workforest")?,
            repos_root: dir_from_env("WORKFOREST_REPOS", "repos")?,
        })
    }
}

/// `$HOME`, which must be set.
fn home() -> Result<PathBuf> {
    match env::var_os("HOME") {
        Some(home) if !home.is_empty() => Ok(PathBuf::from(home)),
        _ => bail!("HOME is not set"),
    }
}

fn dir_from_env(var: &str, default_in_home: &str) -> Result<PathBuf> {
    match env::var_os(var) {
        Some(dir) if !dir.is_empty() => Ok(PathBuf::from(dir)),
        _ => Ok(home()?.join(default_in_home)),
    }
}
