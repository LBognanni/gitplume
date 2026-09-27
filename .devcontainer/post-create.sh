#!/usr/bin/env bash
# Set up SSH access and the shell inside the container. Runs in the workspace folder.
set -euo pipefail

key=.devcontainer/.ssh/id_ed25519
if [ ! -f "$key" ]; then
  mkdir -p "$(dirname "$key")"
  ssh-keygen -q -t ed25519 -N '' -C gitplume-devcontainer -f "$key"
fi

mkdir -p ~/.ssh
chmod 700 ~/.ssh
cp "$key.pub" ~/.ssh/authorized_keys
chmod 600 ~/.ssh/authorized_keys

if ! grep -q 'gitplume-workspace' ~/.bashrc; then
  echo "[ -n \"\${SSH_CONNECTION:-}\" ] && cd '$PWD' # gitplume-workspace" >> ~/.bashrc
fi

# Fall back to a widely known terminal type when the client's is unknown here.
if ! grep -q 'gitplume-term' ~/.bashrc; then
  echo 'infocmp "$TERM" >/dev/null 2>&1 || export TERM=xterm-256color # gitplume-term' >> ~/.bashrc
fi

# SSH forwards the client's locale; fall back to C.UTF-8 when it is not installed.
if ! grep -q 'gitplume-locale' ~/.bashrc; then
  echo '[ "$(LC_ALL=${LANG:-C} locale charmap 2>/dev/null)" = UTF-8 ] || { unset $(env | grep -o "^LC_[A-Z]*"); export LANG=C.UTF-8; } # gitplume-locale' >> ~/.bashrc
fi

git config --global --add safe.directory '*'

# Docker creates a new cargo registry volume owned by root.
sudo chown vscode:rustlang /usr/local/cargo/registry

# Keep Claude Code's state across rebuilds in the mounted ~/.claude directory.
# A single-file bind mount breaks on atomic rewrites, so symlink instead.
ln -sfn ~/.claude/devcontainer.claude.json ~/.claude.json
