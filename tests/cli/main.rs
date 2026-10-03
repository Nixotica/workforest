//! End-to-end tests: real git repositories in a temporary directory, driven
//! through the compiled binary.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

mod cache;

/// A throwaway home holding a forest root, a repos root, and the remotes those
/// repos were cloned from.
struct Sandbox {
    _tmp: TempDir,
    root: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("create a temp dir");
        let root = fs::canonicalize(tmp.path()).expect("canonicalize the temp dir");
        for dir in ["home", "forests", "repos", "remotes", "seeds"] {
            fs::create_dir(root.join(dir)).expect("create a sandbox dir");
        }
        Sandbox { _tmp: tmp, root }
    }

    fn forests(&self) -> PathBuf {
        self.root.join("forests")
    }

    fn repos(&self) -> PathBuf {
        self.root.join("repos")
    }

    fn forest(&self, name: &str) -> PathBuf {
        self.forests().join(name)
    }

    /// Keep a command away from the user's git config, git repos and settings.
    fn isolate<'a>(&self, command: &'a mut Command) -> &'a mut Command {
        let home = self.root.join("home");
        command
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", home.join(".gitconfig"))
            .env("GIT_CEILING_DIRECTORIES", &self.root)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .env("WORKFOREST_ROOT", self.forests())
            .env("WORKFOREST_CACHE", self.root.join("cache"))
            .env_remove("WORKFOREST_CACHE_LINK_MIN")
            .env_remove("XDG_CACHE_HOME")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let output = self
            .isolate(Command::new("git").arg("-C").arg(dir).args(args))
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_owned()
    }

    fn workforest(&self, cwd: &Path, args: &[&str]) -> Output {
        self.isolate(&mut Command::new(env!("CARGO_BIN_EXE_workforest")))
            .current_dir(cwd)
            .args(args)
            .output()
            .expect("run workforest")
    }

    /// Run workforest expecting success, and return its stdout.
    fn ok(&self, cwd: &Path, args: &[&str]) -> String {
        let output = self.workforest(cwd, args);
        assert!(
            output.status.success(),
            "workforest {args:?} failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Run workforest expecting failure, and return its stderr.
    fn fails(&self, cwd: &Path, args: &[&str]) -> String {
        let output = self.workforest(cwd, args);
        assert!(
            !output.status.success(),
            "workforest {args:?} should have failed:\n{}",
            String::from_utf8_lossy(&output.stdout)
        );
        String::from_utf8_lossy(&output.stderr).into_owned()
    }

    /// A repo under the repos root, cloned from its own bare remote, with one
    /// commit on `main` and `origin/HEAD` set.
    fn repo(&self, name: &str) -> PathBuf {
        let seed = self.root.join("seeds").join(name);
        self.init(&seed);
        let remote = self.root.join("remotes").join(format!("{name}.git"));
        self.git(
            &self.root,
            &["clone", "--quiet", "--bare", path(&seed), path(&remote)],
        );
        let repo = self.repos().join(name);
        self.git(
            &self.root,
            &["clone", "--quiet", path(&remote), path(&repo)],
        );
        repo
    }

    /// A repo with one commit on `main` and no remote.
    fn init(&self, dir: &Path) {
        self.git(
            &self.root,
            &["init", "--quiet", "--initial-branch=main", path(dir)],
        );
        self.commit(dir, "README");
    }

    fn commit(&self, dir: &Path, file: &str) {
        fs::write(dir.join(file), format!("{file}\n")).expect("write a file");
        self.git(dir, &["add", file]);
        self.git(dir, &["commit", "--quiet", "--message", file]);
    }

    /// A forest with one tree of `repo`, holding two commits pushed to a
    /// branch of their own, as for a pull request.
    fn pull_request(&self, forest: &str, repo: &str) -> PathBuf {
        self.ok(&self.root, &["new", forest, &format!("repos/{repo}")]);
        let tree = self.forest(forest).join(repo);
        self.commit(&tree, &format!("{forest}-1.txt"));
        self.commit(&tree, &format!("{forest}-2.txt"));
        self.git(&tree, &["push", "--quiet", "-u", "origin", "HEAD"]);
        tree
    }

    /// A clone of `repo`'s remote standing in for a forge such as GitHub, on
    /// `main` as the remote has it now.
    fn forge(&self, repo: &str) -> PathBuf {
        let forge = self.root.join("forge").join(repo);
        if forge.exists() {
            self.git(&forge, &["fetch", "--quiet", "origin"]);
            self.git(
                &forge,
                &["checkout", "--quiet", "-B", "main", "origin/main"],
            );
        } else {
            let remote = self.root.join("remotes").join(format!("{repo}.git"));
            self.git(
                &self.root,
                &["clone", "--quiet", path(&remote), path(&forge)],
            );
        }
        forge
    }

    /// Merge `branch` into `main` on `repo`'s remote the way a forge does,
    /// after `main` has moved on, then delete the branch as forges do on merge.
    fn merge_on_remote(&self, repo: &str, branch: &str, merge: Merge) {
        let forge = self.forge(repo);
        let theirs = format!("origin/{branch}");
        self.commit(&forge, &format!("{branch}-meanwhile.txt"));
        match merge {
            Merge::Regular => {
                self.git(
                    &forge,
                    &["merge", "--quiet", "--no-ff", "--no-edit", &theirs],
                );
            }
            Merge::Squash => {
                self.git(&forge, &["merge", "--quiet", "--squash", &theirs]);
                self.git(&forge, &["commit", "--quiet", "--message", "squash"]);
            }
            Merge::Rebase => {
                self.git(&forge, &["checkout", "--quiet", "-B", "landing", &theirs]);
                self.git(&forge, &["rebase", "--quiet", "main"]);
                self.git(&forge, &["checkout", "--quiet", "main"]);
                self.git(&forge, &["merge", "--quiet", "--ff-only", "landing"]);
            }
        }
        self.push_main_deleting(&forge, branch);
    }

    /// Push the forge's `main`, deleting `branch` from the remote.
    fn push_main_deleting(&self, forge: &Path, branch: &str) {
        let delete = format!(":{branch}");
        self.git(forge, &["push", "--quiet", "origin", "main", &delete]);
    }
}

/// How a forge merges a pull request.
#[derive(Clone, Copy, Debug)]
enum Merge {
    Regular,
    Squash,
    Rebase,
}

fn path(path: &Path) -> &str {
    path.to_str().expect("sandbox paths are UTF-8")
}

#[test]
fn new_plants_one_tree_per_repo_on_a_shared_branch() {
    let sb = Sandbox::new();
    let api = sb.repo("api");
    let web = sb.repo("web");

    let out = sb.ok(&sb.root, &["new", "feature", "repos/api", "repos/web"]);

    assert!(out.starts_with(&format!(
        "new forest feature at {}\n",
        sb.forest("feature").display()
    )));
    assert!(out.contains(&format!(
        "planted api -> {} (branch feature, off origin/main)\n",
        sb.forest("feature").join("api").display()
    )));
    for repo in ["api", "web"] {
        let tree = sb.forest("feature").join(repo);
        assert_eq!(sb.git(&tree, &["branch", "--show-current"]), "feature");
        assert_eq!(
            sb.git(&tree, &["rev-parse", "HEAD"]),
            sb.git(&tree, &["rev-parse", "origin/main"])
        );
    }
    let manifest = fs::read_to_string(sb.forest("feature").join(".workforest")).unwrap();
    assert_eq!(
        manifest,
        format!(
            "api\t{}\tfeature\torigin/main\nweb\t{}\tfeature\torigin/main\n",
            api.display(),
            web.display()
        )
    );
    assert_eq!(sb.git(&api, &["branch", "--show-current"]), "main");
}

#[test]
fn planting_copies_no_history() {
    let sb = Sandbox::new();
    let api = sb.repo("api");
    let objects_before = sb.git(&api, &["count-objects", "-v"]);

    sb.ok(&sb.root, &["new", "light", "repos/api"]);

    assert_eq!(sb.git(&api, &["count-objects", "-v"]), objects_before);
    let tree = sb.forest("light").join("api");
    assert!(
        tree.join(".git").is_file(),
        "a tree's .git points at its repo"
    );
}

#[test]
fn forests_live_in_dot_workforest_by_default() {
    let sb = Sandbox::new();
    sb.repo("api");

    let output = sb
        .isolate(&mut Command::new(env!("CARGO_BIN_EXE_workforest")))
        .env_remove("WORKFOREST_ROOT")
        .current_dir(&sb.root)
        .args(["new", "hidden", "repos/api"])
        .output()
        .expect("run workforest");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(sb.root.join("home/.workforest/hidden/api").is_dir());
}

#[test]
fn forest_names_must_be_single_visible_path_components() {
    let sb = Sandbox::new();
    for name in ["", ".", "..", ".hidden", "a/b"] {
        let err = sb.fails(&sb.root, &["new", name]);
        assert!(err.contains("invalid forest name"), "{name:?}: {err}");
    }
    sb.ok(&sb.root, &["new", "twice"]);
    assert!(
        sb.fails(&sb.root, &["new", "twice"])
            .contains("forest already exists")
    );
}

#[test]
fn plant_targets_the_forest_containing_the_current_directory() {
    let sb = Sandbox::new();
    let api = sb.repo("api");
    let web = sb.repo("web");
    sb.ok(&sb.root, &["new", "here"]);
    let forest = sb.forest("here");

    sb.ok(&forest, &["plant", path(&api)]);
    let nested = forest.join("api").join("src");
    fs::create_dir(&nested).unwrap();
    sb.ok(&nested, &["add", path(&web)]);

    assert!(forest.join("web").is_dir());
    let outside = sb.fails(&sb.root, &["plant", path(&web)]);
    assert!(
        outside.contains("not inside a forest; pass -f <forest>"),
        "{outside}"
    );
    let again = sb.fails(&sb.root, &["plant", "-f", "here", path(&web)]);
    assert!(again.contains("already planted"), "{again}");
}

#[test]
fn plant_checks_out_an_existing_branch_or_creates_one_off_the_base() {
    let sb = Sandbox::new();
    let api = sb.repo("api");
    sb.git(&api, &["checkout", "--quiet", "-b", "topic"]);
    sb.commit(&api, "topic.txt");
    sb.git(&api, &["checkout", "--quiet", "main"]);
    let topic = sb.git(&api, &["rev-parse", "topic"]);

    sb.ok(
        &sb.root,
        &["new", "existing", "repos/api", "--branch", "topic"],
    );
    let existing = sb.forest("existing").join("api");
    assert_eq!(sb.git(&existing, &["branch", "--show-current"]), "topic");

    sb.ok(&sb.root, &["new", "based", "repos/api", "-B", "topic"]);
    let based = sb.forest("based").join("api");
    assert_eq!(sb.git(&based, &["branch", "--show-current"]), "based");
    assert_eq!(sb.git(&based, &["rev-parse", "HEAD"]), topic);
    assert!(sb.ok(&sb.root, &["ls", "based"]).contains("(off topic)"));
}

#[test]
fn the_base_falls_back_to_a_local_main_branch() {
    let sb = Sandbox::new();
    sb.init(&sb.repos().join("solo"));

    sb.ok(&sb.root, &["new", "local", "repos/solo"]);

    assert!(sb.ok(&sb.root, &["ls", "local"]).contains("(off main)"));
}

#[test]
fn new_plants_nothing_when_a_repo_argument_is_wrong() {
    let sb = Sandbox::new();
    let api = sb.repo("api");

    let err = sb.fails(&sb.root, &["new", "typo", "repos/api", "web"]);

    assert!(err.contains("web is a name, not a path"), "{err}");
    assert!(!sb.forest("typo").exists());
    assert_eq!(sb.git(&api, &["branch", "--list", "typo"]), "");
}

#[test]
fn repo_arguments_are_paths_never_names() {
    let sb = Sandbox::new();
    let api = sb.repo("api");
    sb.init(&sb.root.join("elsewhere"));
    fs::create_dir(sb.root.join("plain")).unwrap();

    sb.ok(&sb.root, &["new", "paths", "./elsewhere"]);
    sb.ok(&api, &["plant", "-f", "paths", "."]);

    assert!(sb.forest("paths").join("elsewhere").is_dir());
    assert!(sb.forest("paths").join("api").is_dir());
    let name = sb.fails(&sb.repos(), &["plant", "-f", "paths", "api"]);
    assert!(name.contains("api is a name, not a path"), "{name}");
    let missing = sb.fails(&sb.root, &["plant", "-f", "paths", "./nope"]);
    assert!(missing.contains("no such repo"), "{missing}");
    let plain = sb.fails(&sb.root, &["plant", "-f", "paths", "./plain"]);
    assert!(plain.contains("not a git repo"), "{plain}");
}

#[test]
fn ls_lists_forests_then_the_trees_in_one() {
    let sb = Sandbox::new();
    assert!(sb.ok(&sb.root, &["ls"]).starts_with("no forests yet"));
    sb.repo("api");
    sb.ok(&sb.root, &["new", "one", "repos/api"]);
    sb.ok(&sb.root, &["new", "empty"]);

    assert_eq!(
        sb.ok(&sb.root, &["list"]),
        format!("{:<28} 0 tree(s)\n{:<28} 1 tree(s)\n", "empty", "one")
    );
    let trees = sb.ok(&sb.root, &["ls", "one"]);
    assert!(
        trees.contains(&format!("  {:<24} {:<24} (off origin/main)", "api", "one")),
        "{trees}"
    );
}

#[test]
fn status_reports_dirty_trees_commits_ahead_and_missing_trees() {
    let sb = Sandbox::new();
    sb.repo("api");
    sb.repo("web");
    sb.ok(&sb.root, &["new", "st", "repos/api", "repos/web"]);
    let forest = sb.forest("st");
    assert!(
        sb.ok(&forest, &["status"])
            .contains("clean  +0/-0 vs origin/main\n")
    );

    sb.commit(&forest.join("api"), "work.txt");
    fs::write(forest.join("api").join("scratch"), "x").unwrap();
    fs::remove_dir_all(forest.join("web")).unwrap();

    let status = sb.ok(&sb.root, &["st", "st"]);
    assert!(
        status.contains("dirty  +1/-0 vs origin/main, 1 unpushed\n"),
        "{status}"
    );
    assert!(
        status.contains(&format!("  {:<24} MISSING", "web")),
        "{status}"
    );
}

#[test]
fn status_says_whether_commits_ahead_of_the_base_are_pushed() {
    let sb = Sandbox::new();
    sb.repo("api");
    sb.ok(&sb.root, &["new", "pr", "repos/api"]);
    let tree = sb.forest("pr").join("api");
    let status = || sb.ok(&sb.root, &["status", "pr"]);

    // A planted branch tracks its base, which lacks the branch's commits.
    sb.commit(&tree, "one.txt");
    let tracking_base = status();
    assert!(
        tracking_base.contains("clean  +1/-0 vs origin/main, 1 unpushed\n"),
        "{tracking_base}"
    );

    sb.git(&tree, &["push", "--quiet", "-u", "origin", "HEAD"]);
    let pushed = status();
    assert!(
        pushed.contains("clean  +1/-0 vs origin/main, pushed\n"),
        "{pushed}"
    );

    sb.commit(&tree, "two.txt");
    let one_more = status();
    assert!(
        one_more.contains("clean  +2/-0 vs origin/main, 1 unpushed\n"),
        "{one_more}"
    );
}

#[test]
fn path_finds_the_forest_from_nested_and_symlinked_directories() {
    let sb = Sandbox::new();
    sb.repo("api");
    let real = sb.root.join("real-forests");
    fs::create_dir(&real).unwrap();
    fs::remove_dir(sb.forests()).unwrap();
    symlink(&real, sb.forests()).unwrap();
    sb.ok(&sb.root, &["new", "deep", "repos/api"]);
    let expected = format!("{}\n", sb.forest("deep").display());

    assert_eq!(sb.ok(&sb.root, &["path", "deep"]), expected);
    let nested = real.join("deep").join("api").join("src");
    fs::create_dir(&nested).unwrap();
    assert_eq!(sb.ok(&nested, &["dir"]), expected);
    let outside = sb.fails(&sb.root, &["path"]);
    assert!(
        outside.contains("not inside a forest; pass the forest's name"),
        "{outside}"
    );
}

#[test]
fn cut_refuses_to_lose_work_unless_forced() {
    let sb = Sandbox::new();
    let api = sb.repo("api");
    sb.ok(&sb.root, &["new", "pr", "repos/api"]);
    let forest = sb.forest("pr");
    let tree = forest.join("api");

    fs::write(tree.join("scratch"), "x").unwrap();
    assert!(
        sb.fails(&forest, &["cut", "api"])
            .contains("api has uncommitted changes")
    );
    fs::remove_file(tree.join("scratch")).unwrap();
    sb.commit(&tree, "work.txt");
    let ahead = sb.fails(&forest, &["cut", "api"]);
    assert!(
        ahead.contains("api has 1 unpushed commit(s), not landed on origin/main"),
        "{ahead}"
    );

    sb.ok(&forest, &["remove", "api", "--force", "--delete-branches"]);
    assert!(!tree.exists());
    assert_eq!(sb.git(&api, &["branch", "--list", "pr"]), "");
    assert_eq!(fs::read_to_string(forest.join(".workforest")).unwrap(), "");
    let gone = sb.fails(&forest, &["cut", "api"]);
    assert!(gone.contains("api is not a tree in pr"), "{gone}");
}

#[test]
fn commits_ahead_of_a_local_base_are_not_pushed_anywhere() {
    let sb = Sandbox::new();
    sb.repo("api");
    sb.ok(&sb.root, &["new", "loc", "repos/api", "-B", "main"]);
    sb.commit(&sb.forest("loc").join("api"), "work.txt");

    let refusal = sb.fails(&sb.root, &["burn", "loc"]);
    assert!(
        refusal.contains("api: 1 commit(s) not pushed anywhere or landed on main"),
        "{refusal}"
    );
}

#[test]
fn burn_counts_merged_work_as_landed_however_it_was_merged() {
    for merge in [Merge::Regular, Merge::Squash, Merge::Rebase] {
        let sb = Sandbox::new();
        let api = sb.repo("api");
        let tree = sb.pull_request("pr", "api");

        sb.merge_on_remote("api", "pr", merge);
        sb.git(&tree, &["fetch", "--quiet", "--prune"]);

        let out = sb.ok(&sb.root, &["burn", "pr", "--delete-branches"]);
        assert!(out.contains("burned forest pr"), "{merge:?}: {out}");
        assert!(!sb.forest("pr").exists(), "{merge:?}");
        assert_eq!(sb.git(&api, &["branch", "--list", "pr"]), "", "{merge:?}");
    }
}

#[test]
fn status_shows_squashed_and_rebased_work_as_landed() {
    for merge in [Merge::Squash, Merge::Rebase] {
        let sb = Sandbox::new();
        sb.repo("api");
        let tree = sb.pull_request("pr", "api");
        let before = sb.ok(&sb.root, &["status", "pr"]);
        assert!(
            before.contains("clean  +2/-0 vs origin/main, pushed\n"),
            "{before}"
        );

        sb.merge_on_remote("api", "pr", merge);
        sb.git(&tree, &["fetch", "--quiet", "--prune"]);

        // The pruned upstream doesn't matter once the work has landed.
        let after = sb.ok(&sb.root, &["status", "pr"]);
        assert!(after.contains("landed +2/-"), "{merge:?}: {after}");
        assert!(!after.contains("pushed"), "{merge:?}: {after}");
    }
}

#[test]
fn cut_counts_work_as_landed_once_the_base_is_fetched() {
    let sb = Sandbox::new();
    sb.repo("api");
    sb.ok(&sb.root, &["new", "pr", "repos/api"]);
    let forest = sb.forest("pr");
    let tree = forest.join("api");
    sb.commit(&tree, "work.txt");
    // Pushed without -u, so the branch still tracks origin/main.
    sb.git(&tree, &["push", "--quiet", "origin", "HEAD"]);
    sb.merge_on_remote("api", "pr", Merge::Squash);

    let stale = sb.fails(&forest, &["cut", "api"]);
    assert!(
        stale.contains("api has 1 unpushed commit(s), not landed on origin/main"),
        "{stale}"
    );
    sb.git(&tree, &["fetch", "--quiet"]);
    sb.ok(&forest, &["cut", "api"]);
    assert!(!tree.exists());
}

#[test]
fn burn_refuses_work_that_has_not_all_landed() {
    let sb = Sandbox::new();
    sb.repo("api");

    // Only the first of the branch's two commits reached main.
    let partial = sb.pull_request("partial", "api");
    let forge = sb.forge("api");
    sb.commit(&forge, "partial-meanwhile.txt");
    sb.git(&forge, &["cherry-pick", "origin/partial~1"]);
    sb.push_main_deleting(&forge, "partial");
    sb.git(&partial, &["fetch", "--quiet", "--prune"]);

    // Work committed after the merge.
    let more = sb.pull_request("more", "api");
    sb.merge_on_remote("api", "more", Merge::Squash);
    sb.git(&more, &["fetch", "--quiet", "--prune"]);
    sb.commit(&more, "more-3.txt");

    // A merge that main has since reverted.
    let reverted = sb.pull_request("reverted", "api");
    sb.merge_on_remote("api", "reverted", Merge::Squash);
    let forge = sb.forge("api");
    sb.git(&forge, &["revert", "--no-edit", "HEAD"]);
    sb.git(&forge, &["push", "--quiet", "origin", "main"]);
    sb.git(&reverted, &["fetch", "--quiet", "--prune"]);

    for (forest, ahead) in [("partial", 2), ("more", 3), ("reverted", 2)] {
        let refusal = sb.fails(&sb.root, &["burn", forest]);
        let risk = format!("  api: {ahead} commit(s) not pushed anywhere or landed on origin/main");
        assert!(refusal.contains(&risk), "{forest}: {refusal}");
        assert!(sb.forest(forest).join("api").is_dir(), "{forest}");
        let status = sb.ok(&sb.root, &["status", forest]);
        assert!(status.contains(&format!("clean  +{ahead}/-")), "{status}");
        assert!(
            status.contains(&format!(", {ahead} unpushed\n")),
            "{status}"
        );
    }
}

#[test]
fn landed_work_survives_later_edits_elsewhere_in_the_same_file() {
    let sb = Sandbox::new();
    let api = sb.repo("api");
    let lines: String = (1..=30).map(|n| format!("{n}\n")).collect();
    fs::write(api.join("lines"), lines).unwrap();
    sb.git(&api, &["add", "lines"]);
    sb.git(&api, &["commit", "--quiet", "--message", "lines"]);
    sb.git(&api, &["push", "--quiet", "origin", "main"]);
    let edit = |dir: &Path, from: &str, to: &str| {
        let text = fs::read_to_string(dir.join("lines")).unwrap();
        let text = text.replace(&format!("\n{from}\n"), &format!("\n{to}\n"));
        fs::write(dir.join("lines"), text).unwrap();
        sb.git(dir, &["commit", "--quiet", "--all", "--message", to]);
    };

    sb.ok(&sb.root, &["new", "pr", "repos/api"]);
    let tree = sb.forest("pr").join("api");
    edit(&tree, "2", "two");
    // Not UTF-8, so the diff must be handled as bytes.
    fs::write(tree.join("latin1"), b"caf\xe9\n").unwrap();
    fs::write(tree.join("binary"), [0u8, 159, 146, 150]).unwrap();
    sb.git(&tree, &["add", "latin1", "binary"]);
    sb.git(&tree, &["commit", "--quiet", "--message", "bytes"]);
    sb.git(&tree, &["push", "--quiet", "-u", "origin", "HEAD"]);
    sb.merge_on_remote("api", "pr", Merge::Squash);
    let forge = sb.forge("api");
    edit(&forge, "28", "twenty-eight");
    sb.git(&forge, &["push", "--quiet", "origin", "main"]);
    sb.git(&tree, &["fetch", "--quiet", "--prune"]);

    let status = sb.ok(&sb.root, &["status", "pr"]);
    assert!(status.contains("landed +2/-"), "{status}");
    sb.ok(&sb.root, &["burn", "pr"]);
}

#[test]
fn cut_cleans_up_a_tree_deleted_by_hand() {
    let sb = Sandbox::new();
    let api = sb.repo("api");
    sb.ok(&sb.root, &["new", "gone", "repos/api"]);
    let tree = sb.forest("gone").join("api");
    fs::remove_dir_all(&tree).unwrap();

    sb.ok(&sb.root, &["cut", "-f", "gone", "api"]);

    assert!(!sb.git(&api, &["worktree", "list"]).contains(path(&tree)));
}

#[test]
fn burn_removes_a_forest_only_when_no_work_would_be_lost() {
    let sb = Sandbox::new();
    let api = sb.repo("api");
    sb.repo("web");
    sb.ok(&sb.root, &["new", "blaze", "repos/api", "repos/web"]);
    let forest = sb.forest("blaze");

    sb.commit(&forest.join("api"), "work.txt");
    fs::write(forest.join("web").join("scratch"), "x").unwrap();
    let refusal = sb.fails(&sb.root, &["burn", "blaze"]);
    assert!(refusal.contains("refusing to burn blaze"), "{refusal}");
    assert!(refusal.contains("  api: 1 unpushed commit(s)"), "{refusal}");
    assert!(refusal.contains("  web: uncommitted changes"), "{refusal}");
    assert!(forest.join("api").is_dir(), "a refusal removes nothing");

    sb.git(
        &forest.join("api"),
        &["push", "--quiet", "-u", "origin", "HEAD"],
    );
    fs::remove_file(forest.join("web").join("scratch")).unwrap();
    assert!(
        sb.ok(&sb.root, &["rm", "blaze"])
            .contains("burned forest blaze")
    );
    assert!(!forest.exists());
    assert!(
        sb.git(&api, &["branch", "--list", "blaze"])
            .contains("blaze")
    );
}

#[test]
fn burn_without_a_name_burns_the_forest_you_are_in() {
    let sb = Sandbox::new();
    let api = sb.repo("api");
    sb.ok(&sb.root, &["new", "here", "repos/api"]);
    let nested = sb.forest("here").join("api").join("src");
    fs::create_dir(&nested).unwrap();

    let output = sb.workforest(&nested, &["burn"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(!sb.forest("here").exists());
    assert!(
        stderr.contains(&format!("; cd {}", api.display())),
        "{stderr}"
    );
    let outside = sb.fails(&sb.root, &["burn"]);
    assert!(outside.contains("not inside a forest"), "{outside}");
}

#[test]
fn cutting_the_tree_you_are_in_says_where_to_go() {
    let sb = Sandbox::new();
    let api = sb.repo("api");
    sb.ok(&sb.root, &["new", "leave", "repos/api"]);
    let tree = sb.forest("leave").join("api");

    let output = sb.workforest(&tree, &["cut", "api"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(!tree.exists());
    assert!(
        stderr.contains(&format!("; cd {}", api.display())),
        "{stderr}"
    );
}

#[test]
fn burn_force_discards_work_and_can_delete_branches() {
    let sb = Sandbox::new();
    let api = sb.repo("api");
    sb.ok(&sb.root, &["new", "doomed", "repos/api"]);
    let tree = sb.forest("doomed").join("api");
    sb.commit(&tree, "work.txt");
    fs::write(tree.join("scratch"), "x").unwrap();

    sb.ok(
        &sb.root,
        &["delete", "doomed", "--force", "--delete-branches"],
    );

    assert!(!sb.forest("doomed").exists());
    assert_eq!(sb.git(&api, &["branch", "--list", "doomed"]), "");
}

#[test]
fn burn_refuses_to_delete_a_checkout_the_manifest_does_not_record() {
    let sb = Sandbox::new();
    let api = sb.repo("api");
    sb.ok(&sb.root, &["new", "stray"]);
    let checkout = sb.forest("stray").join("api");
    sb.git(
        &api,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "by-hand",
            path(&checkout),
        ],
    );

    let refusal = sb.fails(&sb.root, &["burn", "stray"]);
    assert!(
        refusal.contains("  api: a checkout the manifest doesn't record"),
        "{refusal}"
    );
    assert!(checkout.is_dir(), "a refusal removes nothing");

    sb.ok(&sb.root, &["burn", "stray", "--force"]);
    assert!(!sb.forest("stray").exists());
}

#[test]
fn concurrent_plants_into_one_forest_are_all_recorded() {
    let sb = Sandbox::new();
    let repos: Vec<String> = (0..24).map(|i| format!("repo{i}")).collect();
    for repo in &repos {
        sb.repo(repo);
    }
    sb.ok(&sb.root, &["new", "busy"]);

    let plants: Vec<_> = repos
        .iter()
        .map(|repo| {
            let repo_path = format!("repos/{repo}");
            sb.isolate(&mut Command::new(env!("CARGO_BIN_EXE_workforest")))
                .current_dir(&sb.root)
                .args(["plant", "-f", "busy", &repo_path])
                .stdout(Stdio::null())
                .spawn()
                .expect("start planting")
        })
        .collect();
    for mut plant in plants {
        assert!(plant.wait().expect("wait for planting").success());
    }

    let trees = sb.ok(&sb.root, &["ls", "busy"]);
    for repo in &repos {
        assert!(
            trees.contains(&format!("  {repo} ")),
            "{repo} is missing:\n{trees}"
        );
    }
}

#[test]
fn version_and_bare_invocation() {
    let sb = Sandbox::new();
    let version = sb.ok(&sb.root, &["--version"]);
    assert!(
        version.starts_with(&format!("workforest {}", env!("CARGO_PKG_VERSION"))),
        "{version}"
    );
    assert!(sb.ok(&sb.root, &[]).contains("Usage:"));
}
