#!/bin/sh
# Install the locally built release binary (run `cargo build --release` first).
# GITPLUME_INSTALL_DIR picks the directory (default: ~/.local/bin).
set -eu

bin="$(cd "$(dirname "$0")" && pwd)/target/release/gitplume"
if [ ! -x "$bin" ]; then
    echo "gitplume: $bin not found; run cargo build --release first." >&2
    exit 1
fi

dir="${GITPLUME_INSTALL_DIR:-$HOME/.local/bin}"
mkdir -p "$dir"
# Copy next to the destination so the final mv is an atomic rename,
# which also works while gitplume is running.
tmp="$(mktemp "$dir/.gitplume.XXXXXX")"
trap 'rm -f "$tmp"' EXIT
cp "$bin" "$tmp"
chmod 755 "$tmp"
mv "$tmp" "$dir/gitplume"

echo "Installed $("$dir/gitplume" --version) to $dir"
