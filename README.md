# workforest

Isolated git worktrees for every piece of work. A **forest** is a directory
holding one git worktree ("tree") per repo involved in a piece of work, all on
one branch named after the forest. The repos' main checkouts are never touched,
so several pieces of work, or several coding agents, can be in flight at once.

```sh
workforest new auth-migration api web     # ~/.workforest/auth-migration/{api,web}, on branch auth-migration
cd "$(workforest path auth-migration)"
workforest status                         # what's dirty, what's ahead of its base
workforest exec -- git push -u origin HEAD
workforest rm auth-migration              # refuses while any work would be lost
```

A forest with a single tree is normal. Trees are `git worktree`s, so they share
their repo's object store: a tree costs its checked-out files, not a copy of
the history.

## Install

workforest needs `git` on your `PATH`. Releases are rolling: every commit on
`main` that passes CI is released to the `release` branch, which each install
method below follows.

With Nix:

```sh
nix profile add github:Nixotica/workforest/release   # older Nix: nix profile install
```

or as a flake input, `workforest.url = "github:Nixotica/workforest/release";`,
adding `workforest.packages.${system}.default` to your packages. Update with
`nix profile upgrade` or `nix flake update workforest`.

With Cargo:

```sh
cargo install --locked --git https://github.com/Nixotica/workforest --branch release
```

## The agent skill

workforest ships a skill that teaches coding agents when to plant a forest and
how to work in one. The skill doesn't need the CLI installed first: when it's
missing, the agent offers to install it.

As a Claude Code plugin, which follows the rolling releases:

```
/plugin marketplace add Nixotica/workforest
/plugin install workforest@workforest
```

Claude Code leaves auto-update off for third-party marketplaces. To receive
each release automatically, turn it on under `/plugin` → **Marketplaces** →
workforest → **Enable auto-update**; otherwise run
`/plugin marketplace update workforest` to update.

Without the plugin, link the copy the Nix package installs, so the skill
updates along with the CLI:

```sh
ln -s ~/.nix-profile/share/workforest/skills/workforest ~/.claude/skills/workforest
```

or fetch it from the `release` branch, which takes a re-run to update:

```sh
mkdir -p ~/.claude/skills/workforest
curl -fsSL https://raw.githubusercontent.com/Nixotica/workforest/release/plugin/skills/workforest/SKILL.md \
  -o ~/.claude/skills/workforest/SKILL.md
```

For other agents, put the skill wherever they read skills from.

## Usage

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

Inside a forest, commands act on that forest unless told otherwise. Every tree
gets the branch named after its forest unless you pass `--branch`, and branches
off `origin/HEAD` (else `main` or `master`) unless you pass `--base`. `prune`
and `rm` refuse to delete uncommitted changes or unpushed commits unless you
pass `--force`. Run `workforest help <command>` for the details.

Repos are named relative to `$WORKFOREST_REPOS` (default `~/repos`); an
argument containing `/` or starting with `.` is a path. Forests live under
`$WORKFOREST_ROOT` (default `~/.workforest`).

## Development

```sh
nix develop        # cargo, clippy, rustfmt, rust-analyzer and git
nix flake check    # tests, clippy, rustfmt and the package build, as CI runs them
```

When CI passes on a commit pushed to `main`, the release workflow moves the
`release` branch forward to it. Bump `version` in `Cargo.toml` when the
command-line interface changes: the first commit to pass CI with a new version
is tagged `v<version>` and gets a GitHub release.

## License

MIT
