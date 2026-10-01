#!/usr/bin/env bash
# Start the devcontainer with the main repository root as the workspace folder,
# then register its SSH key in the local ~/.ssh/config (host: gitplume-dev).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
common="$(git -C "$here" rev-parse --path-format=absolute --git-common-dir)"
root="${common%/.git}"
npx -y @devcontainers/cli up \
  --workspace-folder "$root" \
  --config "$here/devcontainer.json" "$@"

# post-create.sh generates the key in the bind-mounted workspace.
mkdir -p ~/.ssh
chmod 700 ~/.ssh
if grep -q '^Host gitplume-dev$' ~/.ssh/config 2>/dev/null; then
  sed -i "/^Host gitplume-dev\$/,/^Host / s|^  IdentityFile .*|  IdentityFile $root/.devcontainer/.ssh/id_ed25519|" ~/.ssh/config
else
  cat >> ~/.ssh/config <<CONF

Host gitplume-dev
  HostName 127.0.0.1
  Port 2222
  User vscode
  IdentityFile $root/.devcontainer/.ssh/id_ed25519
  IdentitiesOnly yes
  StrictHostKeyChecking no
  UserKnownHostsFile /dev/null
CONF
  chmod 600 ~/.ssh/config
  echo "Added Host gitplume-dev to ~/.ssh/config"
fi
