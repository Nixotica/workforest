//! Copying a cache directory into a tree without copying the bulk of its data.
//!
//! Files of `link_min` bytes and up are hardlinked, so they cost no disk.
//! Everything smaller is copied, private to the tree. That split is the safety
//! property: compiled artifacts are large and get replaced wholesale when
//! rebuilt, so sharing them is free, while the files a build system rewrites in
//! place (fingerprints, dep-info, timestamps, locks) are small. Files matching an
//! always-copy glob are copied whatever their size.
//!
//! Copies keep their modification times. Build tools judge freshness by
//! comparing mtimes, so a copy stamped with the time of the graft would look
//! newer than the tree's freshly checked-out sources, and stale output built
//! from other sources would pass as fresh.

use std::fs::{self, File, FileTimes, Metadata};
use std::io;
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::{Path, PathBuf};

use super::glob;

/// What a clone cost.
#[derive(Default)]
pub struct Cloned {
    pub files: u64,
    /// Bytes hardlinked, which cost no disk.
    pub linked: u64,
    /// Bytes copied into the tree.
    pub copied: u64,
}

/// Clone the directory `src` to `dst`, which must not exist yet.
pub fn clone_dir(
    src: &Path,
    dst: &Path,
    link_min: u64,
    always_copy: &[String],
) -> io::Result<Cloned> {
    fs::create_dir(dst)?;
    let mut cloned = Cloned::default();
    let mut dirs = vec![PathBuf::new()];
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(src.join(&dir))? {
            let entry = entry?;
            let rel = dir.join(entry.file_name());
            let (from, to) = (src.join(&rel), dst.join(&rel));
            // Unlike fs::metadata, this doesn't follow symlinks.
            let meta = entry.metadata()?;
            let kind = meta.file_type();
            if kind.is_dir() {
                fs::create_dir(&to)?;
                dirs.push(rel);
            } else if kind.is_symlink() {
                symlink(fs::read_link(&from)?, &to)?;
                cloned.files += 1;
            } else if kind.is_file() {
                cloned.files += 1;
                let link =
                    meta.len() >= link_min && !always_copy.iter().any(|g| glob::matches(g, &rel));
                // Hardlinking fails once a file has as many links as the
                // filesystem allows; a copy is always safe.
                if link && fs::hard_link(&from, &to).is_ok() {
                    cloned.linked += meta.len();
                } else {
                    copy_file(&from, &to, &meta)?;
                    cloned.copied += meta.len();
                }
            }
            // Sockets, FIFOs and devices aren't build output; they stay behind.
        }
    }
    Ok(cloned)
}

/// Copy `from`, whose metadata is `meta`, to `to`, keeping its times.
fn copy_file(from: &Path, to: &Path, meta: &Metadata) -> io::Result<()> {
    fs::copy(from, to)?;
    let times = FileTimes::new()
        .set_accessed(meta.accessed()?)
        .set_modified(meta.modified()?);
    // Setting explicit times needs ownership, not write access, so this works
    // on a read-only copy too.
    File::open(to)?.set_times(times)
}

/// How much of the cache at `dir` is shared with another checkout through
/// hardlinks, and how much is its own.
#[derive(Default)]
pub struct Usage {
    pub files: u64,
    pub shared: u64,
    pub own: u64,
}

/// Walk the cache at `dir`. Unreadable parts are left out of the count.
pub fn usage(dir: &Path) -> Usage {
    let mut usage = Usage::default();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                dirs.push(entry.path());
            } else if meta.is_file() {
                usage.files += 1;
                if meta.nlink() > 1 {
                    usage.shared += meta.len();
                } else {
                    usage.own += meta.len();
                }
            }
        }
    }
    usage
}

/// Whether `a` and `b` are on the same filesystem, so one can hardlink into
/// the other. Anything that can't be read counts as not.
pub fn same_filesystem(a: &Path, b: &Path) -> bool {
    match (fs::metadata(a), fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev(),
        _ => false,
    }
}
