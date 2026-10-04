#!/usr/bin/env bash
# A repo for the case to work on: ./app, a git repo with one commit.
set -euo pipefail
git init --quiet --initial-branch=main app
printf '# app\n' > app/README.md
git -C app add README.md
git -C app -c user.name=eval -c user.email=eval@example.com commit --quiet --message init
