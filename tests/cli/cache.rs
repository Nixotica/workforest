//! Build caches grafted into trees: what gets hardlinked, copied, shared or
//! left cold, and what workforest refuses to touch.

use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

use tempfile::TempDir;

use super::Sandbox;

mod cargo;

/// The default size from which cache files are hardlinked.
const LINK_MIN: usize = 64 * 1024;

impl Sandbox {
    /// Commit `file` holding `contents` to `repo` and push it, so that trees
    /// planted off `origin/main` have it.
    fn publish(&self, repo: &Path, file: &str, contents: &str) {
        fs::write(repo.join(file), contents).expect("write a file");
        self.git(repo, &["add", file]);
        self.git(repo, &["commit", "--quiet", "--message", file]);
        self.git(repo, &["push", "--quiet", "origin", "main"]);
    }

    /// A repo that declares its caches with `declaration`, and whose git
    /// ignores `build`.
    fn cached_repo(&self, name: &str, declaration: &str) -> PathBuf {
        let repo = self.repo(name);
        self.publish(&repo, ".gitignore", "/build\n");
        self.publish(&repo, ".workforest-cache", declaration);
        repo
    }

    /// Run workforest with extra environment variables.
    fn workforest_with(&self, cwd: &Path, args: &[&str], env: &[(&str, &OsStr)]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_workforest"));
        self.isolate(&mut command).current_dir(cwd).args(args);
        for (name, value) in env {
            command.env(name, value);
        }
        command.output().expect("run workforest")
    }
}

/// Write a file of `size` bytes, creating its directory.
fn write(file: &Path, size: usize) {
    fs::create_dir_all(file.parent().unwrap()).expect("create a directory");
    fs::write(file, vec![b'x'; size]).expect("write a file");
}

/// The time `secs` seconds after the epoch.
fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

/// Set the modification time of `file` to `secs` seconds after the epoch.
fn set_modified(file: &Path, secs: u64) {
    fs::File::options()
        .write(true)
        .open(file)
        .and_then(|file| file.set_modified(at(secs)))
        .unwrap_or_else(|err| panic!("{}: {err}", file.display()));
}

fn meta(file: &Path) -> fs::Metadata {
    fs::symlink_metadata(file).unwrap_or_else(|err| panic!("{}: {err}", file.display()))
}

/// Whether `a` and `b` are the same file: hardlinks of one inode.
fn same_file(a: &Path, b: &Path) -> bool {
    let (a, b) = (meta(a), meta(b));
    (a.dev(), a.ino()) == (b.dev(), b.ino())
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn planting_hardlinks_large_cache_files_and_copies_small_ones() {
    let sb = Sandbox::new();
    let repo = sb.cached_repo("api", "build clone\n");
    let build = repo.join("build");
    write(&build.join("small"), LINK_MIN - 1);
    write(&build.join("big"), LINK_MIN);
    write(&build.join("deep/er/big"), LINK_MIN + 1);
    let past = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
    fs::File::options()
        .write(true)
        .open(build.join("small"))
        .unwrap()
        .set_modified(past)
        .unwrap();

    let out = sb.ok(&sb.root, &["new", "warm", "repos/api"]);

    assert!(
        out.contains("  cache build: grafted from the main checkout: 3 files, "),
        "{out}"
    );
    let grafted = sb.forest("warm").join("api/build");
    assert!(same_file(&build.join("big"), &grafted.join("big")));
    assert!(same_file(
        &build.join("deep/er/big"),
        &grafted.join("deep/er/big")
    ));
    assert!(!same_file(&build.join("small"), &grafted.join("small")));
    assert_eq!(
        fs::read(grafted.join("small")).unwrap(),
        fs::read(build.join("small")).unwrap()
    );
    assert_eq!(
        meta(&grafted.join("small")).modified().unwrap(),
        past,
        "a copy keeps its modification time"
    );
    // git ignores the cache, so the tree is clean and burns without --force.
    assert!(sb.ok(&sb.root, &["status", "warm"]).contains(" clean "));
    sb.ok(&sb.root, &["burn", "warm"]);
    assert_eq!(meta(&build.join("big")).nlink(), 1);
    assert_eq!(fs::read(build.join("small")).unwrap().len(), LINK_MIN - 1);
}

#[test]
fn a_graft_of_many_files_in_nested_directories_misses_none() {
    let sb = Sandbox::new();
    let repo = sb.cached_repo("api", "build clone\n");
    let build = repo.join("build");
    // Fifty directories, nested ten deep in five chains, forty files each:
    // a graft spread over threads must create each directory first.
    let mut files = Vec::new();
    for chain in 0..5 {
        let mut dir = build.clone();
        for depth in 0..10 {
            dir = dir.join(format!("c{chain}d{depth}"));
            for n in 0..40 {
                let file = dir.join(format!("f{n}"));
                let size = if n % 4 == 0 {
                    LINK_MIN + n
                } else {
                    n * 100 + 1
                };
                write(&file, size);
                files.push((file, size));
            }
        }
    }

    let out = sb.ok(&sb.root, &["new", "many", "repos/api"]);

    assert!(out.contains(": 2000 files, "), "{out}");
    let grafted = sb.forest("many").join("api/build");
    for (file, size) in &files {
        let copy = grafted.join(file.strip_prefix(&build).unwrap());
        assert_eq!(meta(&copy).len(), *size as u64, "{}", copy.display());
        assert_eq!(
            same_file(file, &copy),
            *size >= LINK_MIN,
            "{}",
            copy.display()
        );
    }
}

#[test]
fn always_copy_globs_copy_matching_files_whatever_their_size() {
    let sb = Sandbox::new();
    let repo = sb.cached_repo("api", "build clone *.lock,state/*\n");
    let build = repo.join("build");
    for file in ["a.lock", "x/state/s", "x/statefile", "other"] {
        write(&build.join(file), LINK_MIN);
    }

    sb.ok(&sb.root, &["new", "globs", "repos/api"]);

    let grafted = sb.forest("globs").join("api/build");
    for (file, linked) in [
        ("a.lock", false),
        ("x/state/s", false),
        ("x/statefile", true),
        ("other", true),
    ] {
        assert_eq!(
            same_file(&build.join(file), &grafted.join(file)),
            linked,
            "{file} should be {}",
            if linked { "hardlinked" } else { "copied" }
        );
    }
}

#[test]
fn symlinks_in_a_cache_are_copied_as_symlinks() {
    let sb = Sandbox::new();
    let repo = sb.cached_repo("api", "build clone\n");
    let build = repo.join("build");
    write(&build.join("big"), LINK_MIN);
    symlink("big", build.join("relative")).unwrap();
    symlink("/nowhere/at/all", build.join("dangling")).unwrap();
    let inward = build.join("big");
    symlink(&inward, build.join("inward")).unwrap();

    let out = sb.ok(&sb.root, &["new", "links", "repos/api"]);

    let grafted = sb.forest("links").join("api/build");
    for (link, target) in [
        ("relative", Path::new("big")),
        ("dangling", Path::new("/nowhere/at/all")),
        ("inward", &inward),
    ] {
        assert!(meta(&grafted.join(link)).is_symlink(), "{link}");
        assert_eq!(fs::read_link(grafted.join(link)).unwrap(), target);
    }
    assert!(
        out.contains(
            "; 1 symlink into the main checkout, so a build that writes through one changes it"
        ),
        "{out}"
    );
}

#[test]
fn the_size_from_which_files_are_hardlinked_is_configurable() {
    let sb = Sandbox::new();
    let repo = sb.cached_repo("api", "build clone\n");
    write(&repo.join("build/medium"), 100);

    let low = OsStr::new("100");
    let out = sb.workforest_with(
        &sb.root,
        &["new", "low", "repos/api"],
        &[("WORKFOREST_CACHE_LINK_MIN", low)],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let grafted = sb.forest("low").join("api/build/medium");
    assert!(same_file(&repo.join("build/medium"), &grafted));

    // A bad value stops only the commands that graft, before they change
    // anything.
    let bad = [("WORKFOREST_CACHE_LINK_MIN", OsStr::new("lots"))];
    let out = sb.workforest_with(&sb.root, &["new", "bad", "repos/api"], &bad);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("must be a number of bytes, not 'lots'"));
    assert!(!sb.forest("bad").exists());
    for args in [
        &["ls"][..],
        &["new", "cold", "repos/api", "--no-cache"],
        &["cache", "status", "cold"],
        &["burn", "cold"],
    ] {
        let out = sb.workforest_with(&sb.root, args, &bad);
        assert!(out.status.success(), "{args:?}: {}", stderr(&out));
    }
}

/// A directory on a different filesystem from `root`, if there is one.
fn other_filesystem(root: &Path) -> Option<TempDir> {
    let dir = tempfile::tempdir_in("/dev/shm").ok()?;
    (meta(dir.path()).dev() != meta(root).dev()).then_some(dir)
}

#[test]
fn a_cache_on_another_filesystem_is_left_cold() {
    let sb = Sandbox::new();
    let Some(elsewhere) = other_filesystem(&sb.root) else {
        eprintln!("skipped: /dev/shm is missing or on the same filesystem");
        return;
    };
    let repo = sb.cached_repo("api", "build clone\n");
    write(&repo.join("build/big"), LINK_MIN);

    let out = sb.workforest_with(
        &sb.root,
        &["new", "far", "repos/api"],
        &[("WORKFOREST_ROOT", elsewhere.path().as_os_str())],
    );

    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("is on another filesystem, so it can't be hardlinked"),
        "{}",
        stdout(&out)
    );
    let tree = elsewhere.path().join("far/api");
    assert!(tree.join(".workforest-cache").is_file());
    assert!(!tree.join("build").exists());
}

#[test]
fn unsafe_cache_paths_are_ignored_with_a_warning() {
    let sb = Sandbox::new();
    let declaration = "\
/abs clone
../up clone
a/../b clone
.git/hooks clone
. clone
build   clone   -   extra
build sideways
build clone # the one good line
";
    let repo = sb.cached_repo("api", declaration);
    write(&repo.join("build/big"), LINK_MIN);
    fs::create_dir(sb.root.join("up")).unwrap();

    let out = sb.workforest(&sb.root, &["new", "unsafe", "repos/api"]);

    assert!(out.status.success(), "{}", stderr(&out));
    let warnings = stderr(&out);
    for warning in [
        "api: .workforest-cache:1: ignoring cache path /abs: it is absolute",
        "api: .workforest-cache:2: ignoring cache path ../up: it leaves the tree",
        "api: .workforest-cache:3: ignoring cache path a/../b: it leaves the tree",
        "api: .workforest-cache:4: ignoring cache path .git/hooks: it is git's own",
        "api: .workforest-cache:5: ignoring cache path .: it is the whole tree",
        "api: .workforest-cache:6: ignoring a line with more than three fields",
        "api: .workforest-cache:7: ignoring unknown cache mode sideways for build",
    ] {
        assert!(
            warnings.contains(warning),
            "missing {warning:?} in:\n{warnings}"
        );
    }
    assert!(stdout(&out).contains("cache build: grafted"));
    assert!(fs::read_dir(sb.root.join("up")).unwrap().next().is_none());
}

#[test]
fn paths_git_tracks_or_does_not_ignore_are_never_replaced() {
    let sb = Sandbox::new();
    let repo = sb.cached_repo("api", "src clone\nout clone\n");
    sb.publish(&repo, "src", "tracked\n");
    fs::remove_file(repo.join("src")).unwrap();
    write(&repo.join("src/big"), LINK_MIN);
    write(&repo.join("out/big"), LINK_MIN);

    let out = sb.workforest(&sb.root, &["new", "careful", "repos/api"]);

    assert!(out.status.success(), "{}", stderr(&out));
    let warnings = stderr(&out);
    assert!(warnings.contains("git tracks files under src, so it isn't a cache"));
    assert!(warnings.contains("git doesn't ignore out, so it isn't a cache"));
    let tree = sb.forest("careful").join("api");
    assert_eq!(fs::read_to_string(tree.join("src")).unwrap(), "tracked\n");
    assert!(!tree.join("out").exists());

    fs::create_dir(tree.join("out")).unwrap();
    fs::write(tree.join("out/notes"), "mine").unwrap();
    sb.workforest(&tree, &["cache", "graft", "--force"]);
    sb.workforest(&tree, &["cache", "drop"]);
    assert_eq!(fs::read_to_string(tree.join("src")).unwrap(), "tracked\n");
    assert_eq!(fs::read_to_string(tree.join("out/notes")).unwrap(), "mine");
}

#[test]
fn the_machine_local_declaration_overrides_the_repos_per_path() {
    let sb = Sandbox::new();
    let repo = sb.cached_repo("api", "build clone *.lock\ncache clone\n");
    write(&repo.join("build/big"), LINK_MIN);
    let local = repo.join(".git/workforest-cache");

    fs::write(&local, "build never\n").unwrap();
    let paths = sb.ok(&sb.root, &["cache", "paths", "repos/api"]);
    assert!(paths.contains("build"), "{paths}");
    let line = |name: &str| {
        paths
            .lines()
            .find(|line| line.trim_start().starts_with(name))
            .unwrap_or_else(|| panic!("no {name} in:\n{paths}"))
            .split_whitespace()
            .collect::<Vec<_>>()
    };
    assert_eq!(
        line("build"),
        ["build", "never", "*.lock", ".git/workforest-cache:1"]
    );
    assert_eq!(
        line("cache"),
        ["cache", "clone", "-", ".workforest-cache:2"]
    );
    sb.ok(&sb.root, &["new", "local", "repos/api"]);
    assert!(!sb.forest("local").join("api/build").exists());

    // An override that lists no globs keeps those of the entry it overrides,
    // and `-` drops them.
    fs::write(&local, "build clone\n").unwrap();
    let paths = sb.ok(&sb.root, &["cache", "paths", "repos/api"]);
    assert!(
        paths.contains("build                  clone  *.lock"),
        "{paths}"
    );
    fs::write(&local, "build clone -\n").unwrap();
    let paths = sb.ok(&sb.root, &["cache", "paths", "repos/api"]);
    assert!(paths.contains("build                  clone  -"), "{paths}");
}

#[test]
fn no_cache_and_never_leave_caches_cold() {
    let sb = Sandbox::new();
    let repo = sb.cached_repo("api", "build clone\n");
    write(&repo.join("build/big"), LINK_MIN);

    let out = sb.ok(&sb.root, &["new", "cold", "repos/api", "--no-cache"]);
    assert!(!out.contains("cache"), "{out}");
    assert!(!sb.forest("cold").join("api/build").exists());
    let status = sb.ok(&sb.root, &["cache", "status", "cold"]);
    assert!(
        status.contains("    build                  cold"),
        "{status}"
    );

    sb.publish(&repo, ".workforest-cache", "build never\n");
    sb.ok(&sb.root, &["new", "never", "repos/api"]);
    assert!(!sb.forest("never").join("api/build").exists());
    let status = sb.ok(&sb.root, &["cache", "status", "never"]);
    assert!(
        status.contains("    build                  never"),
        "{status}"
    );
}

#[test]
fn cache_graft_fills_planted_trees_and_force_regrafts() {
    let sb = Sandbox::new();
    let repo = sb.cached_repo("api", "build clone\n");
    write(&repo.join("build/big"), LINK_MIN);
    sb.ok(&sb.root, &["new", "later", "repos/api", "--no-cache"]);
    let tree = sb.forest("later").join("api");

    let out = sb.ok(&tree, &["cache", "graft"]);
    assert!(out.contains("cache build: grafted"), "{out}");
    assert!(same_file(&repo.join("build/big"), &tree.join("build/big")));

    // A rebuild in the main checkout replaces the file with a new one.
    fs::remove_file(repo.join("build/big")).unwrap();
    write(&repo.join("build/big"), LINK_MIN + 1);
    let out = sb.ok(&tree, &["cache", "graft", "api"]);
    assert!(
        out.contains("cache build: already there, left alone"),
        "{out}"
    );
    assert!(!same_file(&repo.join("build/big"), &tree.join("build/big")));

    sb.ok(&sb.root, &["cache", "graft", "-f", "later", "--force"]);
    assert!(same_file(&repo.join("build/big"), &tree.join("build/big")));

    let err = sb.fails(&tree, &["cache", "graft", "web"]);
    assert!(err.contains("web is not a tree in later"), "{err}");
}

#[test]
fn force_keeps_the_trees_cache_when_there_is_nothing_to_replace_it_with() {
    let sb = Sandbox::new();
    let repo = sb.cached_repo("api", "build clone\n");
    write(&repo.join("build/big"), LINK_MIN);
    sb.ok(&sb.root, &["new", "keep", "repos/api"]);
    let tree = sb.forest("keep").join("api");
    fs::write(tree.join("build/mine"), "built in the tree").unwrap();
    fs::rename(repo.join("build"), repo.join("elsewhere")).unwrap();

    let out = sb.ok(&tree, &["cache", "graft", "--force"]);

    assert!(
        out.contains("cache build: the main checkout has none; the tree keeps its own"),
        "{out}"
    );
    assert_eq!(
        fs::read_to_string(tree.join("build/mine")).unwrap(),
        "built in the tree"
    );
}

#[test]
fn a_later_graft_marks_the_trees_older_files_as_changed() {
    let sb = Sandbox::new();
    let repo = sb.cached_repo("api", "build clone\n");
    sb.publish(&repo, ".gitignore", "/build\n*.log\n");
    sb.ok(&sb.root, &["new", "later", "repos/api", "--no-cache"]);
    let tree = sb.forest("later").join("api");
    // The tree's files were checked out or edited before the main checkout's
    // last build, all but one.
    for file in [".workforest-cache", "notes.txt", "debug.log"] {
        fs::write(tree.join(file), "x").unwrap();
        set_modified(&tree.join(file), 1_000_000_000);
    }
    fs::write(tree.join("later.txt"), "x").unwrap();
    set_modified(&tree.join("later.txt"), 1_500_000_000);
    write(&repo.join("build/big"), LINK_MIN);
    write(&repo.join("build/small"), 10);
    set_modified(&repo.join("build/big"), 1_200_000_000);
    set_modified(&repo.join("build/small"), 1_100_000_000);

    let out = sb.ok(&tree, &["cache", "graft"]);

    assert!(out.contains("older than it as changed"), "{out}");
    let modified = |file: &str| meta(&tree.join(file)).modified().unwrap();
    // Tracked, and untracked but not ignored: newer than the graft now, so
    // that a build tool doesn't take the grafted output for built from them.
    assert!(modified(".workforest-cache") > at(1_200_000_000));
    assert!(modified("notes.txt") > at(1_200_000_000));
    // Already newer, or ignored: left alone.
    assert_eq!(modified("later.txt"), at(1_500_000_000));
    assert_eq!(modified("debug.log"), at(1_000_000_000));
    assert_eq!(modified("build/small"), at(1_100_000_000));

    // Planting is newer than anything grafted, so nothing needs marking.
    let out = sb.ok(&sb.root, &["new", "fresh", "repos/api"]);
    assert!(out.contains("cache build: grafted"), "{out}");
    assert!(!out.contains("as changed"), "{out}");
}

#[test]
fn a_graft_is_staged_beside_the_cache_and_moved_into_place() {
    let sb = Sandbox::new();
    let repo = sb.cached_repo("api", "build clone\nout/deep clone\n");
    sb.publish(&repo, ".gitignore", "/build\n/out\n");
    write(&repo.join("build/big"), LINK_MIN);
    write(&repo.join("out/deep/big"), LINK_MIN);
    sb.ok(&sb.root, &["new", "staged", "repos/api", "--no-cache"]);
    let tree = sb.forest("staged").join("api");
    // What a graft interrupted halfway leaves behind.
    write(&tree.join(".build.workforest-graft/half"), 10);
    write(&tree.join("out/.deep.workforest-graft/half"), 10);

    let out = sb.ok(&tree, &["cache", "graft"]);

    assert!(out.contains("cache build: grafted"), "{out}");
    assert!(out.contains("cache out/deep: grafted"), "{out}");
    assert!(same_file(&repo.join("build/big"), &tree.join("build/big")));
    assert!(same_file(
        &repo.join("out/deep/big"),
        &tree.join("out/deep/big")
    ));
    assert!(!tree.join(".build.workforest-graft").exists());
    assert!(!tree.join("out/.deep.workforest-graft").exists());
    assert!(sb.ok(&sb.root, &["status", "staged"]).contains(" clean "));
}

#[test]
fn cache_status_reports_hardlinked_and_own_bytes_and_drop_deletes_clones() {
    let sb = Sandbox::new();
    let repo = sb.cached_repo("api", "build clone\n");
    write(&repo.join("build/big"), LINK_MIN);
    write(&repo.join("build/small"), 100);
    sb.ok(&sb.root, &["new", "usage", "repos/api"]);

    let status = sb.ok(&sb.root, &["cache", "status", "usage"]);
    assert!(
        status.contains("    build                  2 files, 64.0 KiB hardlinked, 100 B own"),
        "{status}"
    );

    let out = sb.ok(&sb.root, &["cache", "drop", "-f", "usage"]);
    assert_eq!(out, "dropped api/build\n");
    assert!(!sb.forest("usage").join("api/build").exists());
    assert!(repo.join("build/big").exists());
    assert_eq!(sb.ok(&sb.root, &["cache", "drop", "-f", "usage"]), "");
}

#[test]
fn doctor_passes_a_build_that_replaces_cache_files() {
    let sb = Sandbox::new();
    let repo = sb.cached_repo("api", "build clone\n");
    write(&repo.join("build/big"), LINK_MIN);
    let rebuild = "cp build/big build/new && echo more >> build/new && mv build/new build/big";

    let out = sb.ok(
        &sb.root,
        &["cache", "doctor", "repos/api", "--cmd", rebuild],
    );

    assert!(
        out.contains(
            "main checkout        unchanged: the build wrote nothing through a hardlink\n"
        ),
        "{out}"
    );
    assert!(
        out.contains("main checkout's path no grafted file names the main checkout\n"),
        "{out}"
    );
    assert_eq!(fs::read(repo.join("build/big")).unwrap().len(), LINK_MIN);
    assert_no_scratch_worktree(&sb, &repo);
}

#[test]
fn doctor_lists_grafted_files_that_name_the_main_checkout() {
    let sb = Sandbox::new();
    let repo = sb.cached_repo("api", "build clone\n");
    // A configure step that records where the sources are, as CMake's cache
    // or a virtualenv's scripts do.
    write(&repo.join("build/big"), LINK_MIN);
    fs::write(
        repo.join("build/sources"),
        repo.as_os_str().as_encoded_bytes(),
    )
    .unwrap();
    let rebuild = "cp \"$(cat build/sources)/.gitignore\" build/copied";

    let out = sb.ok(
        &sb.root,
        &["cache", "doctor", "repos/api", "--cmd", rebuild],
    );

    assert!(
        out.contains(
            "main checkout's path 1 file naming the main checkout: a build that reads one \
             works on the main checkout's files, not the tree's\n    build/sources\n"
        ),
        "{out}"
    );
    assert_no_scratch_worktree(&sb, &repo);
}

#[test]
fn doctor_catches_a_build_that_writes_through_a_hardlink() {
    let sb = Sandbox::new();
    let repo = sb.cached_repo("api", "build clone\n");
    write(&repo.join("build/big"), LINK_MIN);

    for write_through in ["echo more >> build/big", "chmod 600 build/big"] {
        let out = sb.workforest(
            &sb.root,
            &["cache", "doctor", "repos/api", "--cmd", write_through],
        );

        assert!(!out.status.success(), "{write_through}");
        assert!(
            stdout(&out)
                .contains("1 file CHANGED by the build in the scratch worktree:\n    build/big\n"),
            "{write_through}: {}",
            stdout(&out)
        );
        assert!(stderr(&out).contains("api's cache is not safe to clone as declared"));
        assert_no_scratch_worktree(&sb, &repo);
        fs::remove_file(repo.join("build/big")).unwrap();
        write(&repo.join("build/big"), LINK_MIN);
    }
}

#[test]
fn doctor_needs_a_cache_a_build_command_and_a_build_that_works() {
    let sb = Sandbox::new();
    let repo = sb.cached_repo("api", "build clone\n");

    let err = sb.fails(&sb.root, &["cache", "doctor", "repos/api", "--cmd", "true"]);
    assert!(err.contains("api has no clone cache to test"), "{err}");

    write(&repo.join("build/big"), LINK_MIN);
    let err = sb.fails(&sb.root, &["cache", "doctor", "repos/api"]);
    assert!(err.contains("pass --cmd '<build command>'"), "{err}");

    let err = sb.fails(&sb.root, &["cache", "doctor", "repos/api", "--cmd", "true"]);
    assert!(
        err.contains("the build changed nothing in the grafted cache, so the check proves nothing"),
        "{err}"
    );

    let err = sb.fails(
        &sb.root,
        &["cache", "doctor", "repos/api", "--cmd", "exit 3"],
    );
    assert!(
        err.contains("the build failed, so the check proves nothing"),
        "{err}"
    );
    assert_no_scratch_worktree(&sb, &repo);
}

#[test]
fn doctor_removes_the_scratch_worktree_an_interrupted_doctor_left() {
    let sb = Sandbox::new();
    let repo = sb.cached_repo("api", "build clone\n");
    write(&repo.join("build/big"), LINK_MIN);
    let mut gone = Command::new("true").spawn().unwrap();
    gone.wait().unwrap();
    let leftover = sb.forests().join(format!(".doctor-api-{}", gone.id()));
    sb.git(
        &repo,
        &[
            "worktree",
            "add",
            "--quiet",
            "--detach",
            leftover.to_str().unwrap(),
        ],
    );
    let rebuild = "cp build/big build/new && mv build/new build/big";

    let out = sb.ok(
        &sb.root,
        &["cache", "doctor", "repos/api", "--cmd", rebuild],
    );

    assert!(
        out.contains("removing a scratch worktree an interrupted doctor left"),
        "{out}"
    );
    assert_no_scratch_worktree(&sb, &repo);
}

fn assert_no_scratch_worktree(sb: &Sandbox, repo: &Path) {
    let worktrees = sb.git(repo, &["worktree", "list", "--porcelain"]);
    assert_eq!(worktrees.matches("worktree ").count(), 1, "{worktrees}");
    let leftovers: Vec<_> = fs::read_dir(sb.forests())
        .map(|entries| entries.flatten().map(|e| e.file_name()).collect())
        .unwrap_or_default();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}
