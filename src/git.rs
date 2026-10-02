//! Thin wrappers over the `git` command line.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{Context, Result, bail};

fn git(dir: &Path) -> Command {
    let mut command = Command::new("git");
    command.arg("-C").arg(dir).stdin(Stdio::null());
    command
}

/// The stdout of a git command that succeeded, without its trailing newline.
pub fn output<I, S>(dir: &Path, args: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let out = git(dir).args(args).stderr(Stdio::null()).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Some(text.trim_end_matches('\n').to_owned())
}

/// Whether a git command succeeds; its output is discarded.
pub fn succeeds<I, S>(dir: &Path, args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    git(dir)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Run a git command that changes something, letting its errors through to stderr.
fn run<I, S>(dir: &Path, args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args: Vec<_> = args
        .into_iter()
        .map(|arg| arg.as_ref().to_owned())
        .collect();
    let status = git(dir)
        .args(&args)
        .stdout(Stdio::null())
        .status()
        .context("could not run git")?;
    if status.success() {
        return Ok(());
    }
    let shown: Vec<_> = args.iter().map(|arg| arg.to_string_lossy()).collect();
    bail!("`git {}` failed in {}", shown.join(" "), dir.display())
}

/// The main worktree of the repo at `arg`, a path that is absolute or relative
/// to the current directory. A bare name is rejected rather than guessed at:
/// nothing says which directory it would name a repo in.
pub fn main_worktree(arg: &str) -> Result<PathBuf> {
    if !arg.contains('/') && !arg.starts_with('.') {
        bail!("{arg} is a name, not a path: pass the repo's path, such as ./{arg}");
    }
    let path = Path::new(arg);
    if !path.is_dir() {
        bail!("no such repo: {}", path.display());
    }
    if !succeeds(path, ["rev-parse", "--git-dir"]) {
        bail!("not a git repo: {}", path.display());
    }
    let list = output(path, ["worktree", "list", "--porcelain"]).unwrap_or_default();
    match list.lines().find_map(|line| line.strip_prefix("worktree ")) {
        Some(main) => Ok(PathBuf::from(main)),
        None => bail!("could not find the main worktree of {}", path.display()),
    }
}

/// The ref a new branch starts from when no base is given: `origin/HEAD`, else
/// a local `main` or `master`, else whatever the repo has checked out.
pub fn default_base(repo: &Path) -> String {
    let origin_head = [
        "symbolic-ref",
        "--quiet",
        "--short",
        "refs/remotes/origin/HEAD",
    ];
    if let Some(base) = output(repo, origin_head) {
        return base;
    }
    for candidate in ["main", "master"] {
        if branch_exists(repo, candidate) {
            return candidate.to_owned();
        }
    }
    output(repo, ["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_else(|| "HEAD".to_owned())
}

pub fn branch_exists(repo: &Path, branch: &str) -> bool {
    let reference = format!("refs/heads/{branch}");
    succeeds(repo, ["show-ref", "--verify", "--quiet", &reference])
}

/// Add a worktree of `repo` at `dest` on `branch`, creating the branch from
/// `base` unless it already exists.
pub fn add_worktree(repo: &Path, dest: &Path, branch: &str, base: &str) -> Result<()> {
    let mut args = vec![
        OsStr::new("worktree"),
        OsStr::new("add"),
        OsStr::new("--quiet"),
    ];
    if branch_exists(repo, branch) {
        args.extend([dest.as_os_str(), OsStr::new(branch)]);
    } else {
        args.extend([
            OsStr::new("-b"),
            OsStr::new(branch),
            dest.as_os_str(),
            OsStr::new(base),
        ]);
    }
    run(repo, args)
}

/// Remove the worktree at `tree`; `force` discards whatever it holds.
pub fn remove_worktree(repo: &Path, tree: &Path, force: bool) -> Result<()> {
    let mut args = vec![OsStr::new("worktree"), OsStr::new("remove")];
    if force {
        args.push(OsStr::new("--force"));
    }
    args.push(tree.as_os_str());
    run(repo, args)
}

/// Forget `repo`'s worktrees whose directories no longer exist.
pub fn prune_worktrees(repo: &Path) -> Result<()> {
    run(repo, ["worktree", "prune"])
}

/// Delete a local branch, merged or not, reporting whether that worked.
pub fn delete_branch(repo: &Path, branch: &str) -> bool {
    succeeds(repo, ["branch", "-D", branch])
}

/// Whether the worktree at `dir` has uncommitted changes, if git can tell.
pub fn is_dirty(dir: &Path) -> Option<bool> {
    output(dir, ["status", "--porcelain"]).map(|changes| !changes.is_empty())
}

/// The number of commits in `range`, if git can resolve it.
pub fn count(dir: &Path, range: &str) -> Option<u64> {
    output(dir, ["rev-list", "--count", range])?.parse().ok()
}

/// Why deleting the tree at `dir` would lose work, if it would: uncommitted
/// changes, commits its upstream lacks, or, with no upstream, commits `base`
/// lacks. Anything git cannot answer counts as a risk.
pub fn unlanded_work(dir: &Path, base: &str) -> Option<String> {
    if !dir.is_dir() {
        return None;
    }
    match is_dirty(dir) {
        Some(false) => {}
        Some(true) => return Some("uncommitted changes".to_owned()),
        None => return Some("a git status that could not be read".to_owned()),
    }
    if succeeds(dir, ["rev-parse", "--abbrev-ref", "@{upstream}"]) {
        return match count(dir, "@{upstream}..HEAD") {
            Some(0) => None,
            Some(ahead) => Some(format!("{ahead} unpushed commit(s)")),
            None => Some("commits that could not be compared with its upstream".to_owned()),
        };
    }
    match count(dir, &format!("{base}..HEAD")) {
        Some(0) => None,
        Some(ahead) => Some(format!("{ahead} commit(s) not pushed anywhere")),
        None => Some(format!("commits that could not be compared with {base}")),
    }
}
