//! `fire`: which trees are dead, and which forests it burns. Every kind of live
//! tree that could pass for dead is left standing.

use std::fs;

use super::{Merge, Sandbox, path};

#[test]
fn fire_burns_forests_whose_work_landed_however_it_was_merged() {
    for merge in [Merge::Regular, Merge::Squash, Merge::Rebase] {
        let sb = Sandbox::new();
        let api = sb.repo("api");
        sb.pull_request("pr", "api");
        sb.merge_on_remote("api", "pr", merge);

        // fire fetches the merge itself; nothing here has fetched it.
        let plan = sb.ok(&sb.root, &["fire"]);
        assert!(plan.contains("pr  burn\n"), "{merge:?}: {plan}");
        assert!(
            plan.contains(&format!(
                "  {:<24} {:<6} landed on origin/main\n",
                "api", "dead"
            )),
            "{merge:?}: {plan}"
        );
        assert!(
            plan.contains("dry run: pass --yes to burn 1 forest(s)"),
            "{merge:?}: {plan}"
        );
        assert!(
            sb.forest("pr").exists(),
            "{merge:?}: a dry run burns nothing"
        );

        let out = sb.ok(&sb.root, &["fire", "--yes", "--delete-branches"]);
        assert!(out.contains("burned forest pr"), "{merge:?}: {out}");
        assert!(!sb.forest("pr").exists(), "{merge:?}");
        assert_eq!(sb.git(&api, &["branch", "--list", "pr"]), "", "{merge:?}");
    }
}

#[test]
fn fire_leaves_live_trees_that_could_pass_for_dead() {
    let sb = Sandbox::new();
    let api = sb.repo("api");

    // Planted, then moved along its base without a commit of its own.
    sb.ok(&sb.root, &["new", "fresh", "repos/api"]);
    sb.ok(&sb.root, &["new", "pulled", "repos/api"]);
    let forge = sb.forge("api");
    sb.commit(&forge, "moved-on.txt");
    sb.git(&forge, &["push", "--quiet", "origin", "main"]);
    let pulled = sb.forest("pulled").join("api");
    sb.git(&pulled, &["pull", "--quiet", "--ff-only"]);

    // A commit made, then discarded to start over: its reflog records a
    // commit, but not one that is on the base.
    sb.ok(&sb.root, &["new", "restarted", "repos/api"]);
    let restarted = sb.forest("restarted").join("api");
    sb.commit(&restarted, "attempt.txt");
    sb.git(&restarted, &["reset", "--quiet", "--hard", "origin/main"]);

    // Carrying on with a pushed branch, planted off that branch itself, so
    // reaching its base only means being pushed.
    sb.git(&api, &["push", "--quiet", "origin", "main:carry-on"]);
    sb.git(&api, &["fetch", "--quiet"]);
    sb.ok(
        &sb.root,
        &["new", "carry-on", "repos/api", "-B", "origin/carry-on"],
    );
    let carry_on = sb.forest("carry-on").join("api");
    sb.commit(&carry_on, "carry-on.txt");
    sb.git(&carry_on, &["push", "--quiet"]);

    // Pushed for review, not merged.
    sb.pull_request("open", "api");

    // Landed, then edited again without committing.
    let edited = sb.pull_request("edited", "api");
    sb.merge_on_remote("api", "edited", Merge::Squash);
    fs::write(edited.join("edited-1.txt"), "again\n").unwrap();

    // Landed, then given a new file that isn't committed or ignored.
    let untracked = sb.pull_request("untracked", "api");
    sb.merge_on_remote("api", "untracked", Merge::Rebase);
    fs::write(untracked.join("notes.txt"), "keep me\n").unwrap();

    // Only the first of two commits reached main.
    sb.pull_request("partial", "api");
    let forge = sb.forge("api");
    sb.git(&forge, &["cherry-pick", "origin/partial~1"]);
    sb.push_main_deleting(&forge, "partial");

    // Work committed after the merge.
    let more = sb.pull_request("more", "api");
    sb.merge_on_remote("api", "more", Merge::Regular);
    sb.commit(&more, "more-3.txt");

    // A merge that main has since reverted.
    sb.pull_request("reverted", "api");
    sb.merge_on_remote("api", "reverted", Merge::Squash);
    let forge = sb.forge("api");
    sb.git(&forge, &["revert", "--no-edit", "HEAD"]);
    sb.git(&forge, &["push", "--quiet", "origin", "main"]);

    let out = sb.ok(&sb.root, &["fire", "--yes", "--delete-branches"]);

    let forests = [
        "fresh",
        "pulled",
        "restarted",
        "carry-on",
        "open",
        "edited",
        "untracked",
        "partial",
        "more",
        "reverted",
    ];
    assert!(
        out.contains(&format!("{} other forest(s) in flight", forests.len())),
        "{out}"
    );
    assert!(out.contains("nothing to burn"), "{out}");
    for forest in forests {
        assert!(sb.forest(forest).join("api").is_dir(), "{forest}: {out}");
        assert!(
            sb.git(&api, &["branch", "--list", forest]).contains(forest),
            "{forest}"
        );
    }
    let plan = sb.ok(&sb.root, &["fire", "--no-fetch", "--scorch"]);
    assert!(plan.contains("nothing to burn"), "{plan}");
}

#[test]
fn fire_burns_a_forest_only_once_every_tree_in_it_is_dead() {
    let sb = Sandbox::new();
    sb.repo("api");
    sb.repo("web");
    sb.ok(&sb.root, &["new", "both", "repos/api", "repos/web"]);
    let forest = sb.forest("both");
    for repo in ["api", "web"] {
        let tree = forest.join(repo);
        sb.commit(&tree, &format!("{repo}.txt"));
        sb.git(&tree, &["push", "--quiet", "-u", "origin", "HEAD"]);
    }
    sb.merge_on_remote("api", "both", Merge::Squash);

    let kept = sb.ok(&sb.root, &["fire", "--yes"]);
    assert!(kept.contains("both  keep\n"), "{kept}");
    assert!(
        kept.contains(&format!(
            "  {:<24} {:<6} 1 commit(s) not on origin/main\n",
            "web", "live"
        )),
        "{kept}"
    );
    assert!(
        kept.contains("1 forest(s) kept for their live trees hold dead ones; --scorch cuts those"),
        "{kept}"
    );
    assert!(forest.join("api").is_dir() && forest.join("web").is_dir());

    let plan = sb.ok(&sb.root, &["fire", "--scorch"]);
    assert!(plan.contains("both  cut its dead trees\n"), "{plan}");
    assert!(
        plan.contains("dry run: pass --yes to cut 1 tree(s)"),
        "{plan}"
    );
    let scorched = sb.ok(
        &sb.root,
        &["fire", "--scorch", "--yes", "--delete-branches"],
    );
    assert!(scorched.contains("cut api from both"), "{scorched}");
    assert!(!forest.join("api").exists());
    assert!(forest.join("web").is_dir());
    assert!(!sb.ok(&sb.root, &["ls", "both"]).contains("api"));
    assert_eq!(
        sb.git(&sb.repos().join("api"), &["branch", "--list", "both"]),
        ""
    );
    assert!(
        sb.git(&sb.repos().join("web"), &["branch", "--list", "both"])
            .contains("both")
    );
}

#[test]
fn fire_burns_missing_trees_but_keeps_branches_with_work_on_them() {
    let sb = Sandbox::new();
    let api = sb.repo("api");
    sb.repo("web");
    sb.ok(&sb.root, &["new", "gone", "repos/api", "repos/web"]);
    let forest = sb.forest("gone");
    sb.commit(&forest.join("api"), "unlanded.txt");
    fs::remove_dir_all(forest.join("api")).unwrap();
    fs::remove_dir_all(forest.join("web")).unwrap();
    sb.ok(&sb.root, &["new", "empty"]);

    let out = sb.ok(&sb.root, &["fire", "--yes", "--delete-branches"]);

    assert!(out.contains("gone  burn\n"), "{out}");
    assert!(
        out.contains(&format!(
            "  {:<24} {:<6} missing; keeps branch gone, which has commits not on origin/main\n",
            "api", "dead"
        )),
        "{out}"
    );
    assert!(
        out.contains(&format!("  {:<24} {:<6} missing\n", "web", "dead")),
        "{out}"
    );
    assert!(out.contains("empty  burn\n  no trees\n"), "{out}");
    assert!(!forest.exists() && !sb.forest("empty").exists());
    assert!(sb.git(&api, &["branch", "--list", "gone"]).contains("gone"));
    let web = sb.repos().join("web");
    assert_eq!(sb.git(&web, &["branch", "--list", "gone"]), "");
    assert!(!sb.git(&api, &["worktree", "list"]).contains(path(&forest)));
}

#[test]
fn fire_leaves_trees_it_cannot_judge() {
    let sb = Sandbox::new();
    let api = sb.repo("api");
    let landed = |forest: &str| {
        sb.pull_request(forest, "api");
        sb.merge_on_remote("api", forest, Merge::Squash);
        sb.forest(forest).join("api")
    };

    let detached = landed("detached");
    sb.git(&detached, &["checkout", "--quiet", "--detach"]);
    let switched = landed("switched");
    sb.git(&switched, &["checkout", "--quiet", "-b", "elsewhere"]);
    let locked = landed("locked");
    sb.git(&api, &["worktree", "lock", path(&locked)]);
    let unlisted = landed("unlisted");
    landed("stray");
    sb.git(
        &api,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "by-hand",
            path(&sb.forest("stray").join("extra")),
        ],
    );

    // A stacked branch, whose base branch is deleted once it merges.
    sb.pull_request("lower", "api");
    sb.ok(&sb.root, &["burn", "lower"]);
    sb.ok(
        &sb.root,
        &["new", "upper", "repos/api", "-B", "origin/lower"],
    );
    let upper = sb.forest("upper").join("api");
    sb.commit(&upper, "upper.txt");
    sb.git(&upper, &["push", "--quiet", "-u", "origin", "HEAD"]);
    sb.merge_on_remote("api", "lower", Merge::Squash);
    sb.git(&upper, &["fetch", "--quiet", "--prune"]);

    // Last, so that no later worktree takes over the record's name.
    let record = sb.git(&unlisted, &["rev-parse", "--absolute-git-dir"]);
    fs::remove_dir_all(record).unwrap();

    let out = sb.ok(&sb.root, &["fire", "--yes"]);

    for (forest, reason) in [
        ("detached", "its HEAD is detached".to_owned()),
        (
            "switched",
            "it has elsewhere checked out, not its branch switched".to_owned(),
        ),
        ("upper", "its base origin/lower no longer exists".to_owned()),
        (
            "locked",
            "it is locked: `git worktree unlock` it to let it burn".to_owned(),
        ),
        (
            "unlisted",
            "git can't read it: its repo may no longer list it as a worktree".to_owned(),
        ),
    ] {
        assert!(
            out.contains(&format!("{forest}  keep\n")),
            "{forest}: {out}"
        );
        assert!(
            out.contains(&format!("  {:<24} {:<6} {reason}\n", "api", "?")),
            "{forest}: {out}"
        );
        assert!(sb.forest(forest).join("api").is_dir(), "{forest}");
    }
    assert!(
        out.contains(&format!(
            "  {:<24} {:<6} a checkout the manifest doesn't record\n",
            "extra", "?"
        )),
        "{out}"
    );
    assert!(sb.forest("stray").join("extra").is_dir());
    assert!(out.contains("nothing to burn"), "{out}");
}

#[test]
fn fire_leaves_a_tree_whose_repo_is_gone() {
    let sb = Sandbox::new();
    let api = sb.repo("api");
    sb.pull_request("moved", "api");
    sb.merge_on_remote("api", "moved", Merge::Squash);
    let moved = sb.root.join("moved-api");
    fs::rename(&api, &moved).unwrap();

    let out = sb.ok(&sb.root, &["fire", "--yes"]);

    assert!(
        out.contains(&format!("its repo {} is gone", api.display())),
        "{out}"
    );
    assert!(sb.forest("moved").join("api").is_dir());
}

#[test]
fn fire_judges_bases_as_last_fetched_when_it_cannot_or_may_not_fetch() {
    let sb = Sandbox::new();
    sb.repo("api");
    sb.pull_request("pr", "api");
    sb.merge_on_remote("api", "pr", Merge::Squash);

    let stale = sb.ok(&sb.root, &["fire", "--no-fetch"]);
    assert!(stale.contains("1 other forest(s) in flight"), "{stale}");

    let remote = sb.root.join("remotes").join("api.git");
    let away = sb.root.join("remotes").join("away.git");
    fs::rename(&remote, &away).unwrap();
    let output = sb.workforest(&sb.root, &["fire"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(
        stderr.contains("could not fetch origin into")
            && stderr.contains("judging it as last fetched"),
        "{stderr}"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("nothing to burn"));

    fs::rename(&away, &remote).unwrap();
    assert!(sb.ok(&sb.root, &["fire"]).contains("pr  burn\n"));
}

#[test]
fn fire_says_where_to_go_after_burning_the_forest_you_are_in() {
    let sb = Sandbox::new();
    let api = sb.repo("api");
    let tree = sb.pull_request("here", "api");
    sb.merge_on_remote("api", "here", Merge::Rebase);

    let output = sb.workforest(&tree, &["fire", "--yes"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(!sb.forest("here").exists());
    assert!(
        stderr.contains(&format!("; cd {}", api.display())),
        "{stderr}"
    );
}

#[test]
fn fire_burns_work_merged_by_fast_forward() {
    let sb = Sandbox::new();
    sb.repo("api");
    sb.pull_request("ff", "api");
    let forge = sb.forge("api");
    sb.git(&forge, &["merge", "--quiet", "--ff-only", "origin/ff"]);
    sb.push_main_deleting(&forge, "ff");

    let out = sb.ok(&sb.root, &["fire", "--yes"]);

    assert!(out.contains("ff  burn\n"), "{out}");
    assert!(!sb.forest("ff").exists());
}

#[test]
fn fire_judges_a_tree_based_on_its_own_branch_against_the_default_branch() {
    let sb = Sandbox::new();
    let api = sb.repo("api");
    sb.git(&api, &["push", "--quiet", "origin", "main:carry-on"]);
    sb.git(&api, &["fetch", "--quiet"]);
    sb.ok(
        &sb.root,
        &["new", "carry-on", "repos/api", "-B", "origin/carry-on"],
    );
    let tree = sb.forest("carry-on").join("api");
    sb.commit(&tree, "carry-on.txt");
    sb.git(&tree, &["push", "--quiet"]);
    let status = sb.ok(&sb.root, &["status", "carry-on"]);
    assert!(
        status.contains("clean  +0/-0 vs origin/carry-on\n"),
        "{status}"
    );

    sb.merge_on_remote("api", "carry-on", Merge::Squash);

    let out = sb.ok(&sb.root, &["fire"]);
    assert!(out.contains("carry-on  burn\n"), "{out}");
    assert!(out.contains("landed on origin/main\n"), "{out}");
}

#[test]
fn fire_burns_work_landed_on_a_local_base() {
    let sb = Sandbox::new();
    let api = sb.repo("api");
    sb.ok(&sb.root, &["new", "loc", "repos/api", "-B", "main"]);
    sb.commit(&sb.forest("loc").join("api"), "work.txt");
    sb.git(&api, &["merge", "--quiet", "--no-ff", "--no-edit", "loc"]);

    let out = sb.ok(&sb.root, &["fire", "--yes"]);

    assert!(out.contains("landed on main\n"), "{out}");
    assert!(!sb.forest("loc").exists());
}

#[test]
fn fire_fetches_the_remote_each_base_is_on() {
    let sb = Sandbox::new();
    let api = sb.repo("api");
    let remote = sb.root.join("remotes").join("api.git");
    sb.git(&api, &["remote", "add", "upstream", path(&remote)]);
    sb.git(&api, &["fetch", "--quiet", "upstream"]);
    sb.ok(
        &sb.root,
        &["new", "fork", "repos/api", "-B", "upstream/main"],
    );
    let tree = sb.forest("fork").join("api");
    sb.commit(&tree, "fork.txt");
    sb.git(&tree, &["push", "--quiet", "-u", "origin", "HEAD"]);
    sb.merge_on_remote("api", "fork", Merge::Squash);

    // Only a fetch of upstream, not of origin, shows the merge.
    let out = sb.ok(&sb.root, &["fire"]);

    assert!(out.contains("fork  burn\n"), "{out}");
    assert!(out.contains("landed on upstream/main\n"), "{out}");
}

#[test]
fn fire_burns_work_that_landed_without_being_pushed() {
    let sb = Sandbox::new();
    sb.repo("api");
    sb.ok(&sb.root, &["new", "twin", "repos/api"]);
    sb.commit(&sb.forest("twin").join("api"), "twin.txt");
    let forge = sb.forge("api");
    sb.commit(&forge, "twin.txt");
    sb.git(&forge, &["push", "--quiet", "origin", "main"]);

    let out = sb.ok(&sb.root, &["fire", "--yes"]);

    assert!(out.contains("twin  burn\n"), "{out}");
    assert!(!sb.forest("twin").exists());
}

#[test]
fn fire_leaves_landed_work_it_can_no_longer_recognise() {
    let sb = Sandbox::new();
    let api = sb.repo("api");
    let lines: String = (1..=9).map(|n| format!("{n}\n")).collect();
    fs::write(api.join("lines"), lines).unwrap();
    sb.git(&api, &["add", "lines"]);
    sb.git(&api, &["commit", "--quiet", "--message", "lines"]);
    sb.git(&api, &["push", "--quiet", "origin", "main"]);
    let edit = |dir: &std::path::Path, from: &str, to: &str| {
        let text = fs::read_to_string(dir.join("lines")).unwrap();
        fs::write(
            dir.join("lines"),
            text.replace(&format!("\n{from}\n"), &format!("\n{to}\n")),
        )
        .unwrap();
        sb.git(dir, &["commit", "--quiet", "--all", "--message", to]);
    };
    sb.ok(&sb.root, &["new", "near", "repos/api"]);
    let tree = sb.forest("near").join("api");
    edit(&tree, "4", "four");
    sb.git(&tree, &["push", "--quiet", "-u", "origin", "HEAD"]);
    sb.merge_on_remote("api", "near", Merge::Squash);

    // A later change to the line next to the landed one, as lockfiles see all
    // the time, hides the landing from the reverse apply. The tree stays live
    // rather than risk a wrong burn.
    let forge = sb.forge("api");
    edit(&forge, "5", "five");
    sb.git(&forge, &["push", "--quiet", "origin", "main"]);

    let out = sb.ok(&sb.root, &["fire", "--yes"]);
    assert!(out.contains("1 other forest(s) in flight"), "{out}");
    assert!(tree.is_dir());
}

#[test]
fn fire_carries_on_past_a_forest_it_cannot_read() {
    let sb = Sandbox::new();
    sb.repo("api");
    sb.pull_request("good", "api");
    sb.merge_on_remote("api", "good", Merge::Squash);
    sb.ok(&sb.root, &["new", "broken"]);
    let manifest = sb.forest("broken").join(".workforest");
    fs::remove_file(&manifest).unwrap();
    fs::create_dir(&manifest).unwrap();

    let output = sb.workforest(&sb.root, &["fire", "--yes"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(
        stderr.contains("workforest: broken: could not read"),
        "{stderr}"
    );
    assert!(
        stderr.contains("could not finish with 1 forest(s)"),
        "{stderr}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("burned forest good"), "{stdout}");
    assert!(sb.forest("broken").exists());
}

#[test]
fn fire_leaves_a_tree_whose_record_another_worktree_took_over() {
    let sb = Sandbox::new();
    let api = sb.repo("api");
    let orphan = sb.pull_request("orphan", "api");
    let record = sb.git(&orphan, &["rev-parse", "--absolute-git-dir"]);
    fs::remove_dir_all(&record).unwrap();
    // A new tree of the same repo takes the freed record name, so the orphan's
    // .git now leads to it, and to a landed branch.
    sb.pull_request("taker", "api");
    sb.merge_on_remote("api", "taker", Merge::Squash);
    let taker = sb.forest("taker").join("api");
    assert_eq!(sb.git(&taker, &["rev-parse", "--absolute-git-dir"]), record);

    let out = sb.ok(&sb.root, &["fire", "--yes"]);

    assert!(out.contains("orphan  keep\n"), "{out}");
    assert!(
        out.contains("its repo's record of it belongs to another worktree"),
        "{out}"
    );
    assert!(orphan.is_dir());
    assert!(out.contains("burned forest taker"), "{out}");
    assert_eq!(sb.git(&api, &["worktree", "list"]).lines().count(), 1);
}
