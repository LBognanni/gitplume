use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{
    Event as Input, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::layout::{Position, Rect};

use crate::code_view::{CodeView, Press, Scrollbar};
use crate::document::Document;
use crate::find;
use crate::git::GitError;
use crate::layout::{Panes, Splitter};
use crate::model::{Commit, CommitFile, DiffEntry, FileEntry, GrepMatch, RepoState, Side};
use crate::watcher::{Invalidation, Watch};

const TOAST_LIFETIME: Duration = Duration::from_secs(5);
/// Most file jump results listed.
pub const MAX_FILE_JUMP_RESULTS: usize = 100;
/// Most find-in-files matches listed, in total across every file.
pub const MAX_FIND_RESULTS: usize = 500;
/// Find query length below which no search runs.
pub const MIN_FIND_QUERY: usize = 3;
/// Context rows kept above a change when scrolling to it.
const CHANGE_CONTEXT: usize = 4;
/// Rows or columns scrolled per mouse wheel step in lists and trees.
const WHEEL: usize = 3;
/// Shown when the entry of the open diff disappears from its side.
pub const MISSING: &str = "The selected change is no longer present.";

/// Automatic work whose failures warn once per streak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Streak {
    Status,
    History,
    Diff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Changes,
    Files,
    Find,
}

/// A focusable pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Staged,
    Unstaged,
    Commits,
    Diff,
    FilesTree,
    /// The Files or Find tab's preview, shared between both.
    Preview,
    FindQuery,
    FindFilter,
    FindResults,
}

const CHANGES_ORDER: [Focus; 4] = [Focus::Staged, Focus::Unstaged, Focus::Commits, Focus::Diff];
const FILES_ORDER: [Focus; 2] = [Focus::FilesTree, Focus::Preview];
const FIND_ORDER: [Focus; 4] = [
    Focus::FindQuery,
    Focus::FindFilter,
    Focus::FindResults,
    Focus::Preview,
];

/// A Git mutation on status entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Stage,
    Unstage,
    Discard,
}

impl Action {
    /// The action as named in `Could not <action>`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Stage => "stage",
            Self::Unstage => "unstage",
            Self::Discard => "discard changes",
        }
    }

    /// The row action that moves an entry off `side`.
    fn toggle(side: Side) -> Self {
        match side {
            Side::Staged => Self::Unstage,
            Side::Unstaged => Self::Stage,
        }
    }
}

/// An icon button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    PreviousChange,
    NextChange,
    /// A row action on one entry of a status list.
    Row(Side, usize, Action),
    /// A title-bar action on the checked entries of a status list.
    Bulk(Action),
}

impl Button {
    pub fn hint(self) -> &'static str {
        match self {
            Self::PreviousChange => "Previous Change",
            Self::NextChange => "Next Change",
            Self::Row(_, _, Action::Stage) => "Stage Changes",
            Self::Row(_, _, Action::Unstage) => "Unstage Changes",
            Self::Row(_, _, Action::Discard) => "Discard Changes",
            Self::Bulk(Action::Stage) => "Stage Selected Changes",
            Self::Bulk(Action::Unstage) => "Unstage Selected Changes",
            Self::Bulk(Action::Discard) => "Discard Selected Changes",
        }
    }
}

/// Something the pointer can hit, recorded by `ui::render`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Tab(Tab),
    Pane(Focus),
    Button(Button),
    /// A status list row.
    Row(Side, usize),
    /// A visible commit tree row.
    TreeRow(usize),
    /// A visible Files tree row.
    FileRow(usize),
    /// A visible Find-in-files result row.
    FindResult(usize),
    /// The file jump dialog; swallows clicks.
    JumpDialog,
    /// A file jump result.
    JumpResult(usize),
    /// The `[ ]` columns of a supported status list row.
    Checkbox(Side, usize),
    /// The dimmed area behind a modal; swallows input.
    Backdrop,
    CloseShortcuts,
    CancelDiscard,
    ConfirmDiscard,
    Toast(u64),
    Splitter(Splitter),
    /// A list or tree scrollbar.
    Scrollbar(Focus, Scrollbar),
    /// The file jump results scrollbar.
    JumpScrollbar(Scrollbar),
}

impl Target {
    /// The list or tree pane this target lies in, if any.
    fn list_pane(self) -> Option<Focus> {
        let side = |side| match side {
            Side::Staged => Focus::Staged,
            Side::Unstaged => Focus::Unstaged,
        };
        match self {
            Self::Pane(
                focus @ (Focus::Staged
                | Focus::Unstaged
                | Focus::Commits
                | Focus::FilesTree
                | Focus::FindResults),
            ) => Some(focus),
            Self::Row(s, _) | Self::Checkbox(s, _) | Self::Button(Button::Row(s, _, _)) => {
                Some(side(s))
            }
            Self::Scrollbar(focus, _) => Some(focus),
            Self::TreeRow(_) => Some(Focus::Commits),
            Self::FileRow(_) => Some(Focus::FilesTree),
            Self::FindResult(_) => Some(Focus::FindResults),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Modal {
    Shortcuts,
    /// Discard confirmation for `entries`; `confirm` is true when Discard has focus.
    Discard {
        entries: Vec<FileEntry>,
        confirm: bool,
    },
    /// File jump: the query, and the highlighted result when the results have focus.
    FileJump {
        query: String,
        selected: Option<usize>,
    },
}

/// The first `MAX_FILE_JUMP_RESULTS` paths containing `query` case-insensitively,
/// and whether more exist. `files` pairs each path with its lowercase form.
pub fn matching_files(files: &[(String, String)], query: &str) -> (Vec<String>, bool) {
    if query.chars().count() < 3 {
        return (Vec::new(), false);
    }
    let needle = query.to_lowercase();
    let mut matches: Vec<String> = files
        .iter()
        .filter(|(_, lower)| lower.contains(&needle))
        .take(MAX_FILE_JUMP_RESULTS + 1)
        .map(|(path, _)| path.clone())
        .collect();
    let truncated = matches.len() > MAX_FILE_JUMP_RESULTS;
    matches.truncate(MAX_FILE_JUMP_RESULTS);
    (matches, truncated)
}

/// A status list: its entries, checkboxes, and highlighted row.
#[derive(Debug, Default)]
pub struct StatusList {
    pub entries: Vec<FileEntry>,
    pub checked: Vec<bool>,
    pub highlight: Option<usize>,
    /// First visible row, kept by `ui::render` so the highlight stays visible.
    pub offset: usize,
    /// Highlight last scrolled into view; the wheel scrolls freely until it moves.
    pub followed: Option<usize>,
}

impl StatusList {
    fn new(entries: Vec<FileEntry>) -> Self {
        Self {
            checked: vec![false; entries.len()],
            entries,
            highlight: None,
            offset: 0,
            followed: None,
        }
    }

    pub fn any_checked(&self) -> bool {
        self.checked.contains(&true)
    }

    fn checked_entries(&self) -> Vec<FileEntry> {
        self.entries
            .iter()
            .zip(&self.checked)
            .filter(|(_, checked)| **checked)
            .map(|(entry, _)| entry.clone())
            .collect()
    }

    fn toggle(&mut self, index: usize) {
        if self
            .entries
            .get(index)
            .is_some_and(|e| e.unsupported_reason.is_none())
        {
            self.checked[index] = !self.checked[index];
        }
    }

    fn highlighted(&self) -> Option<&FileEntry> {
        self.highlight.and_then(|index| self.entries.get(index))
    }

    fn step(&mut self, down: bool) {
        let Some(last) = self.entries.len().checked_sub(1) else {
            return;
        };
        self.highlight = Some(match (self.highlight, down) {
            (None, _) => 0,
            (Some(index), true) => (index + 1).min(last),
            (Some(index), false) => index.saturating_sub(1),
        });
    }
}

/// A visible row of the commit tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeRow {
    Commit(usize),
    /// File `.1` of commit `.0`.
    File(usize, usize),
    /// The inert `(no changed files)` leaf of a commit.
    Empty(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitNode {
    pub commit: Commit,
    pub expanded: bool,
    /// Loaded files; `None` until the commit is first expanded.
    pub files: Option<Vec<CommitFile>>,
}

/// The commit tree: commits with lazily loaded files, and a cursor row.
#[derive(Debug, Default)]
pub struct CommitTree {
    pub nodes: Vec<CommitNode>,
    pub cursor: Option<usize>,
    /// First visible row, kept by `ui::render` so the cursor stays visible.
    pub offset: usize,
    /// First visible cell column.
    pub scroll_x: usize,
    /// Cursor row last scrolled into view; the wheel scrolls freely until it moves.
    pub followed: Option<usize>,
}

impl CommitTree {
    pub fn rows(&self) -> Vec<TreeRow> {
        let mut rows = Vec::new();
        for (index, node) in self.nodes.iter().enumerate() {
            rows.push(TreeRow::Commit(index));
            if let (true, Some(files)) = (node.expanded, &node.files) {
                if files.is_empty() {
                    rows.push(TreeRow::Empty(index));
                }
                rows.extend((0..files.len()).map(|file| TreeRow::File(index, file)));
            }
        }
        rows
    }

    fn cursor_row(&self) -> Option<TreeRow> {
        self.cursor.and_then(|row| self.rows().get(row).copied())
    }

    /// Hash of the commit the cursor is on, or of the parent commit of its file.
    fn cursor_hash(&self) -> Option<String> {
        let (TreeRow::Commit(index) | TreeRow::File(index, _) | TreeRow::Empty(index)) =
            self.cursor_row()?;
        Some(self.nodes[index].commit.hash.clone())
    }

    fn step(&mut self, down: bool) {
        let Some(last) = self.rows().len().checked_sub(1) else {
            return;
        };
        self.cursor = Some(match (self.cursor, down) {
            (None, _) => 0,
            (Some(row), true) => (row + 1).min(last),
            (Some(row), false) => row.saturating_sub(1),
        });
    }
}

/// A Files tree node, stored in display (preorder) order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileNode {
    /// Name shown in the tree; the root shows the launch directory.
    pub name: String,
    /// Path relative to the launch directory; empty for the root.
    pub path: String,
    pub depth: usize,
    pub dir: bool,
    pub expanded: bool,
}

/// The Files tree: the launch directory with nested folders and files.
#[derive(Debug, Default)]
pub struct FileTree {
    pub nodes: Vec<FileNode>,
    /// Node under the cursor.
    pub cursor: Option<usize>,
    /// First visible row, kept by `ui::render` so the cursor stays visible.
    pub offset: usize,
    /// First visible cell column.
    pub scroll_x: usize,
    /// Cursor row last scrolled into view; the wheel scrolls freely until it moves.
    pub followed: Option<usize>,
}

impl FileTree {
    /// Build the tree under `root_name` from `/`-separated `files`:
    /// subdirectories first, then files, each sorted.
    fn build(root_name: String, files: &[String]) -> Self {
        #[derive(Default)]
        struct Dir {
            dirs: std::collections::BTreeMap<String, Dir>,
            files: Vec<String>,
        }
        let mut top = Dir::default();
        for file in files {
            let mut parts: Vec<&str> = file.split('/').collect();
            let name = parts.pop().unwrap_or_default();
            let dir = parts.into_iter().fold(&mut top, |dir, part| {
                dir.dirs.entry(part.to_string()).or_default()
            });
            dir.files.push(name.to_string());
        }
        fn flatten(dir: Dir, prefix: &str, depth: usize, nodes: &mut Vec<FileNode>) {
            for (name, child) in dir.dirs {
                let path = format!("{prefix}{name}");
                nodes.push(FileNode {
                    name,
                    path: path.clone(),
                    depth,
                    dir: true,
                    expanded: false,
                });
                flatten(child, &format!("{path}/"), depth + 1, nodes);
            }
            let mut files = dir.files;
            files.sort();
            for name in files {
                nodes.push(FileNode {
                    path: format!("{prefix}{name}"),
                    name,
                    depth,
                    dir: false,
                    expanded: false,
                });
            }
        }
        let mut nodes = vec![FileNode {
            name: root_name,
            path: String::new(),
            depth: 0,
            dir: true,
            expanded: true,
        }];
        flatten(top, "", 1, &mut nodes);
        Self {
            nodes,
            cursor: Some(0),
            ..Self::default()
        }
    }

    /// Indices of the visible nodes: those without a collapsed ancestor.
    pub fn rows(&self) -> Vec<usize> {
        let mut rows = Vec::new();
        let mut hidden_below = usize::MAX;
        for (index, node) in self.nodes.iter().enumerate() {
            if node.depth > hidden_below {
                continue;
            }
            hidden_below = if node.dir && !node.expanded {
                node.depth
            } else {
                usize::MAX
            };
            rows.push(index);
        }
        rows
    }

    fn step(&mut self, down: bool) {
        let rows = self.rows();
        let Some(last) = rows.len().checked_sub(1) else {
            return;
        };
        let row = self
            .cursor
            .and_then(|node| rows.iter().position(|&r| r == node));
        let row = match (row, down) {
            (None, _) => 0,
            (Some(row), true) => (row + 1).min(last),
            (Some(row), false) => row.saturating_sub(1),
        };
        self.cursor = Some(rows[row]);
    }

    /// Expand every ancestor folder of node `index`.
    fn reveal(&mut self, index: usize) {
        let mut depth = self.nodes[index].depth;
        for node in self.nodes[..index].iter_mut().rev() {
            if node.depth < depth {
                node.expanded = true;
                depth = node.depth;
            }
        }
    }
}

/// Find-in-files: the query and filter text, and the current results.
#[derive(Debug, Default)]
pub struct FindState {
    pub query: String,
    pub filter: String,
    pub results: Vec<GrepMatch>,
    /// More matches existed beyond `MAX_FIND_RESULTS`.
    pub truncated: bool,
    pub loading: bool,
    pub cursor: Option<usize>,
    /// First visible row, kept by `ui::render` so the cursor stays visible.
    pub offset: usize,
    /// First visible cell column.
    pub scroll_x: usize,
    /// Cursor row last scrolled into view; the wheel scrolls freely until it moves.
    pub followed: Option<usize>,
}

impl FindState {
    fn step(&mut self, down: bool) {
        let Some(last) = self.results.len().checked_sub(1) else {
            return;
        };
        self.cursor = Some(match (self.cursor, down) {
            (None, _) => 0,
            (Some(index), true) => (index + 1).min(last),
            (Some(index), false) => index.saturating_sub(1),
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Information,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    pub id: u64,
    pub title: Option<String>,
    pub body: String,
    pub severity: Severity,
    pub expires: Instant,
}

/// Work executed off the UI thread by the runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
    Status {
        root: PathBuf,
        token: u64,
    },
    /// An automatic status read with the current (unincremented) token.
    AutoStatus {
        root: PathBuf,
        token: u64,
    },
    /// Run `action` on `entries`, then read status, in one job.
    Mutate {
        root: PathBuf,
        action: Action,
        entries: Vec<FileEntry>,
        token: u64,
    },
    /// Load the diff of `entry`; runs on the latest-only diff worker.
    Diff {
        root: PathBuf,
        entry: DiffEntry,
        token: u64,
    },
    /// Load recent commits; runs on the latest-only history worker.
    History {
        root: PathBuf,
        token: u64,
    },
    /// Load the files of `commit`; runs on the latest-only commit-files worker.
    CommitFiles {
        root: PathBuf,
        commit: Commit,
        token: u64,
    },
    /// List the files under `cwd`; runs on the latest-only files worker.
    Files {
        cwd: PathBuf,
        token: u64,
    },
    /// Read a file preview; runs on the latest-only preview worker.
    Preview {
        path: PathBuf,
        token: u64,
    },
    /// Search `paths` for `query`; runs on the latest-only grep worker.
    Grep {
        cwd: PathBuf,
        query: String,
        paths: Vec<String>,
        limit: usize,
        token: u64,
    },
}

#[derive(Debug)]
pub enum Event {
    Input(Input),
    /// The clock reached `Instant`; expires toasts.
    Tick(Instant),
    /// A status read for request `token`, and the mutation that preceded it, if it failed.
    Status {
        token: u64,
        result: Result<RepoState, GitError>,
        failed: Option<(Action, GitError)>,
    },
    /// An automatic status read for request `token`.
    AutoStatus {
        token: u64,
        result: Result<RepoState, GitError>,
    },
    /// A watcher message.
    Watch(Watch),
    /// A diff document for request `token`.
    Diff {
        token: u64,
        result: Result<Document, GitError>,
    },
    /// Recent commits for history request `token`.
    History {
        token: u64,
        result: Result<Vec<Commit>, GitError>,
    },
    /// Files of the commit requested with `token`.
    CommitFiles {
        token: u64,
        result: Result<Vec<CommitFile>, GitError>,
    },
    /// Files under the launch directory for request `token`.
    Files {
        token: u64,
        result: Result<Vec<String>, GitError>,
    },
    /// A preview document for request `token`.
    Preview {
        token: u64,
        doc: Document,
    },
    /// Find-in-files results for request `token`.
    Grep {
        token: u64,
        result: Result<(Vec<GrepMatch>, bool), GitError>,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    Quit,
    Git(Job),
    /// Copy text to the system clipboard.
    Copy(String),
}

/// An in-progress splitter drag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Drag {
    pub splitter: Splitter,
    /// Pointer coordinate along the drag axis when the drag started.
    start: u16,
    /// Sizes of the two adjacent panes when the drag started.
    sizes: (u16, u16),
}

pub struct App {
    pub root: PathBuf,
    /// The launch directory, browsed by the Files tab.
    pub cwd: PathBuf,
    pub tab: Tab,
    pub focus: Focus,
    pub branch: String,
    pub staged: StatusList,
    pub unstaged: StatusList,
    pub status_loading: bool,
    status_token: u64,
    /// The status last applied to the lists.
    applied_state: Option<RepoState>,
    /// Invalidations waiting for an automatic read.
    auto_pending: Option<Invalidation>,
    /// The invalidation of the automatic read in flight.
    auto_in_flight: Option<Invalidation>,
    /// Automatic work currently in a failure streak.
    failing: Vec<Streak>,
    /// The watcher reported `Started`.
    watch_started: bool,
    /// The entry whose diff was last requested.
    pub selection: Option<DiffEntry>,
    pub commits: CommitTree,
    pub history_loading: bool,
    /// A commit's files are loading.
    pub files_loading: bool,
    history_token: u64,
    /// The pending history load is a quiet refresh.
    history_quiet: bool,
    /// The commits last applied to the tree.
    applied_commits: Option<Vec<Commit>>,
    commit_files_token: u64,
    /// The commit whose files are loading.
    files_for: usize,
    diff_token: u64,
    /// A normal diff load is pending.
    pub diff_loading: bool,
    /// The pending diff load is a quiet reload.
    diff_quiet: bool,
    /// The current change of the diff document, if it has any.
    change_index: Option<usize>,
    /// The viewer receiving mouse drags until the button is released.
    capture: Option<Focus>,
    pub modal: Option<Modal>,
    pub toasts: Vec<Toast>,
    next_toast: u64,
    pub files: FileTree,
    pub files_tree_loading: bool,
    files_token: u64,
    /// Listed paths paired with their lowercase form, for file jump.
    search_index: Vec<(String, String)>,
    /// First file jump result shown, and how many rows the last render showed.
    pub jump_offset: usize,
    pub jump_rows: usize,
    preview_token: u64,
    pub preview_loading: bool,
    /// The line a find-in-files match asked the preview to jump to.
    preview_line: Option<usize>,
    pub find: FindState,
    find_token: u64,
    pub diff_title: String,
    pub preview_title: String,
    pub diff_view: CodeView,
    pub preview_view: CodeView,
    /// Target under the pointer at the last mouse event.
    pub hover: Option<Target>,
    /// Hit map from the last draw; the last matching entry is topmost.
    pub hits: Vec<(Rect, Target)>,
    pub panes: Panes,
    pub drag: Option<Drag>,
    /// A dragged scrollbar thumb, its owner, and where it was grabbed.
    thumb: Option<(ThumbOwner, Scrollbar, usize)>,
}

/// Whose scrollbar thumb is being dragged.
#[derive(Clone, Copy)]
enum ThumbOwner {
    List(Focus),
    Jump,
}

impl App {
    pub fn new(root: PathBuf, cwd: PathBuf, show_shortcuts: bool) -> Self {
        Self {
            root,
            cwd,
            tab: Tab::Changes,
            focus: Focus::Staged,
            branch: String::new(),
            staged: StatusList::default(),
            unstaged: StatusList::default(),
            status_loading: true,
            status_token: 0,
            applied_state: None,
            auto_pending: None,
            auto_in_flight: None,
            failing: Vec::new(),
            watch_started: false,
            selection: None,
            commits: CommitTree::default(),
            history_loading: false,
            files_loading: false,
            history_token: 0,
            history_quiet: false,
            applied_commits: None,
            commit_files_token: 0,
            files_for: 0,
            diff_token: 0,
            diff_loading: false,
            diff_quiet: false,
            change_index: None,
            capture: None,
            modal: show_shortcuts.then_some(Modal::Shortcuts),
            toasts: Vec::new(),
            next_toast: 0,
            files: FileTree::default(),
            files_tree_loading: false,
            files_token: 0,
            search_index: Vec::new(),
            jump_offset: 0,
            jump_rows: 0,
            preview_token: 0,
            preview_loading: false,
            preview_line: None,
            find: FindState::default(),
            find_token: 0,
            diff_title: String::new(),
            preview_title: String::new(),
            diff_view: CodeView::new(),
            preview_view: CodeView::new(),
            hover: None,
            hits: Vec::new(),
            panes: Panes::default(),
            drag: None,
            thumb: None,
        }
    }

    /// Effects to run once at startup.
    pub fn start(&mut self) -> Vec<Effect> {
        self.refresh_all()
    }

    /// Manual refresh of status, history, and files.
    fn refresh_all(&mut self) -> Vec<Effect> {
        let mut effects = self.refresh_status();
        effects.extend(self.refresh_history());
        effects.extend(self.refresh_files());
        effects
    }

    /// Reload the Files tree: close file jump, show loading, clear the preview.
    fn refresh_files(&mut self) -> Vec<Effect> {
        if matches!(self.modal, Some(Modal::FileJump { .. })) {
            self.modal = None;
        }
        self.files_token += 1;
        self.files_tree_loading = true;
        self.clear_preview();
        vec![Effect::Git(Job::Files {
            cwd: self.cwd.clone(),
            token: self.files_token,
        })]
    }

    /// Clear the preview title and document and drop pending previews.
    fn clear_preview(&mut self) {
        self.preview_token += 1;
        self.preview_loading = false;
        self.preview_line = None;
        self.preview_title.clear();
        self.preview_view.set_document(Document::empty());
    }

    fn apply_files(&mut self, files: Vec<String>) -> Vec<Effect> {
        self.files = FileTree::build(self.cwd.display().to_string(), &files);
        self.search_index = files
            .into_iter()
            .map(|path| {
                let lower = path.to_lowercase();
                (path, lower)
            })
            .collect();
        self.clear_preview();
        self.search()
    }

    /// Activate the cursor node: toggle a folder (not the root), or preview a file.
    fn activate_file_row(&mut self) -> Vec<Effect> {
        let Some(index) = self.files.cursor else {
            return Vec::new();
        };
        let node = &mut self.files.nodes[index];
        if node.dir {
            if index > 0 {
                node.expanded = !node.expanded;
            }
            return Vec::new();
        }
        let path = self.cwd.join(&node.path);
        self.preview_title = path
            .strip_prefix(&self.root)
            .unwrap_or(&path)
            .display()
            .to_string();
        self.preview_token += 1;
        self.preview_loading = true;
        self.preview_line = None;
        vec![Effect::Git(Job::Preview {
            path,
            token: self.preview_token,
        })]
    }

    /// Open a find-in-files match in the preview, scrolled to its line.
    fn open_match(&mut self, found: GrepMatch) -> Vec<Effect> {
        let path = self.cwd.join(&found.path);
        self.preview_title = path
            .strip_prefix(&self.root)
            .unwrap_or(&path)
            .display()
            .to_string();
        self.preview_token += 1;
        self.preview_loading = true;
        self.preview_line = Some(found.line);
        vec![Effect::Git(Job::Preview {
            path,
            token: self.preview_token,
        })]
    }

    /// Run or clear find-in-files, bumping the token so a stale result is dropped.
    fn search(&mut self) -> Vec<Effect> {
        self.find_token += 1;
        if self.find.query.chars().count() < MIN_FIND_QUERY {
            self.find.loading = false;
            self.find.results.clear();
            self.find.truncated = false;
            self.find.cursor = None;
            self.find.offset = 0;
            self.find.followed = None;
            return Vec::new();
        }
        let paths = find::filter_paths(&self.search_index, &self.find.filter);
        self.find.loading = true;
        vec![Effect::Git(Job::Grep {
            cwd: self.cwd.clone(),
            query: self.find.query.clone(),
            paths,
            limit: MAX_FIND_RESULTS,
            token: self.find_token,
        })]
    }

    /// The field the focused find input box edits.
    fn find_field_mut(&mut self) -> &mut String {
        match self.focus {
            Focus::FindFilter => &mut self.find.filter,
            _ => &mut self.find.query,
        }
    }

    /// Edit the focused find input box, or move focus out of it.
    fn find_box_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        match key.code {
            KeyCode::Tab => self.cycle_focus(true),
            KeyCode::BackTab => self.cycle_focus(false),
            KeyCode::Down | KeyCode::Enter => {
                if !self.find.results.is_empty() {
                    self.focus = Focus::FindResults;
                    self.find.cursor = Some(0);
                }
            }
            KeyCode::Backspace => {
                self.find_field_mut().pop();
                return self.search();
            }
            KeyCode::Char(c) if (key.modifiers - KeyModifiers::SHIFT).is_empty() => {
                self.find_field_mut().push(c);
                return self.search();
            }
            _ => {}
        }
        Vec::new()
    }

    fn find_results_key(&mut self, code: KeyCode) -> Vec<Effect> {
        match code {
            KeyCode::Down | KeyCode::Char('j') => self.find.step(true),
            KeyCode::Up | KeyCode::Char('k') => self.find.step(false),
            KeyCode::Enter => return self.open_find_cursor(),
            _ => {}
        }
        Vec::new()
    }

    fn open_find_cursor(&mut self) -> Vec<Effect> {
        let Some(found) = self
            .find
            .cursor
            .and_then(|i| self.find.results.get(i))
            .cloned()
        else {
            return Vec::new();
        };
        self.open_match(found)
    }

    fn files_key(&mut self, code: KeyCode) -> Vec<Effect> {
        match code {
            KeyCode::Down | KeyCode::Char('j') => self.files.step(true),
            KeyCode::Up | KeyCode::Char('k') => self.files.step(false),
            KeyCode::Enter => return self.activate_file_row(),
            KeyCode::Right | KeyCode::Left => {
                if let Some(index) = self.files.cursor.filter(|&index| index > 0) {
                    let node = &mut self.files.nodes[index];
                    if node.dir {
                        node.expanded = code == KeyCode::Right;
                    }
                }
            }
            _ => {}
        }
        Vec::new()
    }

    /// The current file jump matches and whether they were truncated.
    pub fn jump_matches(&self) -> (Vec<String>, bool) {
        match &self.modal {
            Some(Modal::FileJump { query, .. }) => matching_files(&self.search_index, query),
            _ => (Vec::new(), false),
        }
    }

    /// Close file jump and reveal, select, and preview `path`, focusing the tree.
    fn jump_to(&mut self, path: &str) -> Vec<Effect> {
        self.modal = None;
        let Some(index) = self
            .files
            .nodes
            .iter()
            .position(|node| !node.dir && node.path == path)
        else {
            return Vec::new();
        };
        self.files.reveal(index);
        self.files.cursor = Some(index);
        self.tab = Tab::Files;
        self.focus = Focus::FilesTree;
        self.activate_file_row()
    }

    fn jump_key(&mut self, code: KeyCode) -> Vec<Effect> {
        let (matches, _) = self.jump_matches();
        let Some(Modal::FileJump { query, selected }) = &mut self.modal else {
            return Vec::new();
        };
        match code {
            KeyCode::Esc => self.modal = None,
            KeyCode::Enter => {
                if let Some(path) = matches.get(selected.unwrap_or(0)) {
                    return self.jump_to(&path.clone());
                }
            }
            KeyCode::Down if !matches.is_empty() => {
                *selected = Some(selected.map_or(0, |i| (i + 1).min(matches.len() - 1)));
            }
            KeyCode::Up => *selected = selected.and_then(|i| i.checked_sub(1)),
            KeyCode::Backspace => {
                query.pop();
                *selected = None;
                self.jump_offset = 0;
            }
            KeyCode::Char(c) => {
                query.push(c);
                *selected = None;
                self.jump_offset = 0;
            }
            _ => {}
        }
        // Keep the highlighted result in view once a render has sized the list.
        if self.jump_rows > 0
            && let Some(Modal::FileJump {
                selected: Some(i), ..
            }) = self.modal
        {
            self.jump_offset = self
                .jump_offset
                .min(i)
                .max((i + 1).saturating_sub(self.jump_rows));
        }
        Vec::new()
    }

    /// The tree is loading history or commit files, and ignores input.
    pub fn tree_loading(&self) -> bool {
        self.history_loading || self.files_loading
    }

    /// Manual history refresh: show loading on the tree, then load.
    fn refresh_history(&mut self) -> Vec<Effect> {
        self.history_loading = true;
        self.load_history(false)
    }

    /// Request history; a quiet load shows no loading state and warns once per streak.
    fn load_history(&mut self, quiet: bool) -> Vec<Effect> {
        self.history_token += 1;
        self.history_quiet = quiet;
        vec![Effect::Git(Job::History {
            root: self.root.clone(),
            token: self.history_token,
        })]
    }

    /// Apply loaded commits; an unchanged list keeps the tree exactly. Returns the
    /// file load of the first commit when a non-quiet load selects it.
    fn apply_history(&mut self, commits: Vec<Commit>, quiet: bool) -> Vec<Effect> {
        if self.applied_commits.as_ref() == Some(&commits) {
            return Vec::new();
        }
        let target = self.commits.cursor_hash();
        // Drop pending commit-file loads.
        self.commit_files_token += 1;
        self.files_loading = false;
        self.commits.nodes = commits
            .iter()
            .map(|commit| CommitNode {
                commit: commit.clone(),
                expanded: false,
                files: None,
            })
            .collect();
        self.commits.cursor = commits
            .iter()
            .position(|commit| Some(&commit.hash) == target.as_ref());
        self.applied_commits = Some(commits);
        if !quiet
            && !self.commits.nodes.is_empty()
            && self.staged.entries.is_empty()
            && self.unstaged.entries.is_empty()
        {
            self.commits.cursor = Some(0);
            if self.tab == Tab::Changes {
                self.focus = Focus::Commits;
            }
            return self.set_expanded(0, true);
        }
        Vec::new()
    }

    /// Expand or collapse commit `index`, loading its files on first expansion.
    fn set_expanded(&mut self, index: usize, expanded: bool) -> Vec<Effect> {
        let node = &mut self.commits.nodes[index];
        node.expanded = expanded;
        if !expanded || node.files.is_some() {
            return Vec::new();
        }
        let commit = node.commit.clone();
        self.commit_files_token += 1;
        self.files_for = index;
        self.files_loading = true;
        vec![Effect::Git(Job::CommitFiles {
            root: self.root.clone(),
            commit,
            token: self.commit_files_token,
        })]
    }

    /// Activate the cursor row: toggle a commit, or show a file's diff.
    fn activate_tree_row(&mut self) -> Vec<Effect> {
        match self.commits.cursor_row() {
            Some(TreeRow::Commit(index)) => {
                let expanded = self.commits.nodes[index].expanded;
                self.set_expanded(index, !expanded)
            }
            Some(TreeRow::File(index, file)) => {
                let files = self.commits.nodes[index].files.as_ref();
                let file = files.expect("loaded files")[file].clone();
                self.request_diff(DiffEntry::Commit(file))
            }
            Some(TreeRow::Empty(_)) | None => Vec::new(),
        }
    }

    fn tree_key(&mut self, code: KeyCode) -> Vec<Effect> {
        match code {
            KeyCode::Down | KeyCode::Char('j') => self.commits.step(true),
            KeyCode::Up | KeyCode::Char('k') => self.commits.step(false),
            KeyCode::Enter => return self.activate_tree_row(),
            KeyCode::Right | KeyCode::Left => {
                if let Some(TreeRow::Commit(index)) = self.commits.cursor_row() {
                    return self.set_expanded(index, code == KeyCode::Right);
                }
            }
            _ => {}
        }
        Vec::new()
    }

    pub fn list(&self, side: Side) -> &StatusList {
        match side {
            Side::Staged => &self.staged,
            Side::Unstaged => &self.unstaged,
        }
    }

    fn list_mut(&mut self, side: Side) -> &mut StatusList {
        match side {
            Side::Staged => &mut self.staged,
            Side::Unstaged => &mut self.unstaged,
        }
    }

    /// The side of the focused status list, if one has focus.
    fn focused_side(&self) -> Option<Side> {
        match self.focus {
            Focus::Staged => Some(Side::Staged),
            Focus::Unstaged => Some(Side::Unstaged),
            _ => None,
        }
    }

    /// Start a manual status refresh and return the token of its request.
    fn start_status_refresh(&mut self) -> u64 {
        self.status_token += 1;
        // Drop pending diff loads without clearing the diff.
        self.diff_token += 1;
        self.status_loading = true;
        self.status_token
    }

    fn refresh_status(&mut self) -> Vec<Effect> {
        let token = self.start_status_refresh();
        vec![Effect::Git(Job::Status {
            root: self.root.clone(),
            token,
        })]
    }

    /// Queue `action` on the supported `entries` as one job.
    fn mutate(&mut self, action: Action, entries: Vec<FileEntry>) -> Vec<Effect> {
        let entries: Vec<FileEntry> = entries
            .into_iter()
            .filter(|entry| entry.unsupported_reason.is_none())
            .collect();
        if entries.is_empty() {
            return Vec::new();
        }
        let token = self.start_status_refresh();
        vec![Effect::Git(Job::Mutate {
            root: self.root.clone(),
            action,
            entries,
            token,
        })]
    }

    /// Stage or unstage directly; ask before discarding.
    fn request(&mut self, action: Action, entries: Vec<FileEntry>) -> Vec<Effect> {
        if action != Action::Discard {
            return self.mutate(action, entries);
        }
        let entries: Vec<FileEntry> = entries
            .into_iter()
            .filter(|entry| entry.unsupported_reason.is_none())
            .collect();
        if !entries.is_empty() {
            self.modal = Some(Modal::Discard {
                entries,
                confirm: false,
            });
        }
        Vec::new()
    }

    /// Select a status entry for the diff pane and load its diff.
    fn select(&mut self, entry: FileEntry) -> Vec<Effect> {
        self.request_diff(DiffEntry::File(entry))
    }

    /// Show `entry` in the diff pane and load its diff.
    fn request_diff(&mut self, entry: DiffEntry) -> Vec<Effect> {
        let (path, reason) = match &entry {
            DiffEntry::File(file) => (&file.path, file.unsupported_reason.as_ref()),
            DiffEntry::Commit(file) => (&file.path, None),
        };
        self.diff_title = path.clone();
        self.change_index = None;
        self.diff_token += 1;
        self.diff_quiet = false;
        if let Some(reason) = reason.cloned() {
            self.selection = Some(entry);
            self.diff_loading = false;
            self.apply_diff(Document::message(&reason));
            return Vec::new();
        }
        self.selection = Some(entry.clone());
        self.diff_loading = true;
        vec![Effect::Git(Job::Diff {
            root: self.root.clone(),
            entry,
            token: self.diff_token,
        })]
    }

    /// Show a loaded diff and scroll to its first change.
    fn apply_diff(&mut self, doc: Document) {
        self.change_index = (!doc.changes.is_empty()).then_some(0);
        self.diff_view.set_document(doc);
        self.scroll_to_change();
    }

    /// Clear the selection, title, document, and change buttons; drop pending loads.
    fn invalidate_diff(&mut self) {
        self.diff_token += 1;
        self.diff_loading = false;
        self.selection = None;
        self.diff_title.clear();
        self.change_index = None;
        self.diff_view.set_document(Document::empty());
    }

    fn scroll_to_change(&mut self) {
        let Some(index) = self.change_index else {
            return;
        };
        let row = self.diff_view.document().changes[index].saturating_sub(CHANGE_CONTEXT);
        let (_, y) = self.diff_view.scroll_offset();
        self.diff_view.scroll_to(0, y);
        self.diff_view.scroll_to_row(row);
    }

    /// Whether a change button can be pressed.
    pub fn enabled(&self, button: Button) -> bool {
        let count = self.diff_view.document().changes.len();
        match (button, self.change_index) {
            (Button::PreviousChange, Some(index)) => index > 0,
            (Button::NextChange, Some(index)) => index + 1 < count,
            (Button::PreviousChange | Button::NextChange, None) => false,
            _ => true,
        }
    }

    /// Move to the next (`forward`) or previous change, if there is one.
    fn navigate(&mut self, forward: bool) {
        let button = if forward {
            Button::NextChange
        } else {
            Button::PreviousChange
        };
        if let Some(index) = self.change_index.filter(|_| self.enabled(button)) {
            self.change_index = Some(if forward { index + 1 } else { index - 1 });
            self.scroll_to_change();
        }
    }

    fn view_mut(&mut self, focus: Focus) -> &mut CodeView {
        match focus {
            Focus::Preview => &mut self.preview_view,
            _ => &mut self.diff_view,
        }
    }

    /// The focused viewer, if a viewer has focus.
    fn focused_view(&mut self) -> Option<&mut CodeView> {
        matches!(self.focus, Focus::Diff | Focus::Preview).then(|| self.view_mut(self.focus))
    }

    /// The hint for the hovered button, if any.
    pub fn hint(&self) -> Option<&'static str> {
        match self.hover {
            Some(Target::Button(button)) => Some(button.hint()),
            _ => None,
        }
    }

    /// When the next toast expires.
    pub fn deadline(&self) -> Option<Instant> {
        self.toasts.iter().map(|toast| toast.expires).min()
    }

    /// Drop toasts whose expiry is at or before `now`.
    pub fn expire_due(&mut self, now: Instant) {
        self.toasts.retain(|toast| toast.expires > now);
    }

    pub fn notify(&mut self, title: Option<&str>, body: &str, severity: Severity) {
        self.next_toast += 1;
        self.toasts.push(Toast {
            id: self.next_toast,
            title: title.map(str::to_string),
            body: body.to_string(),
            severity,
            expires: Instant::now() + TOAST_LIFETIME,
        });
    }

    fn git_error(&mut self, action: &str, error: &GitError) {
        let title = format!("Could not {action}");
        self.notify(Some(&title), &error.to_string(), Severity::Error);
    }

    pub fn update(&mut self, event: Event) -> Vec<Effect> {
        let before = (self.tab, self.modal.clone());
        let effects = self.handle(event);
        if (self.tab, self.modal.clone()) != before {
            self.hover = None;
            self.thumb = None;
        }
        effects
    }

    fn handle(&mut self, event: Event) -> Vec<Effect> {
        match event {
            Event::Input(Input::Key(key)) if key.kind != KeyEventKind::Release => {
                return self.key(key);
            }
            Event::Input(Input::Mouse(mouse)) => return self.mouse(mouse),
            Event::Input(_) => {}
            Event::Tick(now) => self.expire_due(now),
            Event::Status {
                token,
                result,
                failed,
            } => {
                if let Some((action, error)) = failed {
                    self.git_error(action.name(), &error);
                }
                if token != self.status_token {
                    return Vec::new();
                }
                self.status_loading = false;
                self.diff_loading = false;
                match result {
                    Ok(state) => {
                        self.apply_status(state);
                        self.focus_first_entry();
                    }
                    Err(error) => self.git_error("refresh status", &error),
                }
            }
            Event::AutoStatus { token, result } => {
                let invalidation = self.auto_in_flight.take().unwrap_or_default();
                if token != self.status_token {
                    self.merge_pending(invalidation);
                    return self.start_auto_read();
                }
                return match result {
                    Ok(state) => {
                        self.failing.retain(|s| *s != Streak::Status);
                        let mut effects = self.quiet_apply(state, invalidation);
                        effects.extend(self.start_auto_read());
                        effects
                    }
                    Err(error) => {
                        self.merge_pending(invalidation);
                        self.auto_failure(Streak::Status, &error);
                        Vec::new()
                    }
                };
            }
            Event::Watch(Watch::Started) => self.watch_started = true,
            Event::Watch(Watch::Changed(invalidation)) => {
                if invalidation.status {
                    self.merge_pending(invalidation);
                    return self.start_auto_read();
                }
            }
            Event::Watch(Watch::Stopped) => {
                let body = if self.watch_started {
                    "Automatic refresh stopped. Press r to refresh manually."
                } else {
                    "Automatic refresh could not start. Press r to refresh manually."
                };
                self.notify(None, body, Severity::Warning);
            }
            Event::Diff { token, result } => {
                if token != self.diff_token {
                    return Vec::new();
                }
                self.diff_loading = false;
                match (result, self.diff_quiet) {
                    (Ok(doc), false) => self.apply_diff(doc),
                    (Err(error), false) => self.git_error("load diff", &error),
                    (Ok(doc), true) => {
                        self.failing.retain(|s| *s != Streak::Diff);
                        self.apply_quiet_diff(doc);
                    }
                    (Err(error), true) => self.auto_failure(Streak::Diff, &error),
                }
            }
            Event::History { token, result } => {
                if token != self.history_token {
                    return Vec::new();
                }
                self.history_loading = false;
                let quiet = self.history_quiet;
                match result {
                    Ok(commits) => {
                        if quiet {
                            self.failing.retain(|s| *s != Streak::History);
                        }
                        return self.apply_history(commits, quiet);
                    }
                    Err(error) if quiet => self.auto_failure(Streak::History, &error),
                    Err(error) => self.git_error("refresh history", &error),
                }
            }
            Event::Files { token, result } => {
                if token != self.files_token {
                    return Vec::new();
                }
                self.files_tree_loading = false;
                match result {
                    Ok(files) => return self.apply_files(files),
                    Err(error) => self.git_error("refresh files", &error),
                }
            }
            Event::Preview { token, doc } => {
                if token != self.preview_token {
                    return Vec::new();
                }
                self.preview_loading = false;
                match self.preview_line.take() {
                    Some(line) => {
                        let row = line.saturating_sub(1);
                        let mut doc = doc;
                        doc.mark_row(row);
                        self.preview_view.set_document(doc);
                        self.preview_view
                            .scroll_to_row(row.saturating_sub(CHANGE_CONTEXT));
                    }
                    None => self.preview_view.set_document(doc),
                }
            }
            Event::Grep { token, result } => {
                if token != self.find_token {
                    return Vec::new();
                }
                self.find.loading = false;
                match result {
                    Ok((results, truncated)) => {
                        self.find.results = results;
                        self.find.truncated = truncated;
                        self.find.cursor = None;
                        self.find.offset = 0;
                        self.find.followed = None;
                    }
                    Err(error) => self.git_error("search files", &error),
                }
            }
            Event::CommitFiles { token, result } => {
                if token != self.commit_files_token {
                    return Vec::new();
                }
                self.files_loading = false;
                match result {
                    Ok(files) => self.commits.nodes[self.files_for].files = Some(files),
                    Err(error) => self.git_error("load commit files", &error),
                }
            }
        }
        Vec::new()
    }

    /// Rebuild both lists, invalidate the diff pane, and update the branch.
    fn apply_status(&mut self, state: RepoState) {
        self.branch = state.branch.clone();
        self.staged = StatusList::new(state.staged.clone());
        self.unstaged = StatusList::new(state.unstaged.clone());
        self.invalidate_diff();
        self.applied_state = Some(state);
    }

    fn merge_pending(&mut self, invalidation: Invalidation) {
        match &mut self.auto_pending {
            Some(pending) => pending.merge(invalidation),
            None => self.auto_pending = Some(invalidation),
        }
    }

    /// Start an automatic read of the pending invalidation unless one is in flight.
    fn start_auto_read(&mut self) -> Vec<Effect> {
        if self.auto_in_flight.is_some() {
            return Vec::new();
        }
        let Some(pending) = self.auto_pending.take() else {
            return Vec::new();
        };
        self.auto_in_flight = Some(pending);
        vec![Effect::Git(Job::AutoStatus {
            root: self.root.clone(),
            token: self.status_token,
        })]
    }

    /// Warn once per failure streak of `streak`.
    fn auto_failure(&mut self, streak: Streak, error: &GitError) {
        if self.failing.contains(&streak) {
            return;
        }
        self.failing.push(streak);
        let body = format!("Automatic refresh failed: {error}. Press r to refresh manually.");
        self.notify(None, &body, Severity::Warning);
    }

    /// Apply an automatic status read without moving focus or showing loading.
    fn quiet_apply(&mut self, state: RepoState, invalidation: Invalidation) -> Vec<Effect> {
        if self.applied_state.as_ref() != Some(&state) {
            self.branch = state.branch.clone();
            rebuild(&mut self.staged, state.staged.clone());
            rebuild(&mut self.unstaged, state.unstaged.clone());
        }
        let mut effects = self.reconcile_diff(&state, &invalidation);
        if invalidation.history {
            effects.extend(self.load_history(true));
        }
        self.applied_state = Some(state);
        effects
    }

    /// Clear or quietly reload the open working-tree diff after a quiet apply.
    fn reconcile_diff(&mut self, state: &RepoState, invalidation: &Invalidation) -> Vec<Effect> {
        let Some(DiffEntry::File(selection)) = &self.selection else {
            return Vec::new();
        };
        let entries = match selection.side {
            Side::Staged => &state.staged,
            Side::Unstaged => &state.unstaged,
        };
        let Some(current) = entries.iter().find(|e| e.path == selection.path).cloned() else {
            self.invalidate_diff();
            self.diff_view.set_document(Document::message(MISSING));
            return Vec::new();
        };
        let stale = match current.side {
            Side::Staged => invalidation.index_changed || invalidation.history,
            Side::Unstaged => {
                invalidation.changed_paths.contains(&current.path)
                    || invalidation.history
                    || (invalidation.index_changed && current.status != '?')
            }
        };
        if !stale {
            return Vec::new();
        }
        if current.unsupported_reason.is_some() {
            return self.select(current);
        }
        self.selection = Some(DiffEntry::File(current.clone()));
        self.diff_token += 1;
        self.diff_quiet = true;
        self.diff_loading = false;
        vec![Effect::Git(Job::Diff {
            root: self.root.clone(),
            entry: DiffEntry::File(current),
            token: self.diff_token,
        })]
    }

    /// Replace the diff quietly, keeping scroll, wrap, and a still-valid change index.
    fn apply_quiet_diff(&mut self, doc: Document) {
        if same_document(self.diff_view.document(), &doc) {
            return;
        }
        if self
            .change_index
            .is_none_or(|index| index >= doc.changes.len())
        {
            self.change_index = (!doc.changes.is_empty()).then_some(0);
        }
        let (x, y) = self.diff_view.scroll_offset();
        self.diff_view.set_document(doc);
        self.diff_view.scroll_to(x, y);
    }

    /// Highlight and focus the first entry, Staged before Unstaged (manual loads only).
    fn focus_first_entry(&mut self) {
        let focus = self.first_changes_pane();
        if let Some(side) = match focus {
            Focus::Staged => Some(Side::Staged),
            Focus::Unstaged => Some(Side::Unstaged),
            _ => None,
        } {
            self.list_mut(side).highlight = Some(0);
            if self.tab == Tab::Changes {
                self.focus = focus;
            }
        }
    }

    fn first_changes_pane(&self) -> Focus {
        if !self.staged.entries.is_empty() {
            Focus::Staged
        } else if !self.unstaged.entries.is_empty() {
            Focus::Unstaged
        } else {
            Focus::Commits
        }
    }

    fn show_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.focus = match tab {
            Tab::Changes => self.first_changes_pane(),
            Tab::Files => Focus::FilesTree,
            Tab::Find => Focus::FindQuery,
        };
    }

    fn cycle_focus(&mut self, forward: bool) {
        let order: &[Focus] = match self.tab {
            Tab::Changes => &CHANGES_ORDER,
            Tab::Files => &FILES_ORDER,
            Tab::Find => &FIND_ORDER,
        };
        let index = order.iter().position(|f| *f == self.focus).unwrap_or(0);
        let step = if forward { 1 } else { order.len() - 1 };
        self.focus = order[(index + step) % order.len()];
    }

    fn key(&mut self, key: KeyEvent) -> Vec<Effect> {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return vec![Effect::Quit];
        }
        if key.code == KeyCode::Char('f')
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && self.modal.is_none()
        {
            self.show_tab(Tab::Find);
            return Vec::new();
        }
        match &mut self.modal {
            Some(Modal::Shortcuts) => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('h')) {
                    self.modal = None;
                }
                return Vec::new();
            }
            Some(Modal::Discard { confirm, .. }) => {
                match key.code {
                    KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => {
                        *confirm = !*confirm;
                    }
                    KeyCode::Esc => self.modal = None,
                    KeyCode::Enter => {
                        let confirmed = *confirm;
                        return self.close_discard(confirmed);
                    }
                    _ => {}
                }
                return Vec::new();
            }
            Some(Modal::FileJump { .. }) => return self.jump_key(key.code),
            None => {}
        }
        if matches!(self.focus, Focus::FindQuery | Focus::FindFilter) {
            return self.find_box_key(key);
        }
        if self.focus == Focus::FindResults
            && matches!(
                key.code,
                KeyCode::Down
                    | KeyCode::Up
                    | KeyCode::Char('j')
                    | KeyCode::Char('k')
                    | KeyCode::Enter
            )
        {
            return self.find_results_key(key.code);
        }
        if let Some(side) = self.focused_side() {
            let list = self.list_mut(side);
            match key.code {
                KeyCode::Down | KeyCode::Char('j') => list.step(true),
                KeyCode::Up | KeyCode::Char('k') => list.step(false),
                KeyCode::Char(' ') => {
                    if let Some(index) = list.highlight {
                        list.toggle(index);
                    }
                }
                KeyCode::Enter => {
                    if let Some(entry) = list.highlighted().cloned() {
                        return self.select(entry);
                    }
                }
                KeyCode::Char('s') => {
                    if let Some(entry) = list.highlighted().cloned() {
                        return self.mutate(Action::toggle(side), vec![entry]);
                    }
                }
                KeyCode::Char('d') if side == Side::Unstaged => {
                    if let Some(entry) = list.highlighted().cloned() {
                        return self.request(Action::Discard, vec![entry]);
                    }
                }
                _ => {}
            }
        }
        if self.focus == Focus::Commits
            && matches!(
                key.code,
                KeyCode::Down
                    | KeyCode::Up
                    | KeyCode::Char('j')
                    | KeyCode::Char('k')
                    | KeyCode::Enter
                    | KeyCode::Right
                    | KeyCode::Left
            )
        {
            if self.tree_loading() {
                return Vec::new();
            }
            return self.tree_key(key.code);
        }
        if self.focus == Focus::FilesTree
            && matches!(
                key.code,
                KeyCode::Down
                    | KeyCode::Up
                    | KeyCode::Char('j')
                    | KeyCode::Char('k')
                    | KeyCode::Enter
                    | KeyCode::Right
                    | KeyCode::Left
            )
        {
            if self.files_tree_loading {
                return Vec::new();
            }
            return self.files_key(key.code);
        }
        match key.code {
            KeyCode::Char('q') => return vec![Effect::Quit],
            KeyCode::Char('r') => return self.refresh_all(),
            KeyCode::Char('t') if self.tab == Tab::Files && !self.files_tree_loading => {
                self.modal = Some(Modal::FileJump {
                    query: String::new(),
                    selected: None,
                });
                self.jump_offset = 0;
            }
            KeyCode::Char('h') => self.modal = Some(Modal::Shortcuts),
            KeyCode::Char('1') => self.show_tab(Tab::Changes),
            KeyCode::Char('2') => self.show_tab(Tab::Files),
            KeyCode::Char('3') => self.show_tab(Tab::Find),
            KeyCode::Tab => self.cycle_focus(true),
            KeyCode::BackTab => self.cycle_focus(false),
            KeyCode::Char('n') if self.tab == Tab::Changes => self.navigate(true),
            KeyCode::Char('p') if self.tab == Tab::Changes => self.navigate(false),
            KeyCode::Char('w') => {
                let view = match self.tab {
                    Tab::Changes => &mut self.diff_view,
                    Tab::Files | Tab::Find => &mut self.preview_view,
                };
                view.set_wrapped(!view.wrapped());
            }
            // App shortcuts win: only unmodified keys reach a focused viewer.
            _ if (key.modifiers - KeyModifiers::SHIFT).is_empty() => {
                if let Some(view) = self.focused_view() {
                    view.handle_key(key);
                }
            }
            _ => {}
        }
        Vec::new()
    }

    /// Close the discard dialog, discarding its entries when `confirmed`.
    fn close_discard(&mut self, confirmed: bool) -> Vec<Effect> {
        match self.modal.take() {
            Some(Modal::Discard { entries, .. }) if confirmed => {
                self.mutate(Action::Discard, entries)
            }
            _ => Vec::new(),
        }
    }

    fn press_button(&mut self, button: Button) -> Vec<Effect> {
        match button {
            Button::Row(side, index, action) => match self.list(side).entries.get(index) {
                Some(entry) => self.request(action, vec![entry.clone()]),
                None => Vec::new(),
            },
            Button::Bulk(action) => {
                let side = match action {
                    Action::Unstage => Side::Staged,
                    Action::Stage | Action::Discard => Side::Unstaged,
                };
                let entries = self.list(side).checked_entries();
                self.request(action, entries)
            }
            Button::PreviousChange => {
                self.navigate(false);
                Vec::new()
            }
            Button::NextChange => {
                self.navigate(true);
                Vec::new()
            }
        }
    }

    fn focus_list(&mut self, side: Side) {
        self.tab = Tab::Changes;
        self.focus = match side {
            Side::Staged => Focus::Staged,
            Side::Unstaged => Focus::Unstaged,
        };
    }

    fn mouse(&mut self, mouse: MouseEvent) -> Vec<Effect> {
        // A drag captures the mouse until the button is released.
        if let Some(drag) = self.drag {
            match mouse.kind {
                MouseEventKind::Up(_) => self.drag = None,
                MouseEventKind::Drag(_) | MouseEventKind::Moved => {
                    let delta = axis(drag.splitter, mouse) as i32 - drag.start as i32;
                    let (group, index) = self.panes.group(drag.splitter);
                    group.drag(index, drag.sizes, delta);
                }
                _ => {}
            }
            return Vec::new();
        }
        if let Some((focus, bar, grab)) = self.thumb {
            match mouse.kind {
                MouseEventKind::Up(_) => self.thumb = None,
                MouseEventKind::Drag(_) | MouseEventKind::Moved => {
                    let value = bar.drag(Position::new(mouse.column, mouse.row), grab);
                    match focus {
                        ThumbOwner::List(focus) => self.set_list_scroll(focus, bar.vertical, value),
                        ThumbOwner::Jump => self.jump_offset = value,
                    }
                }
                _ => {}
            }
            return Vec::new();
        }
        // A viewer that took a press receives drags and the release.
        if let Some(focus) = self.capture
            && matches!(mouse.kind, MouseEventKind::Drag(_) | MouseEventKind::Up(_))
        {
            let copied = self.view_mut(focus).handle_mouse(mouse);
            if matches!(mouse.kind, MouseEventKind::Up(_)) {
                self.capture = None;
            }
            return copied.map(Effect::Copy).into_iter().collect();
        }
        let target = self.hit(Position::new(mouse.column, mouse.row));
        match mouse.kind {
            MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight => {
                if let Some(Target::Pane(focus @ (Focus::Diff | Focus::Preview))) = target {
                    self.view_mut(focus).handle_mouse(mouse);
                } else if let Some(focus) = target.and_then(Target::list_pane) {
                    self.scroll_list(focus, mouse);
                } else if let Some(
                    Target::JumpDialog | Target::JumpResult(_) | Target::JumpScrollbar(_),
                ) = target
                {
                    // The wheel scrolls the results without moving the highlight.
                    match mouse.kind {
                        MouseEventKind::ScrollUp => {
                            self.jump_offset = self.jump_offset.saturating_sub(1)
                        }
                        MouseEventKind::ScrollDown => self.jump_offset += 1,
                        _ => {}
                    }
                }
            }
            MouseEventKind::Moved => self.hover = target,
            MouseEventKind::Down(MouseButton::Left) => match target {
                Some(Target::Tab(tab)) => self.show_tab(tab),
                Some(Target::Pane(focus)) => {
                    self.focus = focus;
                    if matches!(focus, Focus::Diff | Focus::Preview) {
                        self.capture = Some(focus);
                        self.view_mut(focus).handle_mouse(mouse);
                    }
                }
                Some(Target::Scrollbar(focus, bar)) => {
                    self.focus = focus;
                    let pos = Position::new(mouse.column, mouse.row);
                    match bar.press(pos, self.list_scroll(focus, bar.vertical)) {
                        Press::Grab(grab) => {
                            self.thumb = Some((ThumbOwner::List(focus), bar, grab))
                        }
                        Press::Jump(value) => self.set_list_scroll(focus, bar.vertical, value),
                    }
                }
                Some(Target::JumpScrollbar(bar)) => {
                    let pos = Position::new(mouse.column, mouse.row);
                    match bar.press(pos, self.jump_offset) {
                        Press::Grab(grab) => self.thumb = Some((ThumbOwner::Jump, bar, grab)),
                        Press::Jump(value) => self.jump_offset = value,
                    }
                }
                Some(Target::CloseShortcuts) => self.modal = None,
                Some(Target::CancelDiscard) => return self.close_discard(false),
                Some(Target::ConfirmDiscard) => return self.close_discard(true),
                Some(Target::Row(side, index)) => {
                    self.focus_list(side);
                    self.list_mut(side).highlight = Some(index);
                    if let Some(entry) = self.list(side).entries.get(index).cloned() {
                        return self.select(entry);
                    }
                }
                Some(Target::TreeRow(_)) if self.tree_loading() => {}
                Some(Target::TreeRow(row)) => {
                    self.tab = Tab::Changes;
                    self.focus = Focus::Commits;
                    self.commits.cursor = Some(row);
                    return self.activate_tree_row();
                }
                Some(Target::FileRow(_)) if self.files_tree_loading => {}
                Some(Target::FileRow(row)) => {
                    self.tab = Tab::Files;
                    self.focus = Focus::FilesTree;
                    self.files.cursor = self.files.rows().get(row).copied();
                    return self.activate_file_row();
                }
                Some(Target::FindResult(row)) => {
                    self.focus = Focus::FindResults;
                    self.find.cursor = Some(row);
                    if let Some(found) = self.find.results.get(row).cloned() {
                        return self.open_match(found);
                    }
                }
                Some(Target::JumpResult(index)) => {
                    if let Some(path) = self.jump_matches().0.get(index) {
                        return self.jump_to(&path.clone());
                    }
                }
                Some(Target::Backdrop) if matches!(self.modal, Some(Modal::FileJump { .. })) => {
                    self.modal = None;
                }
                Some(Target::Checkbox(side, index)) => {
                    self.focus_list(side);
                    self.list_mut(side).toggle(index);
                }
                Some(Target::Button(button)) => return self.press_button(button),
                Some(Target::Toast(id)) => self.toasts.retain(|toast| toast.id != id),
                Some(Target::Splitter(splitter)) => {
                    let start = axis(splitter, mouse);
                    let (group, index) = self.panes.group(splitter);
                    let sizes = group.start_drag(index);
                    self.drag = Some(Drag {
                        splitter,
                        start,
                        sizes,
                    });
                }
                Some(Target::Backdrop | Target::JumpDialog) | None => {}
            },
            _ => {}
        }
        Vec::new()
    }

    /// The (vertical, horizontal) scroll offsets of a list or tree.
    fn list_axes(&mut self, focus: Focus) -> Option<(&mut usize, Option<&mut usize>)> {
        match focus {
            Focus::Staged => Some((&mut self.staged.offset, None)),
            Focus::Unstaged => Some((&mut self.unstaged.offset, None)),
            Focus::Commits => Some((&mut self.commits.offset, Some(&mut self.commits.scroll_x))),
            Focus::FilesTree => Some((&mut self.files.offset, Some(&mut self.files.scroll_x))),
            Focus::FindResults => Some((&mut self.find.offset, Some(&mut self.find.scroll_x))),
            Focus::Diff | Focus::Preview | Focus::FindQuery | Focus::FindFilter => None,
        }
    }

    fn list_axis(&mut self, focus: Focus, vertical: bool) -> Option<&mut usize> {
        let (y, x) = self.list_axes(focus)?;
        if vertical { Some(y) } else { x }
    }

    fn list_scroll(&mut self, focus: Focus, vertical: bool) -> usize {
        self.list_axis(focus, vertical).map_or(0, |v| *v)
    }

    /// Scroll a list or tree axis without moving its cursor; `ui::render`
    /// clamps the result.
    fn set_list_scroll(&mut self, focus: Focus, vertical: bool, value: usize) {
        if let Some(axis) = self.list_axis(focus, vertical) {
            *axis = value;
        }
    }

    /// Scroll a list or tree with the wheel, leaving its cursor in place;
    /// `ui::render` clamps the result.
    fn scroll_list(&mut self, focus: Focus, mouse: MouseEvent) {
        let Some((y, x)) = self.list_axes(focus) else {
            return;
        };
        let shift = mouse.modifiers.contains(KeyModifiers::SHIFT);
        match (mouse.kind, x) {
            (MouseEventKind::ScrollUp, Some(x)) if shift => *x = x.saturating_sub(WHEEL),
            (MouseEventKind::ScrollDown, Some(x)) if shift => *x += WHEEL,
            (MouseEventKind::ScrollLeft, Some(x)) => *x = x.saturating_sub(WHEEL),
            (MouseEventKind::ScrollRight, Some(x)) => *x += WHEEL,
            (MouseEventKind::ScrollUp, _) => *y = y.saturating_sub(WHEEL),
            (MouseEventKind::ScrollDown, _) => *y += WHEEL,
            _ => {}
        }
    }

    fn hit(&self, position: Position) -> Option<Target> {
        self.hits
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(position))
            .map(|(_, target)| *target)
    }
}

/// The pointer coordinate along `splitter`'s drag axis.
fn axis(splitter: Splitter, mouse: MouseEvent) -> u16 {
    match splitter {
        Splitter::Section(_) => mouse.row,
        Splitter::Sidebar | Splitter::Files => mouse.column,
    }
}

/// Replace `list`'s entries, keeping its highlighted and checked rows by path.
fn rebuild(list: &mut StatusList, entries: Vec<FileEntry>) {
    let highlighted = list.highlighted().map(|entry| entry.path.clone());
    let checked: Vec<String> = list
        .checked_entries()
        .into_iter()
        .map(|entry| entry.path)
        .collect();
    let mut rebuilt = StatusList::new(entries);
    rebuilt.offset = list.offset;
    for (index, entry) in rebuilt.entries.iter().enumerate() {
        if Some(&entry.path) == highlighted.as_ref() {
            rebuilt.highlight = Some(index);
        }
        rebuilt.checked[index] =
            entry.unsupported_reason.is_none() && checked.contains(&entry.path);
    }
    *list = rebuilt;
}

/// Whether two documents show the same rows and changes.
fn same_document(a: &Document, b: &Document) -> bool {
    a.changes == b.changes
        && a.rows.len() == b.rows.len()
        && a.rows
            .iter()
            .zip(&b.rows)
            .all(|(x, y)| x.gutter == y.gutter && x.text == y.text)
}
