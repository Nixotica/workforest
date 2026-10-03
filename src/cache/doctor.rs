//! `cache doctor`: whether a repo's caches survive being grafted.
//!
//! `clone` is only safe for a cache that still works at another path and whose
//! large files a build replaces rather than rewrites in place. That is tested
//! here rather than assumed: graft the cache into a throwaway worktree, build
//! there, and check that no file in the main checkout's cache changed.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::time::Instant;

use super::{Entry, Mode, declared, ecosystem, human_bytes, usage, warn};
use crate::config::Config;
use crate::error::{Context, Result, bail};
use crate::forest::dir_name;
use crate::git;

/// How many changed files the report names before it stops.
const SHOWN: usize = 10;

/// Test the `clone` caches of the repo whose main worktree is at `source` by
/// building with `cmd`, a shell command, in a throwaway worktree.
pub fn doctor(config: &Config, source: &Path, cmd: Option<&str>) -> Result<()> {
    let repo = dir_name(source);
    let declared = declared(source);
    warn(&declared.warnings);
    let clones: Vec<&Entry> = declared
        .entries
        .iter()
        .filter(|entry| entry.mode == Mode::Clone && source.join(&entry.path).is_dir())
        .collect();
    if clones.is_empty() {
        bail!(
            "{repo} has no clone cache to test: declare one, then build once in the main \
             checkout so there is something to graft"
        );
    }
    // Without --cmd, build the way each detected build system does.
    let builds: Vec<&str> = ecosystem::detected(source).map(|e| e.build).collect();
    let cmd = match cmd {
        Some(cmd) => cmd.to_owned(),
        None if !builds.is_empty() => builds.join(" && "),
        None => bail!("pass --cmd '<build command>' to say how {repo} builds"),
    };

    fs::create_dir_all(&config.forest_root)
        .context(format!("could not create {}", config.forest_root.display()))?;
    let scratch = Scratch::add(
        source,
        config
            .forest_root
            .join(format!(".doctor-{repo}-{}", process::id())),
    )?;
    println!(
        "grafting into a scratch worktree, {}",
        scratch.dir.display()
    );
    let mut grafted = Vec::new();
    for entry in clones {
        match super::graft(config, &scratch.dir, source, entry, true) {
            Ok(note) => {
                println!("  cache {}: {}", entry.path, note.unwrap_or_default());
                if scratch.dir.join(&entry.path).is_dir() {
                    grafted.push(entry);
                }
            }
            Err(err) => eprintln!("workforest: cache {}: {err}", entry.path),
        }
    }
    if grafted.is_empty() {
        bail!("no cache was grafted, so there is nothing to test");
    }
    // Grafting leaves the main checkout's files as they were, inode, mtime and
    // size, so a snapshot now still shows the build's effect alone.
    let before = snapshot(source, &grafted);

    println!("building: {cmd}");
    let start = Instant::now();
    let status = Command::new("sh")
        .arg("-c")
        .arg(&cmd)
        .current_dir(&scratch.dir)
        .stdin(Stdio::null())
        .status()
        .context("could not run sh")?;
    let elapsed = start.elapsed().as_secs_f64();
    let after = snapshot(source, &grafted);
    let changed = changes(&before, &after);

    println!("\n{repo}");
    println!("  {:<20} {status}, {elapsed:.1}s", "build command");
    for entry in &grafted {
        let usage = usage(&scratch.dir.join(&entry.path));
        println!(
            "  {:<20} {} files: {} still hardlinked to the main checkout, {} of its own",
            format!("cache {}", entry.path),
            usage.files,
            human_bytes(usage.shared),
            human_bytes(usage.own)
        );
    }
    if !changed.is_empty() {
        println!(
            "  {:<20} {} file(s) CHANGED by the build in the scratch worktree:",
            "main checkout",
            changed.len()
        );
        for path in changed.iter().take(SHOWN) {
            println!("    {}", path.display());
        }
        if changed.len() > SHOWN {
            println!("    and {} more", changed.len() - SHOWN);
        }
        bail!("{repo}'s cache is not safe to clone as declared");
    }
    if !status.success() {
        bail!("the build failed, so the check proves nothing; pass a --cmd that builds {repo}");
    }
    println!("  {:<20} intact: safe to clone", "main checkout");
    Ok(())
}

/// A file's identity and state: inode, modification time and size.
type Stamp = (u64, i64, i64, u64);

/// Every file in the `clone` caches of the main checkout at `source`.
fn snapshot(source: &Path, clones: &[&Entry]) -> BTreeMap<PathBuf, Stamp> {
    let mut files = BTreeMap::new();
    let mut dirs: Vec<PathBuf> = clones
        .iter()
        .map(|entry| entry.path.clone().into())
        .collect();
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = fs::read_dir(source.join(&dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let rel = dir.join(entry.file_name());
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                dirs.push(rel);
            } else {
                let stamp = (meta.ino(), meta.mtime(), meta.mtime_nsec(), meta.size());
                files.insert(rel, stamp);
            }
        }
    }
    files
}

/// Files added, removed or changed between two snapshots.
fn changes(before: &BTreeMap<PathBuf, Stamp>, after: &BTreeMap<PathBuf, Stamp>) -> Vec<PathBuf> {
    let mut changed: Vec<PathBuf> = after
        .iter()
        .filter(|(path, stamp)| before.get(*path) != Some(stamp))
        .map(|(path, _)| path.clone())
        .collect();
    changed.extend(
        before
            .keys()
            .filter(|path| !after.contains_key(*path))
            .cloned(),
    );
    changed.sort();
    changed
}

/// A throwaway worktree, removed when dropped.
struct Scratch {
    source: PathBuf,
    dir: PathBuf,
}

impl Scratch {
    /// A detached worktree of `source` at `dir`, on the commit `source` has
    /// checked out, which is the one its cache was most likely built from.
    fn add(source: &Path, dir: PathBuf) -> Result<Scratch> {
        git::add_detached_worktree(source, &dir)?;
        Ok(Scratch {
            source: source.to_path_buf(),
            dir,
        })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if git::remove_worktree(&self.source, &self.dir, true).is_err() {
            let _ = fs::remove_dir_all(&self.dir);
            let _ = git::prune_worktrees(&self.source);
        }
    }
}
