#!/usr/bin/env bash
# Exercise an installed workforest the way a user would: start a forest in a
# throwaway set of repos, check that unlanded work blocks the burn, land it,
# and burn the forest. Also checks that `wf` runs the same binary, that the
# package ships the skill, and that the plugin's session-start hook accepts the
# installed CLI.
#
# usage: ci/smoke.sh <prefix>   where <prefix>/bin/workforest is the install
set -euo pipefail

prefix=$(cd "${1:?usage: smoke.sh <install prefix>}" && pwd)
workforest="$prefix/bin/workforest"
plugin="$(cd "$(dirname "$0")/.." && pwd)/plugin"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
export HOME="$tmp/home" WORKFOREST_ROOT="$tmp/forests"
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL="$tmp/home/.gitconfig"
export GIT_AUTHOR_NAME=smoke GIT_AUTHOR_EMAIL=smoke@example.com
export GIT_COMMITTER_NAME=smoke GIT_COMMITTER_EMAIL=smoke@example.com
mkdir -p "$HOME" "$tmp/repos"

git init --quiet --initial-branch=main "$tmp/seed"
git -C "$tmp/seed" commit --quiet --allow-empty --message initial
git clone --quiet --bare "$tmp/seed" "$tmp/origin.git"
git clone --quiet "$tmp/origin.git" "$tmp/repos/demo"

"$workforest" --version
test "$("$prefix/bin/wf" --version)" = "$("$workforest" --version)"
"$workforest" new smoke "$tmp/repos/demo"
tree="$("$workforest" path smoke)/demo"
echo change >"$tree/file"
"$workforest" status smoke | grep --quiet dirty
if "$workforest" burn smoke; then
  echo "rm burned a forest with uncommitted work" >&2
  exit 1
fi
git -C "$tree" add file
git -C "$tree" commit --quiet --message change
git -C "$tree" push --quiet --set-upstream origin HEAD
"$workforest" burn smoke --delete-branches
test ! -e "$WORKFOREST_ROOT/smoke"

cmp "$prefix/share/workforest/skills/workforest/SKILL.md" "$plugin/skills/workforest/SKILL.md"
hook=$(PATH="$prefix/bin:$PATH" sh "$plugin/hooks/check-cli.sh")
if [ -n "$hook" ]; then
  echo "the session-start hook rejected the installed CLI: $hook" >&2
  exit 1
fi
echo "smoke test passed"
