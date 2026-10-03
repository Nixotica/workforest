//! Copying a cache directory into a tree without copying the bulk of its data.
//!
//! Files of `link_min` bytes and up are hardlinked, so they cost no disk.
//! Everything smaller is copied, private to the tree. That split is a bet, not
//! a guarantee: compiled artifacts are large and get replaced wholesale when
//! rebuilt, so sharing them is free, while the files a build system rewrites in
//! place (fingerprints, dep-info, timestamps, locks) are usually small. A large
//! file that is rewritten in place would reach the main checkout through its
//! hardlink, so files matching an always-copy glob are copied whatever their
//! size, and `cache doctor` is how to find the ones that need it.
//!
//! Copies keep their modification times. Build tools judge freshness by
//! comparing mtimes, so a copy stamped with the time of the graft would look
//! newer than the tree's freshly checked-out sources, and stale output built
//! from other sources would pass as fresh.
//!
//! On Linux, `fs::copy` uses `copy_file_range`, which tries a reflink first, so
//! on btrfs or XFS a copy shares its data until written and costs little disk
//! either. It is counted as copied all the same.

use std::fs::{self, File, FileTimes, Metadata};
use std::io;
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::Path;

use super::glob::Glob;
use super::walk::walk;

/// What a clone cost.
#[derive(Default)]
pub struct Cloned {
    pub files: u64,
    /// Bytes hardlinked, which cost no disk.
    pub linked: u64,
    /// Bytes copied into the tree.
    pub copied: u64,
    /// Symlinks whose absolute target is in the main checkout, so that a
    /// build writing through them changes it.
    pub links_to_main: u64,
}

/// Clone the directory `src`, in the main checkout at `main`, to `dst`, which
/// must not exist yet.
pub fn clone_dir(
    src: &Path,
    dst: &Path,
    main: &Path,
    link_min: u64,
    always_copy: &[String],
) -> io::Result<Cloned> {
    let always_copy: Vec<Glob> = always_copy.iter().map(|glob| Glob::new(glob)).collect();
    fs::create_dir(dst)?;
    let mut cloned = Cloned::default();
    for item in walk(src) {
        let (rel, meta) = item?;
        let (from, to) = (src.join(&rel), dst.join(&rel));
        let kind = meta.file_type();
        if kind.is_dir() {
            fs::create_dir(&to)?;
        } else if kind.is_symlink() {
            let target = fs::read_link(&from)?;
            if target.is_absolute() && target.starts_with(main) {
                cloned.links_to_main += 1;
            }
            symlink(target, &to)?;
            cloned.files += 1;
        } else if kind.is_file() {
            cloned.files += 1;
            let link = meta.len() >= link_min && !always_copy.iter().any(|glob| glob.matches(&rel));
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
    for (_, meta) in walk(dir).flatten() {
        if !meta.is_file() {
            continue;
        }
        usage.files += 1;
        if meta.nlink() > 1 {
            usage.shared += meta.len();
        } else {
            usage.own += meta.len();
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
