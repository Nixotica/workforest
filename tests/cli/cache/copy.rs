//! What a main checkout ignores, copied into each new tree as the tree's own:
//! everything but the caches, which are grafted instead, and what doesn't work
//! at another path. A copy is never given back, and never replaced.

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use super::super::Sandbox;
use super::{LINK_MIN, at, meta, same_file, set_modified, write};

impl Sandbox {
    /// A repo whose git ignores what `gitignore` says.
    fn ignoring(&self, name: &str, gitignore: &str) -> PathBuf {
        let repo = self.repo(name);
        self.publish(&repo, ".gitignore", gitignore);
        repo
    }

    /// What `git status` says of the tree: nothing, for a clean one.
    fn changes(&self, tree: &Path) -> String {
        self.git(tree, &["status", "--porcelain"])
    }
}

/// Write `contents` to `file`, creating its directory.
fn put(file: &Path, contents: &str) {
    fs::create_dir_all(file.parent().unwrap()).expect("create a directory");
    fs::write(file, contents).expect("write a file");
}

fn read(file: &Path) -> String {
    fs::read_to_string(file).unwrap_or_else(|err| panic!("{}: {err}", file.display()))
}

#[test]
fn planting_copies_what_the_main_checkout_ignores_as_the_trees_own() {
    let sb = Sandbox::new();
    let repo = sb.ignoring("api", ".env\nnode_modules/\n");
    put(&repo.join(".env"), "TOKEN=main\n");
    fs::set_permissions(repo.join(".env"), fs::Permissions::from_mode(0o600)).unwrap();
    put(&repo.join("config/.env"), "NESTED=1\n");
    put(
        &repo.join("node_modules/pkg/index.js"),
        "module.exports = 1;\n",
    );

    let out = sb.ok(&sb.root, &["new", "env", "repos/api"]);
    let tree = sb.forest("env").join("api");
    for line in [
        "  copy .env: 11 B\n",
        "  copy config/.env: 9 B\n",
        "  copy node_modules: 1 file, 20 B\n",
    ] {
        assert!(out.contains(line), "{out}");
    }
    assert_eq!(read(&tree.join(".env")), "TOKEN=main\n");
    assert_eq!(read(&tree.join("config/.env")), "NESTED=1\n");
    assert_eq!(
        read(&tree.join("node_modules/pkg/index.js")),
        "module.exports = 1;\n"
    );
    assert!(!same_file(&repo.join(".env"), &tree.join(".env")));
    assert_eq!(meta(&tree.join(".env")).permissions().mode() & 0o777, 0o600);
    assert_eq!(sb.changes(&tree), "");
}

#[test]
fn a_copy_is_never_given_back() {
    let sb = Sandbox::new();
    let repo = sb.ignoring("api", ".env\n");
    put(&repo.join(".env"), "TOKEN=main\n");
    sb.ok(&sb.root, &["new", "env", "repos/api"]);
    let tree = sb.forest("env").join("api");

    put(&tree.join(".env"), "TOKEN=tree\n");
    assert_eq!(read(&repo.join(".env")), "TOKEN=main\n");
    sb.ok(&sb.root, &["burn", "env"]);
    assert_eq!(read(&repo.join(".env")), "TOKEN=main\n");
}

#[test]
fn what_git_does_not_ignore_stays_behind() {
    let sb = Sandbox::new();
    let repo = sb.ignoring("api", ".env\n");
    put(&repo.join(".env"), "TOKEN=main\n");
    put(&repo.join("draft.rs"), "fn wip() {}\n");

    sb.ok(&sb.root, &["new", "wip", "repos/api"]);
    let tree = sb.forest("wip").join("api");
    assert!(tree.join(".env").exists());
    assert!(!tree.join("draft.rs").exists());
    assert_eq!(sb.changes(&tree), "");
}

#[test]
fn clean_plants_only_what_git_checks_out() {
    let sb = Sandbox::new();
    let repo = sb.cached_repo("api", "build clone\n");
    sb.publish(&repo, ".gitignore", "/build\n.env\n");
    put(&repo.join(".env"), "TOKEN=main\n");
    write(&repo.join("build/big"), LINK_MIN);

    let out = sb.ok(&sb.root, &["new", "bare", "repos/api", "--clean"]);
    assert!(!out.contains("  copy") && !out.contains("  cache"), "{out}");
    let tree = sb.forest("bare").join("api");
    assert!(!tree.join(".env").exists());
    assert!(!tree.join("build").exists());
}

#[test]
fn no_cache_leaves_caches_cold_but_still_copies() {
    let sb = Sandbox::new();
    let repo = sb.cached_repo("api", "build clone\n");
    sb.publish(&repo, ".gitignore", "/build\n.env\n");
    put(&repo.join(".env"), "TOKEN=main\n");
    write(&repo.join("build/big"), LINK_MIN);

    let out = sb.ok(&sb.root, &["new", "cold", "repos/api", "--no-cache"]);
    assert!(out.contains("  copy .env: 11 B\n"), "{out}");
    assert!(
        !out.contains("  cache build") && !out.contains("  copy build"),
        "{out}"
    );
    let tree = sb.forest("cold").join("api");
    assert!(tree.join(".env").exists());
    assert!(!tree.join("build").exists());
}

#[test]
fn caches_are_grafted_rather_than_copied_and_never_leaves_a_path_out() {
    let sb = Sandbox::new();
    let repo = sb.cached_repo("api", "build clone\nlogs never\n");
    sb.publish(&repo, ".gitignore", "/build\n/logs\n");
    write(&repo.join("build/big"), LINK_MIN);
    put(&repo.join("logs/today.log"), "started\n");

    let out = sb.ok(&sb.root, &["new", "graft", "repos/api"]);
    assert!(!out.contains("  copy"), "{out}");
    let tree = sb.forest("graft").join("api");
    assert!(same_file(&repo.join("build/big"), &tree.join("build/big")));
    assert!(!tree.join("logs").exists());
}

#[test]
fn a_cache_inside_an_ignored_directory_is_grafted_and_the_rest_copied() {
    let sb = Sandbox::new();
    let repo = sb.cached_repo("api", "out/cache clone\n");
    sb.publish(&repo, ".gitignore", "/out\n");
    put(&repo.join("out/report.txt"), "report\n");
    write(&repo.join("out/cache/big"), LINK_MIN);

    let out = sb.ok(&sb.root, &["new", "nested", "repos/api"]);
    assert!(out.contains("  copy out: 1 file, 7 B\n"), "{out}");
    assert!(out.contains("  cache out/cache: grafted"), "{out}");
    let tree = sb.forest("nested").join("api");
    assert_eq!(read(&tree.join("out/report.txt")), "report\n");
    assert!(!same_file(
        &repo.join("out/report.txt"),
        &tree.join("out/report.txt")
    ));
    assert!(same_file(
        &repo.join("out/cache/big"),
        &tree.join("out/cache/big")
    ));
}

#[test]
fn what_names_the_path_it_was_made_at_is_left_out() {
    let sb = Sandbox::new();
    let repo = sb.ignoring("api", "/.venv\n/cmake-build\n/tools\n/android/.gradle\n");
    put(&repo.join(".venv/pyvenv.cfg"), "home = /usr/bin\n");
    put(&repo.join(".venv/bin/activate"), "VIRTUAL_ENV=/main\n");
    put(
        &repo.join("cmake-build/CMakeCache.txt"),
        "CMAKE_HOME_DIRECTORY=/main\n",
    );
    put(&repo.join("tools/run.sh"), "echo hi\n");
    put(&repo.join("tools/env/pyvenv.cfg"), "home = /usr/bin\n");
    put(&repo.join("android/.gradle/file-system.probe"), "probe\n");

    let out = sb.ok(&sb.root, &["new", "venv", "repos/api"]);
    for line in [
        "  copy .venv: left out, since a Python venv names the path it was made at\n",
        "  copy cmake-build: left out, since a CMake build directory names the source",
        "  copy tools/env: left out, since a Python venv names the path it was made at\n",
        "  copy android/.gradle: left out, since Gradle's .gradle doesn't work at another path\n",
        "  copy tools: 1 file, 8 B\n",
    ] {
        assert!(out.contains(line), "{out}");
    }
    let tree = sb.forest("venv").join("api");
    for gone in [".venv", "cmake-build", "tools/env", "android"] {
        assert!(!tree.join(gone).exists(), "{gone}");
    }
    assert_eq!(read(&tree.join("tools/run.sh")), "echo hi\n");
}

#[test]
fn a_venv_that_ignores_itself_is_left_out_whole() {
    let sb = Sandbox::new();
    let repo = sb.ignoring("api", ".env\n");
    // How `python -m venv` makes one: it ignores itself from inside.
    put(&repo.join(".venv/.gitignore"), "*\n");
    put(&repo.join(".venv/pyvenv.cfg"), "home = /usr/bin\n");
    put(&repo.join(".venv/bin/activate"), "VIRTUAL_ENV=/main\n");

    let out = sb.ok(&sb.root, &["new", "venv", "repos/api"]);
    assert!(
        out.contains("  copy .venv: left out, since a Python venv names the path it was made at\n"),
        "{out}"
    );
    assert_eq!(out.matches("  copy").count(), 1, "{out}");
    assert!(!sb.forest("venv").join("api/.venv").exists());
}

#[test]
fn a_worktree_inside_an_ignored_directory_is_left_out() {
    let sb = Sandbox::new();
    let repo = sb.ignoring("api", "/nested\n");
    sb.git(
        &repo,
        &["worktree", "add", "--quiet", "-b", "side", "nested/side"],
    );
    put(&repo.join("nested/notes.txt"), "notes\n");

    let out = sb.ok(&sb.root, &["new", "wt", "repos/api"]);
    assert!(
        out.contains(
            "  copy nested/side: left out, since a git worktree or submodule names the repository"
        ),
        "{out}"
    );
    assert!(out.contains("  copy nested: 1 file, 6 B\n"), "{out}");
    let tree = sb.forest("wt").join("api");
    assert!(!tree.join("nested/side").exists());
    assert_eq!(read(&tree.join("nested/notes.txt")), "notes\n");
}

#[test]
fn a_path_the_trees_branch_does_not_ignore_is_left_out() {
    let sb = Sandbox::new();
    let repo = sb.ignoring("api", ".env\n");
    // Ignored on the main checkout's branch, which the tree's base doesn't have.
    fs::write(repo.join(".gitignore"), ".env\nlocal.json\n").unwrap();
    sb.git(&repo, &["commit", "--quiet", "--all", "--message", "local"]);
    put(&repo.join("local.json"), "{}\n");

    let out = sb.ok(&sb.root, &["new", "base", "repos/api"]);
    assert!(
        out.contains("  copy local.json: left out, since the tree's branch doesn't ignore it"),
        "{out}"
    );
    let tree = sb.forest("base").join("api");
    assert!(!tree.join("local.json").exists());
    assert_eq!(sb.changes(&tree), "");
}

#[test]
fn cache_graft_copies_what_a_tree_lacks_and_replaces_nothing() {
    let sb = Sandbox::new();
    let repo = sb.ignoring("api", ".env\n");
    put(&repo.join(".env"), "TOKEN=main\n");
    sb.ok(&sb.root, &["new", "later", "repos/api", "--clean"]);
    let tree = sb.forest("later").join("api");
    assert!(!tree.join(".env").exists());

    let out = sb.ok(&tree, &["cache", "graft"]);
    assert!(out.contains("  copy .env: 11 B\n"), "{out}");
    assert_eq!(read(&tree.join(".env")), "TOKEN=main\n");

    put(&tree.join(".env"), "TOKEN=tree\n");
    put(&repo.join(".env"), "TOKEN=new\n");
    let out = sb.ok(&tree, &["cache", "graft", "--force"]);
    assert!(!out.contains("  copy"), "{out}");
    assert_eq!(read(&tree.join(".env")), "TOKEN=tree\n");
}

#[test]
fn a_sparse_tree_gets_copies_at_its_top_level_and_in_its_directories() {
    let sb = Sandbox::new();
    let repo = sb.ignoring("api", ".env\n");
    for dir in ["a", "b"] {
        fs::create_dir(repo.join(dir)).unwrap();
        sb.publish(&repo, &format!("{dir}/README"), "readme\n");
        put(&repo.join(dir).join(".env"), "IN=1\n");
    }
    put(&repo.join(".env"), "TOP=1\n");

    sb.ok(&sb.root, &["new", "thin", "repos/api", "--sparse", "a"]);
    let tree = sb.forest("thin").join("api");
    assert_eq!(read(&tree.join(".env")), "TOP=1\n");
    assert_eq!(read(&tree.join("a/.env")), "IN=1\n");
    assert!(!tree.join("b").exists());
}

#[test]
fn an_ignored_symlink_is_copied_as_a_symlink() {
    let sb = Sandbox::new();
    let repo = sb.ignoring("api", "/current\n");
    symlink("releases/v2", repo.join("current")).unwrap();

    let out = sb.ok(&sb.root, &["new", "link", "repos/api"]);
    assert!(out.contains("  copy current: a symlink\n"), "{out}");
    let copied = fs::read_link(sb.forest("link").join("api/current")).unwrap();
    assert_eq!(copied, Path::new("releases/v2"));
}

#[test]
fn copies_keep_their_modification_times_and_modes() {
    let sb = Sandbox::new();
    let repo = sb.ignoring("api", ".env\n/tools\n");
    put(&repo.join(".env"), "TOKEN=main\n");
    put(&repo.join("tools/bin/tool"), "#!/bin/sh\n");
    let tool = repo.join("tools/bin/tool");
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    set_modified(&repo.join(".env"), 1_000_000_000);
    set_modified(&tool, 1_000_000_000);

    sb.ok(&sb.root, &["new", "times", "repos/api"]);
    let tree = sb.forest("times").join("api");
    for file in [".env", "tools/bin/tool"] {
        let copied = meta(&tree.join(file)).modified().unwrap();
        assert_eq!(copied, at(1_000_000_000), "{file}");
    }
    let mode = meta(&tree.join("tools/bin/tool")).permissions().mode();
    assert_eq!(mode & 0o777, 0o755);
}

#[test]
fn a_later_copy_marks_the_trees_older_files_as_changed() {
    let sb = Sandbox::new();
    let repo = sb.ignoring("api", "/dist\n");
    sb.ok(&sb.root, &["new", "later", "repos/api", "--clean"]);
    let tree = sb.forest("later").join("api");
    // Output the main checkout built after the tree's README last changed.
    set_modified(&tree.join("README"), 1_000_000_000);
    put(&repo.join("dist/app.js"), "built\n");
    set_modified(&repo.join("dist/app.js"), 1_500_000_000);

    let out = sb.ok(&tree, &["cache", "graft"]);
    assert!(out.contains("  copy dist: 1 file, 6 B\n"), "{out}");
    assert!(
        out.contains("  copy: marked the tree's 1 file older than the copies as changed\n"),
        "{out}"
    );
    assert!(meta(&tree.join("README")).modified().unwrap() > at(1_500_000_000));
    assert_eq!(
        meta(&tree.join("dist/app.js")).modified().unwrap(),
        at(1_500_000_000)
    );
}

#[test]
fn many_copies_are_summed_up_after_the_first_few() {
    let sb = Sandbox::new();
    let repo = sb.ignoring("api", "*.local\n");
    for n in 0..10 {
        put(&repo.join(format!("{n}.local")), "x\n");
    }

    let out = sb.ok(&sb.root, &["new", "many", "repos/api"]);
    assert_eq!(out.matches("  copy ").count(), 8, "{out}");
    assert!(
        out.contains("  copy: 2 more paths, 2 files, 4 B\n"),
        "{out}"
    );
    let tree = sb.forest("many").join("api");
    for n in 0..10 {
        assert!(tree.join(format!("{n}.local")).exists(), "{n}.local");
    }
}

#[test]
fn what_an_interrupted_copy_left_is_cleared_away() {
    let sb = Sandbox::new();
    let repo = sb.ignoring("api", "/node_modules\n");
    put(&repo.join("node_modules/pkg/index.js"), "x\n");
    sb.ok(&sb.root, &["new", "again", "repos/api", "--clean"]);
    let tree = sb.forest("again").join("api");
    // A copy cut short leaves what it staged beside where it was going.
    put(
        &tree.join(".node_modules.workforest-copy/pkg/half"),
        "half\n",
    );

    let out = sb.ok(&tree, &["cache", "graft"]);
    assert!(out.contains("  copy node_modules: 1 file, 2 B\n"), "{out}");
    assert!(!tree.join(".node_modules.workforest-copy").exists());
    assert_eq!(read(&tree.join("node_modules/pkg/index.js")), "x\n");
    assert!(!tree.join("node_modules/pkg/half").exists());
    assert_eq!(sb.changes(&tree), "");
}

#[test]
fn an_ignored_directory_with_nothing_left_to_copy_stays_behind() {
    let sb = Sandbox::new();
    let repo = sb.ignoring("api", "/py\n");
    put(&repo.join("py/.venv/pyvenv.cfg"), "home = /usr/bin\n");

    let out = sb.ok(&sb.root, &["new", "empty", "repos/api"]);
    assert!(
        out.contains("  copy py/.venv: left out, since a Python venv"),
        "{out}"
    );
    assert!(!out.contains("  copy py:"), "{out}");
    let tree = sb.forest("empty").join("api");
    assert!(!tree.join("py").exists());
    assert!(!tree.join(".py.workforest-copy").exists());
}
