# Contributing

## Setup

You need stable Rust, a C compiler (the tree-sitter grammars are C), and Git.

Or use the devcontainer, which has everything. With Docker running and Node.js
installed (for `npx`), start it from the repository root:

```bash
.devcontainer/up.sh
```

Then connect over SSH with this host `~/.ssh/config` entry. The key is
generated on the first start.

```
Host gitplume-dev
  HostName 127.0.0.1
  Port 2222
  User vscode
  IdentityFile <repo>/.devcontainer/.ssh/id_ed25519
  StrictHostKeyChecking no
  UserKnownHostsFile /dev/null
  LogLevel ERROR
```

You can also attach VS Code with "Dev Containers: Attach to Running
Container".

## Build and test

Run `cargo run` inside any Git repository. Before committing, run the quality
gates:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

[docs/design-spec.md](docs/design-spec.md) describes the design and how the
code is organized, and the testing rules are in [AGENTS.md](AGENTS.md). Commit
messages use [gitmoji](https://gitmoji.dev/).

## Releasing

Releases are built by `.github/workflows/release.yml`:

- **Release:** bump the version in `Cargo.toml` and merge to `main`. Then tag
  `vX.Y.Z` and push the tag. The build fails if the tag and the `Cargo.toml`
  version differ.
- **Beta:** every push to a branch other than `main` publishes a
  `vX.Y.Z-beta.N` prerelease. Install one with:

  ```bash
  curl -fsSL https://github.com/LBognanni/gitplume/releases/download/<tag>/install.sh | sh
  ```

To build the static Linux binary locally, as CI does (the devcontainer has the
musl tools):

```bash
CC=musl-gcc cargo build --release --target x86_64-unknown-linux-musl
```
