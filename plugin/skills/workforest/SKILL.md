---
name: workforest
description: Manage "workforests" — collections of git worktrees under ~/.workforest, one per repo involved in a piece of work, so a feature is developed in isolation without ever touching the repos' main checkouts. Use whenever isolation is wanted, INCLUDING single-repo work — a forest with one tree is normal and expected. Use when a task spans two or more repos, when a feature should not disturb the main checkout, when several sessions or agents may work in parallel, when starting/switching/cleaning up a feature branch, or when the user says forest, plant, cut, burn, worktree, isolate, or asks to work on several repos at once.
---

# workforest

A **forest** is a directory under `~/.workforest/` holding one git worktree ("tree")
per repo involved in a piece of work. Each tree is a worktree of a repo's main
checkout, wherever that lives, checked out on a shared branch named after the
forest. The main checkouts are never modified — no branch switching, no
stashing — so several pieces of work can be in flight at once.

The unit of a forest is **a piece of work**, not a set of repos. One repo is a
perfectly normal forest. Isolation is the point; spanning repos is just
something forests also happen to do.

```
~/.workforest/auth-migration/
├── .workforest     manifest: repo \t source repo \t branch \t base ref
├── api/            worktree of ~/code/api on branch auth-migration
└── web/            worktree of ~/code/web on branch auth-migration
```

A tree costs only its checked-out files: worktrees share their repo's object
store, so no history is copied.

## Before first use

This skill drives the `workforest` command-line tool, version 0.2.0 or later.
Check that it is installed:

```sh
workforest --version
```

If the command is missing or older, stop before touching any repo and ask the
user how to install it. Ask in so many words: a permission prompt is not
consent, because some sessions run without them. Offer only what works on this
machine:

| on the machine | offer |
| --- | --- |
| packages managed by Home Manager or NixOS | install nothing yourself; show the flake input `workforest.url = "github:Nixotica/workforest/release";` and adding `workforest.packages.<system>.default` to their packages |
| `nix` | `nix profile add github:Nixotica/workforest/release` (older Nix calls it `nix profile install`) |
| `cargo` | `cargo install --locked --git https://github.com/Nixotica/workforest --branch release` |

If none of these fit, point the user at
<https://github.com/Nixotica/workforest#install>. After installing, run
`workforest --version` again, then carry on with the task.

If a command or flag below is reported as unknown, the installed tool is older
than this skill. Offer to update it the same way it was installed: for a Nix
profile, `nix profile list` shows the entry to `nix profile upgrade`; for cargo,
re-run the `cargo install` with `--force`.

## When to start a forest

Start one with `workforest new` — and do the work inside it — whenever any of
these hold, **regardless of how many repos are involved**:

- The user asked for a forest, a worktree, or isolation, by any wording.
- The work is a named feature, fix, or experiment that wants its own branch.
- More than one session, agent, or terminal may touch the same repo.
- The change should not disturb whatever is currently checked out in the main
  checkout.

**A single repo never disqualifies a forest.** `workforest new my-feature ~/code/myrepo`
creates a one-tree forest and is a first-class, expected use. If the user asks
to use workforest and the work touches one repo, start the forest anyway — do
not substitute a plain branch in the main checkout, do not "simplify" to
`git checkout -b`, and do not ask whether a forest is worth it.

Working directly in a main checkout when a forest was asked for is the specific
failure this skill exists to prevent: parallel sessions share that one checkout,
so they switch each other's branches and clobber each other's uncommitted
changes.

Only skip the forest when the work is genuinely not feature work on a checkout —
read-only inspection, or a change to a repo already checked out as a tree in the
forest you're standing in.

## Commands

| command | what it does |
| --- | --- |
| `workforest new <forest> [repo path...]` | start a forest, optionally planting trees right away |
| `workforest plant <repo path>...` | add trees (worktrees) to a forest |
| `workforest cut <tree>...` | remove trees from a forest, by name |
| `workforest burn [forest]` | remove a forest and every tree in it |
| `workforest ls [forest]` | list forests, or the trees in one |
| `workforest status [forest]` | per-tree branch, clean/dirty/landed, ahead/behind its base, pushed or not |
| `workforest path [forest]` | print a forest's path |

Aliases: `add`=`plant`, `remove`=`cut`, `rm`/`delete`=`burn`,
`list`=`ls`, `st`=`status`, `dir`=`path`.

`wf` is a short name for `workforest` itself, which the Nix package installs.
Users may type either; in commands you run, use `workforest`, which every
install provides.

Options:

- `-f, --forest <name>` (`plant`, `cut`) — target forest. Defaults to
  the forest containing the current directory, so inside a tree you can omit
  it. `status`, `path` and `burn` take the forest as an optional argument with the
  same default.
- `-b, --branch <name>` — branch to check out or create. Defaults to the forest
  name, giving every repo the same branch name. If the branch already exists in
  a repo, it is checked out rather than recreated.
- `-B, --base <ref>` — what to branch off. Defaults to `origin/HEAD`, falling
  back to a local `main` or `master`.
- `--force` — on `cut`/`burn`, skip the safety checks for uncommitted changes
  and for commits that are neither pushed nor landed.
- `--delete-branches` — on `cut`/`burn`, also delete the trees' branches.

Repos are always given as paths: absolute, or relative to the current
directory, where `.` is the repo you're in. workforest assumes nothing about
where repos live, so a bare name like `api` is rejected. In commands you run,
pass absolute paths. When the user names a repo without saying where it is,
find it or ask; don't guess.

Environment: `WORKFOREST_ROOT` (default `~/.workforest`). Where this skill says
`~/.workforest`, read that value if it is set.

## Typical flow

```sh
workforest new fix-login ~/code/api           # single-repo forest — a normal case
cd "$(workforest path fix-login)/api"
# ...edit, commit, push, merge, then...
git fetch                                     # burn checks the base as last fetched
workforest burn fix-login
```

Multi-repo is the same commands with more arguments:

```sh
workforest new auth-migration ~/code/api ~/code/web    # both on branch auth-migration
cd "$(workforest path auth-migration)"
# ...edit across api/ and web/...
workforest plant ~/code/docs                  # a third repo turned out to be involved
workforest status                             # what's dirty, what's ahead
workforest burn auth-migration --delete-branches
```

## Burn the forest once its work has landed

A forest is scaffolding for one piece of work. When that work is merged, burn
it: a forest left standing keeps a checkout on disk, and `workforest ls` stops
being a picture of what is really in flight.

`burn` refuses to remove a tree with uncommitted changes, or with commits that
are neither pushed nor landed on its base. Work has landed once everything its
branch changed is on the base, so a regular, squash or rebase merge all count,
whether or not the merged branch was deleted. `status` shows such a tree as
`landed`.

`burn` checks the base as this machine last fetched it, so fetch each tree's
repo after merging, then burn:

```sh
for t in "$(workforest path <forest>)"/*/; do git -C "$t" fetch --quiet; done
workforest burn <forest> --delete-branches
```

A refusal after fetching means some of the work is not on the base: commits
made after the merge, a merge that took only part of the branch, or a merge
since reverted. It also refuses when the base has since rewritten lines next to
the branch's changes, since it can no longer tell. Show the user what differs
(`git diff origin/<base> HEAD -- <files the branch changed>`) and ask; do not
reach for `--force` on the user's behalf.

Burning the forest you're standing in works, but leaves your shell in a
directory that no longer exists; workforest then prints where to `cd`. Prefer
`workforest burn <forest>` from outside the forest.

## Guidance for Claude

- Start the forest **before** the first edit, whenever the "When to start a
  forest" rules above apply. Never create ad-hoc worktrees in or around a
  repo's main checkout.
- One repo is enough. Never treat repo count as a reason to skip the forest,
  and never downgrade an explicit workforest request to a branch in the main
  checkout — the isolation, not the repo count, is what was asked for.
- If you have already started editing in a main checkout and a forest was
  wanted, say so and move the work: start the forest, carry the changes over
  (e.g. `git -C <main checkout> diff | git -C <tree> apply`), and restore the
  main checkout.
- Work from a tree directory for single-repo changes; work from the forest root
  when the change spans repos — relative paths like `api/src/...` then resolve
  naturally, and each subdirectory is a normal repo.
- For a sweep across every tree (status, each repo's tests, pushing), loop
  over the forest's directories, e.g.
  `for t in "$(workforest path <forest>)"/*/; do git -C "$t" push -u origin HEAD; done`.
- Deleting a forest leaves the branches alone unless `--delete-branches` is
  passed, so a burned forest can be started again on the same branch names.
- The manifest is plain TSV. If a tree gets out of sync (deleted by hand, say),
  `workforest status` shows it as `MISSING`, and `cut` cleans up the stale
  worktree registration.
