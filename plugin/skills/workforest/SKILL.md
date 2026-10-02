---
name: workforest
description: Manage "workforests" — collections of git worktrees under ~/forests, one per repo involved in a piece of work, so a feature is developed in isolation without ever touching the repos' main checkouts. Use whenever isolation is wanted, INCLUDING single-repo work — a forest with one tree is normal and expected. Use when a task spans two or more repos, when a feature should not disturb the main checkout, when several sessions or agents may work in parallel, when starting/switching/cleaning up a feature branch, or when the user says forest, graft, prune, burn, worktree, isolate, or asks to work on several repos at once.
---

# workforest

A **forest** is a directory under `~/forests/` holding one git worktree ("tree")
per repo involved in a piece of work. Each tree is a worktree of the repo's main
checkout in `~/repos/<name>`, checked out on a shared branch named after the
forest. The main checkouts are never modified — no branch switching, no
stashing — so several pieces of work can be in flight at once.

The unit of a forest is **a piece of work**, not a set of repos. One repo is a
perfectly normal forest. Isolation is the point; spanning repos is just
something forests also happen to do.

```
~/forests/auth-migration/
├── .workforest     manifest: repo \t source repo \t branch \t base ref
├── api/            worktree of ~/repos/api on branch auth-migration
└── web/            worktree of ~/repos/web on branch auth-migration
```

A tree costs only its checked-out files: worktrees share their repo's object
store, so no history is copied.

## Before first use

This skill drives the `workforest` command-line tool, version 0.1.0 or later.
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

## When to plant a forest

Plant one — and do the work inside it — whenever any of these hold,
**regardless of how many repos are involved**:

- The user asked for a forest, a worktree, or isolation, by any wording.
- The work is a named feature, fix, or experiment that wants its own branch.
- More than one session, agent, or terminal may touch the same repo.
- The change should not disturb whatever is currently checked out in the main
  checkout.

**A single repo never disqualifies a forest.** `workforest new my-feature myrepo`
creates a one-tree forest and is a first-class, expected use. If the user asks
to use workforest and the work touches one repo, plant the forest anyway — do
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
| `workforest new <forest> [repo...]` | plant a forest, optionally grafting repos right away |
| `workforest graft <repo>...` | add worktrees to a forest |
| `workforest prune <repo>...` | remove worktrees from a forest |
| `workforest rm <forest>` | remove a forest and every tree in it |
| `workforest ls [forest]` | list forests, or the trees in one |
| `workforest status [forest]` | per-tree branch, clean/dirty, ahead/behind its base |
| `workforest path [forest]` | print a forest's path |
| `workforest exec [--] <cmd>...` | run a command in every tree, one tree at a time |

Aliases: `plant`=`new`, `add`=`graft`, `remove`=`prune`, `burn`/`delete`=`rm`,
`list`=`ls`, `st`=`status`, `dir`=`path`, `each`=`exec`.

Options:

- `-f, --forest <name>` (`graft`, `prune`, `exec`) — target forest. Defaults to
  the forest containing the current directory, so inside a tree you can omit
  it. `status` and `path` take the forest as an optional argument with the same
  default.
- `-b, --branch <name>` — branch to check out or create. Defaults to the forest
  name, giving every repo the same branch name. If the branch already exists in
  a repo, it is checked out rather than recreated.
- `-B, --base <ref>` — what to branch off. Defaults to `origin/HEAD`, falling
  back to a local `main` or `master`.
- `--force` — on `prune`/`rm`, skip the safety checks for uncommitted changes
  and unpushed commits.
- `--delete-branches` — on `prune`/`rm`, also delete the trees' branches.

Repo arguments are names relative to `~/repos`; anything containing `/` or
starting with `.` is treated as a path, so repos elsewhere can be grafted too.

Environment: `WORKFOREST_ROOT` (default `~/forests`) and `WORKFOREST_REPOS`
(default `~/repos`). Where this skill says `~/forests` or `~/repos`, read those
values if they are set.

## Typical flow

```sh
workforest new fix-login api                  # single-repo forest — a normal case
cd "$(workforest path fix-login)/api"
# ...edit, commit, push, merge, then...
workforest rm fix-login
```

Multi-repo is the same commands with more arguments:

```sh
workforest new auth-migration api web         # both trees on branch auth-migration
cd "$(workforest path auth-migration)"
# ...edit across api/ and web/...
workforest graft docs                         # a third repo turned out to be involved
workforest status                             # what's dirty, what's ahead
workforest exec -- git push -u origin HEAD    # push every tree
workforest rm auth-migration --delete-branches
```

## Burn the forest once its work has landed

A forest is scaffolding for one piece of work. When that work is merged, burn
it: a forest left standing keeps a checkout on disk, and `workforest ls` stops
being a picture of what is really in flight.

`rm` refuses to remove a tree with uncommitted changes, unpushed commits, or
commits ahead of its base. That refusal means the work has not actually landed
— say what would be lost and ask; do not reach for `--force` on the user's
behalf.

### Landing a squash-merged branch

A squash merge is never an ancestor of the branch it came from. A freshly
planted branch tracks its base, so once the work is squashed onto that base,
`rm` still counts the branch's commits as not landed and refuses. Order the
landing so the refusal never comes up:

1. Push with `git push -u origin HEAD`, so the branch tracks its *own* remote
   branch rather than the base it was planted from.
2. Merge without deleting the remote branch (for example
   `gh pr merge <n> --squash`, without `--delete-branch`).
3. Fetch, and confirm the work is on the base, path by path:
   `git diff --quiet origin/<base> HEAD -- <changed files>`.
4. `workforest rm <forest> --delete-branches`.
5. Only then delete the remote branch: `git push origin --delete <branch>`.
   Deleting it before the burn takes the upstream away and brings the refusal
   back.

If the remote branch is already gone — auto-deleted on merge — the refusal is
expected; show the step 3 diff and ask before using `--force`.

## Guidance for Claude

- Plant the forest **before** the first edit, whenever the "When to plant a
  forest" rules above apply. Never create ad-hoc worktrees in or around the
  repos directory.
- One repo is enough. Never treat repo count as a reason to skip the forest,
  and never downgrade an explicit workforest request to a branch in the main
  checkout — the isolation, not the repo count, is what was asked for.
- If you have already started editing in a main checkout and a forest was
  wanted, say so and move the work: plant the forest, carry the changes over
  (e.g. `git -C ~/repos/<name> diff | git -C <tree> apply`), and restore the
  main checkout.
- Work from a tree directory for single-repo changes; work from the forest root
  when the change spans repos — relative paths like `api/src/...` then resolve
  naturally, and each subdirectory is a normal repo.
- `workforest exec` is the right tool for cross-repo sweeps (`git status`,
  running each repo's tests, pushing). It runs one tree at a time and returns
  the last non-zero exit code.
- Deleting a forest leaves the branches alone unless `--delete-branches` is
  passed, so a burned forest can be replanted on the same branch names.
- The manifest is plain TSV. If a tree gets out of sync (deleted by hand, say),
  `workforest status` shows it as `MISSING`, and `prune` cleans up the stale
  worktree registration.
