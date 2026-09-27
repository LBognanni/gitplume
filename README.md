# 🪶GitPlume

A fast terminal app for reviewing and staging your Git changes. See what you
changed, read it as a syntax-highlighted diff with the whole file around it,
and stage or discard it, without leaving the terminal.

- **Full-context diffs**: every change is shown inside the complete file,
  highlighted for 20 languages, and `n` / `p` jump between changes.
- **Stage, unstage, or discard** whole files, one at a time or checked in bulk.
  Discarding always asks first.
- **History**: browse the last 100 commits on your branch and open the diff of
  any file they changed.
- **Files**: browse every file in the repository, preview it with highlighting,
  or jump to one by name.
- **Live**: the status and history update on their own as you edit, commit, or
  switch branches.
- **Keyboard and mouse**: click, hover for row actions, drag dividers to
  resize panes, and select text to copy it.
- **One small binary** that starts instantly.

## Install

On Linux or macOS:

```bash
curl -fsSL https://github.com/LBognanni/gitplume/releases/latest/download/install.sh | sh
```

This installs `gitplume` to `~/.local/bin`. Set `GITPLUME_INSTALL_DIR` on `sh`
to install somewhere else.

You need Git and a [Nerd Font](https://www.nerdfonts.com/) in your terminal.
With a Rust toolchain, you can also install it from crates.io:
`cargo install --locked gitplume`.

gitplume used to be called gitpane. Upgrading? Delete the old `gitpane` binary
from `~/.local/bin`, or run `uv tool uninstall gitpane` if you had the Python
version.

## Usage

Run `gitplume` inside a Git repository.

The **Changes** tab lists staged files, unstaged files, and recent commits.
Select a file to see its diff, or a commit to see the files it changed. The
**Files** tab shows the repository below the directory you started in; select
a file to preview it. Press `h` at any time for the keyboard shortcuts.

## Controls

| Control | Action |
| --- | --- |
| Up/Down or `j` / `k` | Move the selection, or scroll the focused viewer |
| Left/Right | Collapse / expand a commit or folder, or scroll sideways |
| PageUp/PageDown, Home/End | Scroll the focused viewer by a page, or to the top / bottom |
| Tab / Shift+Tab | Move focus to the next / previous pane |
| Enter or click | Open a file's diff or preview, or expand / collapse a commit or folder |
| `1` / `2` or click a tab | Switch to the Changes / Files tab |
| `n` / `p` or click `↓` / `↑` above the diff | Next / previous change |
| `t` | Jump to a file by name (Files tab) |
| Space or click `[ ]` | Check or uncheck a file for a bulk action |
| `s` | Stage or unstage the focused file |
| `d` | Discard the focused unstaged file, after confirmation |
| Hover a file, click `↑` / `↓` / `↶` | Stage, unstage, or discard that file |
| Section-header `↑` / `↓` / `↶` | Stage, unstage, or discard all checked files |
| Drag a divider | Resize the panes |
| Drag across a viewer | Select text and copy it to the clipboard |
| Mouse wheel (Shift for sideways) | Scroll the list or viewer under the pointer |
| `r` | Refresh everything, including the Files tab |
| `w` | Toggle line wrapping |
| `h` / Esc | Show the shortcuts / close a popup |
| `q` or Ctrl+C | Quit |

## Limitations

gitplume stages whole files only: no hunk or line staging yet. It doesn't
make commits, resolve conflicts, handle renames, or preview binary files. The
Files tab only refreshes when you press `r`.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

GNU GPL v3; see [LICENSE](LICENSE).
