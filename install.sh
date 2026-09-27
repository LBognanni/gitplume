#!/bin/sh
# Install gitplume from GitHub Releases:
#   curl -fsSL https://github.com/LBognanni/gitplume/releases/latest/download/install.sh | sh
# GITPLUME_VERSION picks a release tag (default: the release this script was
# downloaded from; latest when run from the repository).
# GITPLUME_INSTALL_DIR picks the directory (default: ~/.local/bin).
set -eu

main() {
    case "$(uname -s) $(uname -m)" in
        "Linux x86_64") target=x86_64-unknown-linux-musl ;;
        "Linux aarch64" | "Linux arm64") target=aarch64-unknown-linux-musl ;;
        "Darwin x86_64") target=x86_64-apple-darwin ;;
        "Darwin arm64") target=aarch64-apple-darwin ;;
        *) echo "gitplume: unsupported platform: $(uname -sm)" >&2; exit 1 ;;
    esac

    version="${GITPLUME_VERSION:-latest}"
    if [ "$version" = latest ]; then
        base=https://github.com/LBognanni/gitplume/releases/latest/download
    else
        base="https://github.com/LBognanni/gitplume/releases/download/$version"
    fi

    dir="${GITPLUME_INSTALL_DIR:-$HOME/.local/bin}"
    mkdir -p "$dir"
    # Extract next to the destination so the final mv is an atomic rename,
    # which also works while gitplume is running.
    tmp="$(mktemp -d "$dir/.gitplume.XXXXXX")"
    trap 'rm -rf "$tmp"' EXIT
    curl -fsSL -o "$tmp/gitplume.tar.gz" "$base/gitplume-$target.tar.gz"
    tar -xzf "$tmp/gitplume.tar.gz" -C "$tmp"
    mv "$tmp/gitplume" "$dir/gitplume"

    echo "Installed $("$dir/gitplume" --version) to $dir"
    case ":$PATH:" in
        *":$dir:"*) ;;
        *) echo "Add $dir to your PATH to run gitplume." ;;
    esac
}

main "$@"
