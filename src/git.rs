//! Thin wrappers over the `git` command line.

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use crate::error::{Context, Result, bail};

fn git(dir: &Path) -> Command {
    let mut command = Command::new("git");
    command.arg("-C").arg(dir).stdin(Stdio::null());
    command
}

/// The raw stdout of a git command that succeeded.
fn stdout<I, S>(dir: &Path, args: I) -> Option<Vec<u8>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let out = git(dir).args(args).stderr(Stdio::null()).output().ok()?;
    out.status.success().then_some(out.stdout)
}

/// The stdout of a git command that succeeded, without its trailing newline.
pub fn output<I, S>(dir: &Path, args: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let out = stdout(dir, args)?;
    let text = String::from_utf8_lossy(&out);
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

/// Whether `dir` is the top of a git checkout: it holds a `.git` directory, or
/// a `.git` file, as a linked worktree does.
pub fn is_checkout(dir: &Path) -> bool {
    dir.join(".git").symlink_metadata().is_ok()
}

/// The main worktree of the repo at `path`.
pub fn main_worktree(path: &Path) -> Result<PathBuf> {
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
    if let Some(base) = default_branch(repo) {
        return base;
    }
    for candidate in ["main", "master"] {
        if branch_exists(repo, candidate) {
            return candidate.to_owned();
        }
    }
    output(repo, ["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_else(|| "HEAD".to_owned())
}

/// The branch `origin/HEAD` points at: where a repo's finished work ends up.
/// Unlike [`default_base`], it never falls back to a local branch.
pub fn default_branch(dir: &Path) -> Option<String> {
    let origin_head = [
        "symbolic-ref",
        "--quiet",
        "--short",
        "refs/remotes/origin/HEAD",
    ];
    output(dir, origin_head)
}

/// The default branch, when it stands in for `base` as where work on `base`
/// lands: once `base` is gone. A stacked branch's base is deleted when it
/// merges, and the forge retargets the stacked pull request to the default
/// branch. A base that lives on, such as a release branch, has no stand-in,
/// even when the default branch has the same change, as it does for a
/// backport.
pub fn base_stand_in(dir: &Path, base: &str) -> Option<String> {
    if resolves(dir, base) {
        return None;
    }
    default_branch(dir).filter(|default| default != base)
}

/// Whether `rev` names a commit in the repo at `dir`.
pub fn resolves(dir: &Path, rev: &str) -> bool {
    let commit = format!("{rev}^{{commit}}");
    succeeds(dir, ["rev-parse", "--verify", "--quiet", &commit])
}

pub fn branch_exists(repo: &Path, branch: &str) -> bool {
    let reference = format!("refs/heads/{branch}");
    succeeds(repo, ["show-ref", "--verify", "--quiet", &reference])
}

/// Add a worktree of `repo` at `dest` on `branch`, creating the branch from
/// `base` unless it already exists. Unless `sparse` is empty, the worktree
/// checks out only those directories, plus the repo's top-level files, as a
/// cone-mode sparse checkout of its own: the main checkout and other
/// worktrees keep theirs.
pub fn add_worktree(
    repo: &Path,
    dest: &Path,
    branch: &str,
    base: &str,
    sparse: &[String],
) -> Result<()> {
    let mut args = vec![
        OsStr::new("worktree"),
        OsStr::new("add"),
        OsStr::new("--quiet"),
    ];
    if !sparse.is_empty() {
        args.push(OsStr::new("--no-checkout"));
    }
    let creating = !branch_exists(repo, branch);
    if !creating {
        args.extend([dest.as_os_str(), OsStr::new(branch)]);
    } else {
        args.extend([
            OsStr::new("-b"),
            OsStr::new(branch),
            dest.as_os_str(),
            OsStr::new(base),
        ]);
    }
    run(repo, args)?;
    if sparse.is_empty() {
        return Ok(());
    }
    // `init --cone` then `set`, since `set --cone` is newer than git 2.35.
    let mut set = vec!["sparse-checkout", "set"];
    set.extend(sparse.iter().map(String::as_str));
    let checked_out = run(dest, ["sparse-checkout", "init", "--cone"])
        .and_then(|()| run(dest, &set))
        .and_then(|()| run(dest, ["checkout", "--quiet"]));
    if checked_out.is_err() {
        // Leave nothing half made.
        let _ = remove_worktree(repo, dest, true);
        if creating {
            delete_branch(repo, branch);
        }
    }
    checked_out
}

/// The directories the worktree at `dir` checks out, if it is a sparse
/// checkout: its sparse-checkout patterns, which in cone mode are directories.
pub fn sparse_dirs(dir: &Path) -> Option<Vec<String>> {
    let sparse = output(dir, ["config", "--get", "--bool", "core.sparseCheckout"]);
    if sparse.as_deref() != Some("true") {
        return None;
    }
    let list = output(dir, ["sparse-checkout", "list"])?;
    Some(list.lines().map(str::to_owned).collect())
}

/// Add a detached worktree of `repo` at `dest`, on the commit `repo` has
/// checked out.
pub fn add_detached_worktree(repo: &Path, dest: &Path) -> Result<()> {
    let args = [
        OsStr::new("worktree"),
        OsStr::new("add"),
        OsStr::new("--quiet"),
        OsStr::new("--detach"),
        dest.as_os_str(),
        OsStr::new("HEAD"),
    ];
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

/// The git common dir of the repo at `repo`: shared by all its worktrees, and
/// where machine-local state such as `workforest-cache` lives.
pub fn common_dir(repo: &Path) -> Option<PathBuf> {
    output(repo, ["rev-parse", "--git-common-dir"]).map(|dir| repo.join(dir))
}

/// Whether git ignores `path` in the worktree at `dir`. `as_dir` asks about a
/// directory at `path`, which need not exist yet, rather than a file or symlink.
pub fn ignores(dir: &Path, path: &str, as_dir: bool) -> bool {
    let path = if as_dir {
        format!("{path}/")
    } else {
        path.to_owned()
    };
    succeeds(dir, ["check-ignore", "--quiet", "--", &path])
}

/// Whether git tracks anything at or under `path` in the worktree at `dir`.
/// Anything git can't answer counts as tracked.
pub fn tracks(dir: &Path, path: &str) -> bool {
    let pathspec = format!(":(literal){path}");
    output(dir, ["ls-files", "--", &pathspec]).is_none_or(|files| !files.is_empty())
}

/// The files of the worktree at `dir` that git tracks, in its checked-out
/// submodules too, or would track if added: those it doesn't ignore. Paths are
/// relative to `dir`.
pub fn files(dir: &Path) -> Option<Vec<PathBuf>> {
    // git can't list the files of submodules and untracked ones at once.
    let tracked = stdout(dir, ["ls-files", "-z", "--cached", "--recurse-submodules"])?;
    let untracked = stdout(dir, ["ls-files", "-z", "--others", "--exclude-standard"])?;
    let files = tracked
        .split(|&byte| byte == 0)
        .chain(untracked.split(|&byte| byte == 0))
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(OsStr::from_bytes(path)))
        .collect();
    Some(files)
}

/// Whether the worktree at `dir` has uncommitted changes, if git can tell.
pub fn is_dirty(dir: &Path) -> Option<bool> {
    output(dir, ["status", "--porcelain"]).map(|changes| !changes.is_empty())
}

/// The number of commits in `range`, if git can resolve it.
pub fn count(dir: &Path, range: &str) -> Option<u64> {
    output(dir, ["rev-list", "--count", range])?.parse().ok()
}

/// Whether the branch at `dir` has an upstream that git can resolve, which it
/// stops having once a fetch prunes the remote branch it tracked.
pub fn has_upstream(dir: &Path) -> bool {
    succeeds(dir, ["rev-parse", "--abbrev-ref", "@{upstream}"])
}

/// Why deleting the tree at `dir` would lose work, if it would: uncommitted
/// changes, or commits that its upstream (else `base`) lacks and that have not
/// [`landed`] on `base`, or on the default branch standing in for it (see
/// [`base_stand_in`]). Anything git cannot answer counts as a risk.
pub fn unlanded_work(dir: &Path, base: &str) -> Option<String> {
    if !dir.is_dir() {
        return None;
    }
    match is_dirty(dir) {
        Some(false) => {}
        Some(true) => return Some("uncommitted changes".to_owned()),
        None => return Some("a git status that could not be read".to_owned()),
    }
    let risk = if has_upstream(dir) {
        match count(dir, "@{upstream}..HEAD") {
            Some(0) => return None,
            Some(ahead) => format!("{ahead} unpushed commit(s), not landed on {base}"),
            None => "commits that could not be compared with its upstream".to_owned(),
        }
    } else {
        match count(dir, &format!("{base}..HEAD")) {
            Some(0) => return None,
            Some(ahead) => format!("{ahead} commit(s) not pushed anywhere or landed on {base}"),
            None => format!("commits that could not be compared with {base}"),
        }
    };
    let landed_anywhere = landed(dir, base, "HEAD")
        || base_stand_in(dir, base).is_some_and(|stand_in| landed(dir, &stand_in, "HEAD"));
    (!landed_anywhere).then_some(risk)
}

/// Whether `tip`, such as `HEAD`, holds work of its own that is all on `base`
/// by now. With commits ahead of `base`, that is whether they have [`landed`].
/// With none ahead, the branch `tip` is on was either merged as it was, or
/// has no work of its own, as when it was planted a moment ago or reset to its
/// base: its reflog tells them apart by whether a commit made on it is on
/// `base`. Without a branch, there is no reflog to tell, so nothing has landed.
pub fn work_landed(dir: &Path, base: &str, tip: &str, branch: Option<&str>) -> bool {
    match count(dir, &format!("{base}..{tip}")) {
        Some(0) => branch.is_some_and(|branch| merged_commits(dir, branch, base)),
        Some(_) => landed(dir, base, tip),
        None => false,
    }
}

/// Whether `branch`'s reflog records a commit made on it that is on `base`, as
/// opposed to only its creation, moves such as resets, pulls and
/// fast-forwards, and commits since discarded. A reflog that can't be read
/// records nothing.
fn merged_commits(dir: &Path, branch: &str, base: &str) -> bool {
    const MADE: [&str; 4] = ["commit", "cherry-pick", "revert", "am"];
    let reflog = format!("refs/heads/{branch}");
    let Some(entries) = output(dir, ["log", "--walk-reflogs", "--format=%H %gs", &reflog]) else {
        return false;
    };
    entries.lines().any(|entry| {
        // Each subject starts with what moved the branch: `commit: <message>`,
        // `commit (amend): <message>`, `reset: moving to <ref>`, and so on.
        let (commit, subject) = entry.split_once(' ').unwrap_or((entry, ""));
        let action = subject.split([':', ' ']).next().unwrap_or_default();
        MADE.contains(&action) && succeeds(dir, ["merge-base", "--is-ancestor", commit, base])
    })
}

/// The branch checked out in the worktree at `dir`, unless its HEAD is detached.
pub fn current_branch(dir: &Path) -> Option<String> {
    output(dir, ["symbolic-ref", "--quiet", "--short", "HEAD"])
}

/// The directory where a repo keeps its record of the linked worktree at `dir`:
/// where it points back at the worktree, and whether it is locked.
pub fn worktree_record(dir: &Path) -> Option<PathBuf> {
    output(dir, ["rev-parse", "--absolute-git-dir"]).map(PathBuf::from)
}

/// The ref a tree's work lands on: its base, unless that is the tree's own
/// branch on a remote, as when a tree is planted to carry on with a pushed
/// branch. Reaching that only means the work was pushed, so the repo's
/// default base stands in for it.
pub fn landing_base(repo: &Path, base: &str, branch: &str) -> String {
    let own = remote_of(repo, base).is_some_and(|remote| {
        let tracking = base.strip_prefix("refs/remotes/").unwrap_or(base);
        tracking.strip_prefix(&format!("{remote}/")) == Some(branch)
    });
    if own {
        default_base(repo)
    } else {
        base.to_owned()
    }
}

/// The remote whose tracking branch `base` names, if it names one. A base that
/// no longer resolves, as after its remote branch was deleted, is read by name.
pub fn remote_of(repo: &Path, base: &str) -> Option<String> {
    let full = output(repo, ["rev-parse", "--symbolic-full-name", base])
        .filter(|full| !full.is_empty())
        .unwrap_or_else(|| base.to_owned());
    let tracking = full.strip_prefix("refs/remotes/").unwrap_or(&full);
    // Remote names may contain slashes, so take the longest that fits.
    output(repo, ["remote"])?
        .lines()
        .filter(|remote| tracking.starts_with(&format!("{remote}/")))
        .max_by_key(|remote| remote.len())
        .map(str::to_owned)
}

/// Fetch `remote` into `repo`, pruning the branches deleted from it, letting
/// git's errors through to stderr, and report whether that worked. Git never
/// stops to ask for credentials.
pub fn fetch(repo: &Path, remote: &str) -> bool {
    git(repo)
        .args(["fetch", "--quiet", "--prune", remote])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdout(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Whether everything `tip`, such as `HEAD` or a branch, changed since it left
/// `base` is already on `base`, as it is after a regular, squash or rebase
/// merge: its whole diff applies to `base` in reverse. The check runs against a
/// throwaway index, so it checks nothing out and writes nothing to the repo.
///
/// A reverse apply needs each hunk's context to match exactly, so it misses
/// work once the base has changed lines next to it, as lockfiles and busy files
/// see all the time. A squash merge is still recognised then, by [`squashed`].
/// Anything git cannot answer counts as not landed.
pub fn landed(dir: &Path, base: &str, tip: &str) -> bool {
    let Some(fork) = output(dir, ["merge-base", base, tip]) else {
        return false;
    };
    let Some(patch) = stdout(dir, ["diff-tree", "-r", "-p", "--binary", &fork, tip]) else {
        return false;
    };
    if patch.is_empty() {
        return true;
    }
    let index = TempIndex::new();
    let read = git(dir)
        .env("GIT_INDEX_FILE", &index.0)
        .args(["read-tree", base])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    (read && applies_in_reverse(dir, &index.0, &patch)) || squashed(dir, &fork, base, tip)
}

/// Whether a commit on `base` since `fork` makes exactly the changes `tip`
/// made since `fork`, as a squash merge does, and no later commit on `base`
/// reverts them exactly. Changes are compared by `git patch-id`, without
/// context lines, so lines `base` has changed next to them since don't matter.
/// Older gits give every change to a binary file the same patch id, so binary
/// files must also match by content.
fn squashed(dir: &Path, fork: &str, base: &str, tip: &str) -> bool {
    let only_change = |from: &str, to: &str| {
        let ids = patch_ids(dir, &["diff", "--no-renames", "-U0", from, to])?;
        ids.into_iter().next().map(|(id, _)| id)
    };
    let (Some(made), Some(undone)) = (only_change(fork, tip), only_change(tip, fork)) else {
        return false;
    };
    let Some(paths) = stdout(
        dir,
        ["diff", "--no-renames", "--name-only", "-z", fork, tip],
    ) else {
        return false;
    };
    let range = format!("{fork}..{base}");
    let mut log: Vec<&OsStr> = [
        "log",
        "--reverse",
        "--no-renames",
        "-U0",
        "-p",
        "--format=commit %H",
        &range,
        "--",
    ]
    .into_iter()
    .map(OsStr::new)
    .collect();
    log.extend(nul_separated(&paths));
    let Some(commits) = patch_ids(dir, &log) else {
        return false;
    };
    // Oldest first, so that a revert cancels the squash before it, and a
    // later squash of the same changes counts again.
    let mut squash = None;
    for (id, commit) in commits {
        if id == made {
            squash = Some(commit);
        } else if id == undone {
            squash = None;
        }
    }
    squash.is_some_and(|squash| binaries_match(dir, fork, tip, &squash))
}

/// Whether `commit` has each binary file that `tip` changed since `fork` just
/// as `tip` has it.
fn binaries_match(dir: &Path, fork: &str, tip: &str, commit: &str) -> bool {
    let Some(stats) = stdout(dir, ["diff", "--no-renames", "--numstat", "-z", fork, tip]) else {
        return false;
    };
    let blob = |rev: &str, path: &[u8]| {
        let mut spec = OsString::from(format!("{rev}:"));
        spec.push(OsStr::from_bytes(path));
        output(
            dir,
            [
                OsStr::new("rev-parse"),
                OsStr::new("--verify"),
                OsStr::new("--quiet"),
                &spec,
            ],
        )
    };
    nul_separated(&stats)
        .filter_map(|stat| stat.as_bytes().strip_prefix(b"-\t-\t"))
        .all(|path| blob(commit, path) == blob(tip, path))
}

/// The non-empty entries of git's `-z` output.
fn nul_separated(list: &[u8]) -> impl Iterator<Item = &OsStr> {
    list.split(|&byte| byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(OsStr::from_bytes)
}

/// The patch ids `git patch-id --stable` gives the output of `git <args>`,
/// each with the commit it belongs to: one per commit of a `git log -p`, or one
/// for a plain diff, which has none. An empty diff has no patch id.
fn patch_ids<S: AsRef<OsStr>>(dir: &Path, args: &[S]) -> Option<Vec<(String, String)>> {
    // After a hunk's line numbers, git adds the nearest line above it that
    // looks like a function, which depends on lines the change didn't touch,
    // and older gits count it in the patch id. It is dropped here.
    let patches: Vec<u8> = stdout(dir, args)?
        .split_inclusive(|&byte| byte == b'\n')
        .flat_map(|line| match line.strip_prefix(b"@@ ") {
            Some(rest) => match rest.windows(3).position(|end| end == b" @@") {
                Some(end) => [b"@@ ", &rest[..end + 3], b"\n"].concat(),
                None => line.to_vec(),
            },
            None => line.to_vec(),
        })
        .collect();
    let mut patch_id = git(dir)
        .args(["patch-id", "--stable"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdin = patch_id.stdin.take()?;
    // Written from another thread, so that neither side waits on the other.
    let writer = thread::spawn(move || stdin.write_all(&patches));
    let ids = patch_id.wait_with_output().ok()?;
    let fed = writer.join().is_ok_and(|written| written.is_ok());
    if !fed || !ids.status.success() {
        return None;
    }
    let ids = String::from_utf8_lossy(&ids.stdout)
        .lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(id, commit)| (id.to_owned(), commit.to_owned()))
        .collect();
    Some(ids)
}

/// Whether `patch` applies in reverse to the tree in `index`.
fn applies_in_reverse(dir: &Path, index: &Path, patch: &[u8]) -> bool {
    let apply = git(dir)
        .env("GIT_INDEX_FILE", index)
        .args(["apply", "--cached", "--check", "--reverse"])
        .arg("--whitespace=nowarn")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut apply) = apply else {
        return false;
    };
    // Dropping stdin once it is written ends the patch.
    let fed = apply
        .stdin
        .take()
        .is_some_and(|mut stdin| stdin.write_all(patch).is_ok());
    apply.wait().is_ok_and(|status| status.success()) && fed
}

/// A path for a throwaway index file, removed when dropped.
struct TempIndex(PathBuf);

impl TempIndex {
    fn new() -> TempIndex {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let name = format!("workforest-{}-{n}.index", process::id());
        TempIndex(env::temp_dir().join(name))
    }
}

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
