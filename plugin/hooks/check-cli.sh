#!/bin/sh
# SessionStart hook: when the workforest CLI is missing or older than the skill
# needs, warn the user and have Claude offer the install in its first reply.
# When the CLI is fine, print nothing.
#
# This runs at every session start, so it stays a quick local check: it runs
# `workforest --version` and nothing else. No network, nothing that can block.

# The oldest CLI the skill works with. Keep in step with "Before first use" in
# skills/workforest/SKILL.md.
required=0.13.0

# warn <message for the user> <context for Claude> <install|update|replace>
#
# The strings go into JSON unescaped, so they must not contain " or \.
warn() {
  context="$2 In your first reply, whatever the user asked, tell them this and offer to $3 it, using the options in the 'Before first use' section of the workforest skill. Install nothing until the user agrees, then carry on with what they asked."
  printf '{"systemMessage":"workforest plugin: %s","hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"%s"}}\n' \
    "$1" "$context"
  exit 0
}

if ! command -v workforest >/dev/null 2>&1; then
  warn "the workforest CLI isn't installed; ask Claude to set it up." \
    "The workforest plugin is installed, but the workforest CLI that its skill drives is not on PATH, so the skill can't start forests yet." \
    install
fi

# `workforest 0.1.0`, or `workforest 0.1.0 (<commit>)` from a Nix build. An
# older tool that rejects --version, or another program of the same name,
# reports no version.
version=$(workforest --version </dev/null 2>/dev/null |
  sed -n '1s/^workforest \([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\).*/\1/p')

if [ -z "$version" ]; then
  warn "the workforest on PATH isn't the CLI this plugin needs ($required or later); ask Claude to set it up." \
    "The workforest plugin needs the workforest CLI $required or later, but the workforest on PATH doesn't report a version: it is an older tool or a different program. Find out how it was installed before replacing it." \
    replace
fi

if ! awk -v have="$version" -v need="$required" 'BEGIN {
  split(have, h, "."); split(need, n, ".")
  for (i = 1; i <= 3; i++) if (h[i] + 0 != n[i] + 0) exit h[i] + 0 < n[i] + 0
}'; then
  warn "workforest $version is older than the $required this plugin needs; ask Claude to update it." \
    "The workforest plugin needs the workforest CLI $required or later, but workforest $version is on PATH." \
    update
fi
