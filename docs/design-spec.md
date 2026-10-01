# gitplume design

## The application

gitplume is a terminal app for reviewing and staging working-tree changes. It
answers "what did I change, and what do I want in my next commit?" without
leaving the terminal, and stays fast on large files and large diffs.

### Screens

```
 Changes  Files  Find                                 ← tabs
 Staged                        ↓│ src/app.rs     ↑ ↓   ← title rows
┌─────────────────────────────┐ │   12   12 fn main() {
│[ ] M src/app.rs             │ │ -   13      old();
└─────────────────────────────┘ │ +        13 new();
────────────────────────────────  (splitter)
 Unstaged                    ↑ ↶│
┌─────────────────────────────┐ │
│[ ] ? notes.txt              │ │
└─────────────────────────────┘ │
────────────────────────────────
 Commits                        │
┌─────────────────────────────┐ │
│▶ ✨ add thing abc1234       │ │
└─────────────────────────────┘ │
 Branch: main                                        ← status bar
```

- **Changes tab:**
  - A sidebar with three sections: Staged, Unstaged, and Commits (the last
    100 on the branch). A diff pane sits beside it.
  - Selecting a file shows its diff with the whole file as context. Selecting
    a commit expands its changed files; selecting one of those shows its
    historical diff.
  - Files can be staged, unstaged, or discarded one at a time (keys, or buttons
    revealed on hover) or checked and handled in bulk. Discarding always asks
    for confirmation.
  - Renames, copies, and conflicts are listed with the reason they aren't
    supported, and are excluded from every action.
- **Files tab:** a tree of the tracked and non-ignored untracked files below
  the launch directory, and a highlighted preview of the selected file. `t`
  opens a jump-to-file dialog.
- **Find tab** (Ctrl+F): a Find box and a comma-separated glob Filter box
  (e.g. `*.rs, src/**`) above a scrollable list of matches, one row per
  match. The filter is matched in memory against the file list; only the
  files it selects are ever handed to `git grep`, so a search never opens
  every file. A search needs at least three characters. Clicking a match, or
  pressing Enter on the highlighted one, opens it in the shared preview pane
  scrolled to that line.
- **Status bar:** the current branch, or the hint for the hovered button.
- **Modals:** discard confirmation, keyboard shortcuts (shown once on first
  launch), and file jump.
- **Toasts:** errors and warnings stack at the bottom right and expire after
  five seconds.

Every divider can be dragged to resize the panes. Everything works with the
keyboard and the mouse; text selected in a viewer is copied to the clipboard
through the terminal.

### Principles

- **The UI never waits.** Git and file work run on background threads; the
  screen stays responsive while they load.
- **Work is bounded by the viewport.** Only visible rows are built,
  highlighted, and drawn, whether a document has a hundred rows or a hundred
  thousand.
- **Git is the source of truth.** gitplume runs the `git` CLI, so it respects
  the user's configuration, and it never caches repository state beyond what
  is on screen.
- **It stays in sync on its own.** A file watcher refreshes status and history
  as the repository changes, without disturbing selection, focus, or scroll.
- **Simple over clever.** No widget framework, no async runtime, no plugin or
  configuration system.

### Out of scope

Hunk or line staging, commits, conflict resolution, renames, binary previews,
side-by-side diffs, persisting pane sizes, and language injections (such as
SQL inside strings).

## Design decisions

| Decision | Choice | Why |
| --- | --- | --- |
| TUI | ratatui and crossterm, no widget framework | The layout is fixed: a few panes and three modals. A small in-house focus and hit-test layer is simpler than a framework. |
| Highlighting | tree-sitter only, queried per visible line | Parse the whole file once on a worker, then run highlight queries only for the rows on screen. Top-down highlighters must process a file from line 1 to reach its end. |
| Languages | 20 bundled grammars, plain text otherwise | Every grammar is a compiled crate; coverage is traded for speed and simplicity. |
| Git access | The `git` CLI | Respects user configuration and keeps adapter tests meaningful. |
| Diff cache | None | Parsing is cheap and highlighting is per viewport. A background reload that produces an equal patch keeps the current document. |
| Concurrency | Threads and `std::sync::mpsc`, no async runtime | A handful of long-lived workers is all the app needs. |
| Tooltips | Hints in the status bar | Floating tooltips need timers and overlay placement. |
| Clipboard | OSC 52 | Works in modern terminals and over SSH, with no system clipboard dependency. |
| Distribution | Static binaries on GitHub Releases and a `curl \| sh` installer | Users need no toolchain. See `CONTRIBUTING.md`. |

## Codebase

A single crate. `main.rs` builds the binary; `lib.rs` exposes the modules so
the tests in `tests/` can drive the app.

### Modules

| Module | Responsibility |
| --- | --- |
| `main.rs` | Handle `--version`, find the repository, set up and restore the terminal (also on panic), and start the loop |
| `model.rs` | Plain data: file entries (side, status, path, and an optional reason they are unsupported), commits, commit files, repository state, and the entry a diff is shown for |
| `git.rs` | The Git adapter: a command runner, the `GitApi` trait, and its CLI implementation with the porcelain parsers |
| `app.rs` | All application state, the `Event`, `Job`, and `Effect` types, and `App::update` |
| `runtime.rs` | The main loop, worker threads, effect execution, and the first-launch marker |
| `ui.rs` | Rendering every screen, and recording the hit map for mouse routing |
| `code_view.rs` | The virtual text viewer shared by the diff and preview panes |
| `diff.rs` | The unified-diff parser |
| `document.rs` | Diff and preview documents: gutter text, source lines, and their highlighting |
| `highlight.rs` | Language detection, tree-sitter parsing, highlight queries, and the capture palette |
| `layout.rs` | Splitter weights and pane sizing |
| `watcher.rs` | The file watcher and the classification of changed paths |
| `icons.rs` | Nerd Font icons for the Files tree, by file name and extension |
| `theme.rs` | Color tokens |

### Event loop

The app follows an update/render split:

1. Every input arrives as an `Event` on one channel. Terminal input comes from
   an input thread, work results from the workers, and watcher messages from
   the watcher.
2. `App::update` applies one event and returns `Effect`s: jobs to run, text to
   copy, or quit. It is the only place state changes. It never blocks and
   never calls Git.
3. The runtime executes the effects by sending jobs to workers.
4. After draining every queued event, the runtime draws **once**, so bursts
   such as mouse motion cost a single frame.

`ui::render` draws the frame and records a hit map: the rectangles of panes,
rows, buttons, splitters, scrollbars, and modal controls. The next mouse
event goes to the topmost target under the pointer.

### Background work

- **Git queue:** one thread runs status reads and mutations strictly in
  order. A mutation and the status read that follows it run as one job, so
  no other read can interleave.
- **Latest-only workers:** one thread each for diffs, history, commit files,
  the file list, previews, and find-in-files searches. Each skips to the
  newest queued request, so fast navigation never piles up work. Diff and
  preview documents, including their tree-sitter parse, are built here.
- **Tokens:** every request carries a token. The app drops a result whose
  token is no longer current. Workers are never cancelled, and quitting never
  waits for them.

### Git adapter

`GitApi` is the boundary between the app and Git. `CliGit` implements it on
top of a `Runner` that executes one Git command. Every command:

- runs with `--literal-pathspecs` and `GIT_OPTIONAL_LOCKS=0`, so it never
  takes locks that would disturb the user's own Git commands;
- reads machine output: porcelain v2 status and NUL-separated lists.

Diffs request a context large enough to cover the whole file. Untracked files
are diffed against `/dev/null`, so everything downstream is a unified diff.
Git's stderr becomes the user-facing error text.

### Diff and highlighting pipeline

1. **Parse:** the unified diff becomes a flat list of rows: old and new line
   numbers, text, and kind (context, added, removed). Change positions are the
   first row of each block of added or removed rows.
2. **Sides:** the new side is the context and added lines in order; the old
   side is the context and removed lines. Each side is parsed with the
   file's tree-sitter grammar once, on the worker.
3. **Highlight on draw:** for each visible row, the language's highlight query
   runs on that line's byte range only. Removed rows use the old side's tree;
   the rest use the new side's. Where captures overlap, the innermost wins. A
   capture maps to a palette color by its longest dotted prefix.
4. **Document:** each row gets a gutter (marker and line numbers, sized for the
   largest number) that never takes syntax color. Added and removed rows get a
   full-width background.

Language detection tries the exact file name, then the extension, then a
shebang. Previews use the same machinery with one side and a line-number
gutter. They refuse non-regular, binary, non-UTF-8, and larger than 1 MiB
files, with a message instead.

### CodeView

One component serves both viewers:

- It materializes only visible rows.
- Tabs expand to 8-column stops, and it crops by terminal cell width, so wide
  characters are never split.
- Unwrapped, it scrolls both ways. Wrapped, rows break at word boundaries with
  an indent past the gutter, and the position is kept across width changes.
- It has scrollbars with click-to-jump and thumb dragging, wheel scrolling,
  and keyboard navigation.
- Selection works on the displayed text. Releasing the button copies the
  selection through OSC 52.

### Layout

Each tab has splitter groups: the sidebar and diff pane, the three sidebar
sections, and the Files tree and preview. Every pane has a minimum size.

- Before any drag, the sidebar and Files tree keep a fixed width.
- A drag freezes the group's current sizes as proportional weights and moves
  only the two panes next to the splitter.
- After a drag, terminal resizes scale the panes by those weights.
- When the terminal is too small for every minimum, panes shrink in proportion
  and never panic.

### Refresh and consistency

These rules keep the screen stable while the repository changes underneath it.

- **Manual refresh** (startup, `r`, after a mutation): shows loading states,
  rebuilds the lists, focuses the first entry, and clears the diff pane. At
  startup with nothing to stage, the first commit is selected and expanded.
- **Watcher:** it watches the worktree and the Git directories, debounced, and
  classifies each changed path:
  - the index changes status;
  - `HEAD` and refs change status and history;
  - any other worktree path changes status, and records the path.
- **Automatic refresh:** the watcher's invalidations are merged, and a burst
  becomes at most one follow-up read. An automatic read uses the current token
  without advancing it, so it never overrides a manual refresh. Its result is
  applied quietly:
  - no loading states;
  - highlights, checks, and focus are kept by side and path;
  - an equal state changes nothing.
- **Open diff:** after a quiet apply, the open working-tree diff reloads only
  if it may have changed, keeping its scroll and change position. An equal
  patch leaves the document untouched. An entry that disappeared shows a
  message instead of jumping elsewhere.
- **History:** an equal commit list keeps the tree exactly as it is, with
  expanded commits, loaded files, and cursor. A changed list rebuilds the tree
  and puts the cursor back on the same commit.
- **Failures:** manual failures show an error toast. Automatic failures warn
  once per failure streak and keep what is on screen. `r` always works.

### Theme

A dark palette of named tokens (`theme.rs`), with the Monokai background and
syntax colors in the viewers. Tests compare rendered cells, never color values.

### Tests

- **Next to the code:** unit and component tests in `#[cfg(test)]` modules.
  - The Git adapter tests use a recording runner and assert command arguments
    and parsed results.
  - Parsers assert their returned values.
  - The CodeView and document tests render into a `TestBackend`.
- **In `tests/*.rs`:** workflow tests, organized by product behavior. The
  harness in `tests/common/mod.rs` runs the app against a `FakeGit` with
  realistic data and executes effects synchronously. It can hold jobs, so
  tests decide the order results arrive in. Tests send key, mouse, and resize
  events, then assert the rendered screen, focus, copied text, and the Git
  calls made.

The rules for writing tests are in `AGENTS.md`.
