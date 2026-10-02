//! The agent skill bundled into the binary.

use std::env;
use std::fs;
use std::path::PathBuf;

use crate::config;
use crate::error::{Context, Result, bail};

/// SKILL.md: the same file the Claude Code plugin ships.
const SKILL_MD: &str = include_str!("../plugin/skills/workforest/SKILL.md");

/// Write the skill to `<dir>/workforest/SKILL.md`, `dir` defaulting to Claude
/// Code's user skills directory.
pub fn install(dir: Option<PathBuf>) -> Result<()> {
    let dir = match dir {
        Some(dir) => dir,
        None => default_dir()?,
    };
    let skill_dir = dir.join("workforest");
    let path = skill_dir.join("SKILL.md");
    if fs::read_to_string(&path).is_ok_and(|installed| installed == SKILL_MD) {
        println!("skill already up to date: {}", path.display());
        return Ok(());
    }
    if path
        .symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink())
    {
        bail!(
            "{} is a symlink, so something else (Home Manager, say) manages it; not replacing it",
            path.display()
        );
    }
    fs::create_dir_all(&skill_dir).context(format!("could not create {}", skill_dir.display()))?;
    fs::write(&path, SKILL_MD).context(format!("could not write {}", path.display()))?;
    println!("installed skill: {}", path.display());
    Ok(())
}

/// `$CLAUDE_CONFIG_DIR/skills`, else `~/.claude/skills`.
fn default_dir() -> Result<PathBuf> {
    match env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) if !dir.is_empty() => Ok(PathBuf::from(dir).join("skills")),
        _ => Ok(config::home()?.join(".claude").join("skills")),
    }
}
