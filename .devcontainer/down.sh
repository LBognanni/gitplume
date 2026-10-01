#!/usr/bin/env bash
# Stop the devcontainer started by up.sh. The devcontainer CLI has no "down",
# so find the container by the label it sets on the workspace folder.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
common="$(git -C "$here" rev-parse --path-format=absolute --git-common-dir)"
root="${common%/.git}"
ids="$(docker ps -q --filter "label=devcontainer.local_folder=$root")"
if [ -z "$ids" ]; then
  echo "No running devcontainer for $root"
  exit 0
fi
docker stop $ids
