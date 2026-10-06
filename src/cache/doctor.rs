//! `cache doctor`: whether a repo's caches survive being grafted.
//!
//! `clone` is only safe for a cache that still works at another path and whose
//! large files a build replaces rather than rewrites in place. The second is
//! tested here rather than assumed: graft the cache into a throwaway worktree,
//! build there, and check that no file in the main checkout's cache changed.
//! The first can't be tested without knowing what the build reads, so the
//! doctor lists the grafted files that name the main checkout's path, which is
//! how a cache records where it was built.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::time::Instant;

use super::glob::Glob;
use super::walk::walk;
use super::{Entry, Mode, clone, count, declared, ecosystem, human_bytes, warn};
use crate::config::Config;
use crate::error::{Context, Result, bail};
use crate::forest::dir_name;
use crate::git;

/// How many files a list in the report names before it stops.
const SHOWN: usize = 10;

/// The largest file searched for the main checkout's path. Larger files are
/// build output, not the configuration and scripts that record where a cache
/// was built.
const SCAN_MAX: u64 = 1024 * 1024;

/// Test the `clone` caches of the repo whose main worktree is at `source` by
/// building with `cmd`, a shell command, in a throwaway worktree.
pub fn doctor(config: &Config, source: &Path, cmd: Option<&str>) -> Result<()> {
    let link_min = config.link_min()?.value;
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
    // Without --cmd, build the way the repo's build system does.
    let Some(cmd) = cmd.or_else(|| ecosystem::detected(source).next().map(|e| e.build)) else {
        bail!("pass --cmd '<build command>' to say how {repo} builds");
    };

    fs::create_dir_all(&config.forest_root)
        .context(format!("could not create {}", config.forest_root.display()))?;
    sweep(config, source, &repo);
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
        match super::graft_from_main(&scratch.dir, source, entry, link_min) {
            Ok(note) => {
                println!("  cache {}: {note}", entry.path);
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
    // Grafting leaves the main checkout's files as they were, so a snapshot
    // now still shows the build's effect alone.
    let main_before = snapshot(source, &grafted);
    let scratch_before = snapshot(&scratch.dir, &grafted);

    println!("building: {cmd}");
    let start = Instant::now();
    let status = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(&scratch.dir)
        .stdin(Stdio::null())
        .status()
        .context("could not run sh")?;
    let elapsed = start.elapsed().as_secs_f64();
    let written_through = changes(&main_before, &snapshot(source, &grafted));
    let rebuilt = changes(&scratch_before, &snapshot(&scratch.dir, &grafted));

    println!("\n{repo}");
    println!("  {:<20} {status}, {elapsed:.1}s", "build command");
    for entry in &grafted {
        let usage = clone::usage(&scratch.dir.join(&entry.path));
        println!(
            "  {:<20} {}: {} still hardlinked to the main checkout, {} of its own",
            format!("cache {}", entry.path),
            count(usage.files, "file"),
            human_bytes(usage.shared),
            human_bytes(usage.own)
        );
    }
    if !written_through.is_empty() {
        println!(
            "  {:<20} {} CHANGED by the build in the scratch worktree:",
            "main checkout",
            count(written_through.len() as u64, "file")
        );
        list(&written_through);
        bail!("{repo}'s cache is not safe to clone as declared");
    }
    if !status.success() {
        bail!("the build failed, so the check proves nothing; pass a --cmd that builds {repo}");
    }
    if rebuilt.is_empty() {
        bail!(
            "the build changed nothing in the grafted cache, so the check proves nothing; \
             pass a --cmd that rebuilds something"
        );
    }
    println!(
        "  {:<20} unchanged: the build wrote nothing through a hardlink",
        "main checkout"
    );
    // Some files a build system leaves in its cache name the main checkout
    // harmlessly, as long as they were grafted rather than written by the
    // build.
    let inert: Vec<(&str, Glob)> = ecosystem::detected(source)
        .flat_map(|e| e.inert.iter().map(|&glob| (e.path, Glob::new(glob))))
        .collect();
    let is_inert = |path: &Path| {
        scratch_before.contains_key(path)
            && inert
                .iter()
                .any(|(cache, glob)| path.strip_prefix(cache).is_ok_and(|rel| glob.matches(rel)))
    };
    let naming: Vec<PathBuf> = naming_main(&scratch.dir, &grafted, source)
        .into_iter()
        .filter(|path| !is_inert(path))
        .collect();
    if naming.is_empty() {
        println!(
            "  {:<20} no grafted file names the main checkout",
            "main checkout's path"
        );
    } else {
        println!(
            "  {:<20} {} naming the main checkout: a build that reads one works on the main \
             checkout's files, not the tree's",
            "main checkout's path",
            count(naming.len() as u64, "file")
        );
        list(&naming);
    }
    Ok(())
}

/// Print the first [`SHOWN`] of `paths`, and how many more there are.
fn list(paths: &[PathBuf]) {
    for path in paths.iter().take(SHOWN) {
        println!("    {}", path.display());
    }
    if paths.len() > SHOWN {
        println!("    and {} more", paths.len() - SHOWN);
    }
}

/// A file's identity, and what a write through a hardlink to it would change:
/// inode, modification time, size and permissions. Not its ctime, which also
/// changes when a build in the tree replaces its own link to the file, as a
/// safe build does.
#[derive(PartialEq)]
struct Stamp {
    ino: u64,
    mtime: (i64, i64),
    size: u64,
    mode: u32,
}

/// Every file in the `clone` caches of the checkout at `root`, by its path
/// relative to `root`.
fn snapshot(root: &Path, clones: &[&Entry]) -> BTreeMap<PathBuf, Stamp> {
    let mut files = BTreeMap::new();
    for entry in clones {
        for (rel, meta) in walk(&root.join(&entry.path)).flatten() {
            if meta.is_dir() {
                continue;
            }
            let stamp = Stamp {
                ino: meta.ino(),
                mtime: (meta.mtime(), meta.mtime_nsec()),
                size: meta.size(),
                mode: meta.mode(),
            };
            files.insert(Path::new(&entry.path).join(rel), stamp);
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

/// The files of at most [`SCAN_MAX`] bytes in the `clone` caches of the
/// scratch worktree at `scratch` that hold the path of the main checkout at
/// `source`, other than as the start of the scratch worktree's own.
fn naming_main(scratch: &Path, clones: &[&Entry], source: &Path) -> Vec<PathBuf> {
    let main = source.as_os_str().as_encoded_bytes();
    let own = scratch.as_os_str().as_encoded_bytes();
    let mut naming = Vec::new();
    for entry in clones {
        let dir = scratch.join(&entry.path);
        for (rel, meta) in walk(&dir).flatten() {
            if !meta.is_file() || meta.len() > SCAN_MAX {
                continue;
            }
            let Ok(bytes) = fs::read(dir.join(&rel)) else {
                continue;
            };
            let names_main = (0..bytes.len())
                .any(|at| bytes[at..].starts_with(main) && !bytes[at..].starts_with(own));
            if names_main {
                naming.push(Path::new(&entry.path).join(rel));
            }
        }
    }
    naming.sort();
    naming
}

/// Remove the scratch worktrees that doctors of `repo` left behind when they
/// were interrupted: those whose process is no longer running.
fn sweep(config: &Config, source: &Path, repo: &str) {
    let prefix = format!(".doctor-{repo}-");
    let Ok(entries) = fs::read_dir(&config.forest_root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|name| name.strip_prefix(&prefix)) else {
            continue;
        };
        if pid.parse::<u32>().is_ok() && !running(pid) {
            println!(
                "removing a scratch worktree an interrupted doctor left, {}",
                entry.path().display()
            );
            drop(Scratch {
                source: source.to_path_buf(),
                dir: entry.path(),
            });
        }
    }
}

/// Whether the process `pid` is running. One that belongs to someone else
/// counts as not, since it can't be a doctor of this user's.
fn running(pid: &str) -> bool {
    Command::new("sh")
        .args(["-c", "kill -0 \"$1\" 2>/dev/null", "sh", pid])
        .stdin(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
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
