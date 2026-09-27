//! Workflow test harness: drives `App` against a `FakeGit`, executing effects
//! synchronously and drawing after every event, like the runtime does.
#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Instant;

use crossterm::event::{
    Event as Input, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use gitplume::app::{App, Effect, Event, Job};
use gitplume::git::{GitApi, GitError};
use gitplume::model::{Commit, CommitFile, DiffEntry, FileEntry, RepoState, Side};
use gitplume::{runtime, ui};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

pub const ROOT: &str = "/repo";

pub fn state(staged: &[&str], unstaged: &[&str]) -> RepoState {
    RepoState {
        root: PathBuf::from(ROOT),
        staged: staged
            .iter()
            .map(|path| FileEntry::new(path, Side::Staged, 'M'))
            .collect(),
        unstaged: unstaged
            .iter()
            .map(|path| FileEntry::new(path, Side::Unstaged, 'M'))
            .collect(),
        branch: "main".to_string(),
    }
}

/// A failed Git command with `stderr`.
pub fn failure(stderr: &str) -> GitError {
    GitError::Failed {
        code: Some(128),
        stderr: stderr.to_string(),
    }
}

/// Lets a test pause the fake inside each call: the fake reports the call on
/// `started`, then waits for one message on `release`.
struct Gate {
    started: Sender<String>,
    release: Mutex<Receiver<()>>,
}

/// A `GitApi` returning canned results and recording calls as `"<name> <args>"`.
pub struct FakeGit {
    pub status: Mutex<Result<RepoState, GitError>>,
    /// When set, every mutation fails with this error.
    pub mutation_error: Mutex<Option<GitError>>,
    /// Patch text (or error) per path; missing paths have an empty diff.
    pub diffs: Mutex<HashMap<String, Result<String, GitError>>>,
    pub commits: Mutex<Result<Vec<Commit>, GitError>>,
    /// Files per commit hash; missing commits have no files.
    pub commit_files: Mutex<HashMap<String, Vec<CommitFile>>>,
    /// Paths listed by `files`, relative to the launch directory.
    pub files: Mutex<Result<Vec<String>, GitError>>,
    pub calls: Mutex<Vec<String>>,
    gate: Option<Gate>,
}

impl FakeGit {
    pub fn new(status: Result<RepoState, GitError>) -> Self {
        Self {
            status: Mutex::new(status),
            mutation_error: Mutex::new(None),
            diffs: Mutex::new(HashMap::new()),
            commits: Mutex::new(Ok(Vec::new())),
            commit_files: Mutex::new(HashMap::new()),
            files: Mutex::new(Ok(Vec::new())),
            calls: Mutex::new(Vec::new()),
            gate: None,
        }
    }

    /// A fake that blocks in every call until released; returns the fake, the
    /// receiver of started calls, and the release sender.
    pub fn gated(status: Result<RepoState, GitError>) -> (Self, Receiver<String>, Sender<()>) {
        let (started, started_rx) = mpsc::channel();
        let (release_tx, release) = mpsc::channel();
        let fake = Self {
            gate: Some(Gate {
                started,
                release: Mutex::new(release),
            }),
            ..Self::new(status)
        };
        (fake, started_rx, release_tx)
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn record(&self, call: String) {
        self.calls.lock().unwrap().push(call.clone());
        if let Some(gate) = &self.gate {
            gate.started.send(call).unwrap();
            gate.release.lock().unwrap().recv().unwrap();
        }
    }

    /// Serve `patch` as the diff of `path`.
    pub fn set_diff(&self, path: &str, patch: Result<String, GitError>) {
        self.diffs.lock().unwrap().insert(path.to_string(), patch);
    }

    fn mutation(&self, name: &str, paths: &[&str]) -> Result<(), GitError> {
        self.record(format!("{name} {}", paths.join(" ")));
        match self.mutation_error.lock().unwrap().clone() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl GitApi for FakeGit {
    fn status(&self, root: &Path) -> Result<RepoState, GitError> {
        self.record(format!("status {}", root.display()));
        self.status.lock().unwrap().clone()
    }
    fn files(&self, cwd: &Path) -> Result<Vec<String>, GitError> {
        self.record(format!("files {}", cwd.display()));
        self.files.lock().unwrap().clone()
    }
    fn commits(&self, _: &Path) -> Result<Vec<Commit>, GitError> {
        self.commits.lock().unwrap().clone()
    }
    fn commit_files(&self, _: &Path, commit: &Commit) -> Result<Vec<CommitFile>, GitError> {
        self.record(format!("commit_files {}", commit.hash));
        let files = self.commit_files.lock().unwrap();
        Ok(files.get(&commit.hash).cloned().unwrap_or_default())
    }
    fn diff(&self, _: &Path, entry: &DiffEntry) -> Result<String, GitError> {
        let path = match entry {
            DiffEntry::File(file) => &file.path,
            DiffEntry::Commit(file) => &file.path,
        };
        self.record(format!("diff {path}"));
        let diffs = self.diffs.lock().unwrap();
        diffs.get(path).cloned().unwrap_or(Ok(String::new()))
    }
    fn stage(&self, _: &Path, paths: &[&str]) -> Result<(), GitError> {
        self.mutation("stage", paths)
    }
    fn unstage(&self, _: &Path, paths: &[&str]) -> Result<(), GitError> {
        self.mutation("unstage", paths)
    }
    fn restore(&self, _: &Path, paths: &[&str]) -> Result<(), GitError> {
        self.mutation("restore", paths)
    }
    fn clean(&self, _: &Path, paths: &[&str]) -> Result<(), GitError> {
        self.mutation("clean", paths)
    }
    fn git_dirs(&self, root: &Path) -> Result<(PathBuf, PathBuf), GitError> {
        Ok((root.join(".git"), root.join(".git")))
    }
}

pub struct Harness {
    pub app: App,
    pub git: FakeGit,
    pub quit: bool,
    /// When true, Git jobs wait in `held` until the test runs them.
    pub hold: bool,
    pub held: VecDeque<Job>,
    /// When true, history and commit-file jobs wait in `held_history`.
    pub hold_history: bool,
    /// Held history and commit-file jobs, kept apart from the Git queue's.
    pub held_history: VecDeque<Job>,
    /// When true, file-list and preview jobs wait in `held_files`.
    pub hold_files: bool,
    pub held_files: VecDeque<Job>,
    /// Text copied to the clipboard, in order.
    pub copied: Vec<String>,
    terminal: Terminal<TestBackend>,
}

impl Harness {
    /// Start the app on `status` at 100x30 without the first-launch dialog.
    pub fn new(status: Result<RepoState, GitError>) -> Self {
        Self::with(status, false, 100, 30)
    }

    pub fn with(
        status: Result<RepoState, GitError>,
        show_shortcuts: bool,
        width: u16,
        height: u16,
    ) -> Self {
        Self::start(status, show_shortcuts, width, height, false)
    }

    /// Like `new`, but every Git job, including the startup read, is held.
    pub fn held(status: Result<RepoState, GitError>) -> Self {
        Self::start(status, false, 100, 30, true)
    }

    /// Start at 100x30 in repository `root`, launched from `cwd`, serving `files`.
    pub fn launched(root: &Path, cwd: &Path, files: &[&str]) -> Self {
        let git = FakeGit::new(Ok(RepoState {
            root: root.to_path_buf(),
            ..state(&[], &[])
        }));
        *git.files.lock().unwrap() = Ok(files.iter().map(|f| f.to_string()).collect());
        let app = App::new(root.to_path_buf(), cwd.to_path_buf(), false);
        Self::launch(app, git, 100, 30, false)
    }

    fn start(
        status: Result<RepoState, GitError>,
        show_shortcuts: bool,
        width: u16,
        height: u16,
        hold: bool,
    ) -> Self {
        let app = App::new(PathBuf::from(ROOT), PathBuf::from(ROOT), show_shortcuts);
        Self::launch(app, FakeGit::new(status), width, height, hold)
    }

    fn launch(app: App, git: FakeGit, width: u16, height: u16, hold: bool) -> Self {
        let mut harness = Self {
            app,
            git,
            quit: false,
            hold,
            held: VecDeque::new(),
            hold_history: false,
            held_history: VecDeque::new(),
            hold_files: hold,
            held_files: VecDeque::new(),
            copied: Vec::new(),
            terminal: Terminal::new(TestBackend::new(width, height)).unwrap(),
        };
        harness.draw();
        let effects = harness.app.start();
        harness.execute(effects);
        harness.draw();
        harness
    }

    fn execute(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Quit => self.quit = true,
                Effect::Copy(text) => self.copied.push(text),
                Effect::Git(job @ (Job::History { .. } | Job::CommitFiles { .. }))
                    if self.hold_history =>
                {
                    self.held_history.push_back(job)
                }
                Effect::Git(job @ (Job::Files { .. } | Job::Preview { .. })) if self.hold_files => {
                    self.held_files.push_back(job)
                }
                Effect::Git(job)
                    if self.hold
                        && !matches!(
                            job,
                            Job::History { .. }
                                | Job::CommitFiles { .. }
                                | Job::Files { .. }
                                | Job::Preview { .. }
                        ) =>
                {
                    self.held.push_back(job)
                }
                Effect::Git(job) => {
                    let event = runtime::run_job(&self.git, job);
                    let effects = self.app.update(event);
                    self.execute(effects);
                }
            }
        }
    }

    /// Run the oldest held job, like the Git queue, and return its result
    /// event without applying it, so the test chooses the delivery order.
    pub fn run_next(&mut self) -> Event {
        let job = self.held.pop_front().expect("a held Git job");
        runtime::run_job(&self.git, job)
    }

    /// Run the oldest held history or commit-files job and return its result event.
    pub fn run_history(&mut self) -> Event {
        let job = self.held_history.pop_front().expect("a held history job");
        runtime::run_job(&self.git, job)
    }

    /// Run the oldest held file-list or preview job and return its result event.
    pub fn run_files(&mut self) -> Event {
        let job = self.held_files.pop_front().expect("a held files job");
        runtime::run_job(&self.git, job)
    }

    /// Apply `event`, execute its effects, and redraw.
    pub fn send(&mut self, event: Event) {
        let effects = self.app.update(event);
        self.execute(effects);
        // Mirror the runtime: expire due toasts after draining, before drawing.
        self.app.expire_due(Instant::now());
        self.draw();
    }

    pub fn press(&mut self, code: KeyCode) {
        self.press_with(code, KeyModifiers::NONE);
    }

    pub fn press_with(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        self.send(Event::Input(Input::Key(KeyEvent::new(code, modifiers))));
    }

    fn mouse(&mut self, kind: MouseEventKind, column: u16, row: u16) {
        self.send(Event::Input(Input::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        })));
    }

    pub fn click(&mut self, (column, row): (u16, u16)) {
        self.mouse(MouseEventKind::Down(MouseButton::Left), column, row);
    }

    pub fn hover(&mut self, (column, row): (u16, u16)) {
        self.mouse(MouseEventKind::Moved, column, row);
    }

    /// Press on `from`, drag to `to`, and release there.
    pub fn drag(&mut self, from: (u16, u16), to: (u16, u16)) {
        self.mouse(MouseEventKind::Down(MouseButton::Left), from.0, from.1);
        self.mouse(MouseEventKind::Drag(MouseButton::Left), to.0, to.1);
        self.mouse(MouseEventKind::Up(MouseButton::Left), to.0, to.1);
    }

    pub fn mouse_down(&mut self, (column, row): (u16, u16)) {
        self.mouse(MouseEventKind::Down(MouseButton::Left), column, row);
    }

    /// Move the pointer with the left button held.
    pub fn drag_to(&mut self, (column, row): (u16, u16)) {
        self.mouse(MouseEventKind::Drag(MouseButton::Left), column, row);
    }

    pub fn scroll(&mut self, kind: MouseEventKind, (column, row): (u16, u16)) {
        self.mouse(kind, column, row);
    }

    pub fn mouse_up(&mut self, (column, row): (u16, u16)) {
        self.mouse(MouseEventKind::Up(MouseButton::Left), column, row);
    }

    /// Resize the terminal and deliver the resize event.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.terminal.backend_mut().resize(width, height);
        self.send(Event::Input(Input::Resize(width, height)));
    }

    pub fn draw(&mut self) -> Buffer {
        let app = &mut self.app;
        self.terminal.draw(|frame| ui::render(app, frame)).unwrap();
        self.buffer()
    }

    pub fn buffer(&self) -> Buffer {
        self.terminal.backend().buffer().clone()
    }

    pub fn line(&self, y: u16) -> String {
        let buffer = self.buffer();
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    }

    pub fn screen(&self) -> String {
        let height = self.buffer().area.height;
        (0..height)
            .map(|y| self.line(y))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Cell position of the first occurrence of `text`, searching row by row.
    pub fn find(&self, text: &str) -> Option<(u16, u16)> {
        let buffer = self.buffer();
        (0..buffer.area.height).find_map(|y| {
            let cells: Vec<&str> = (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect();
            let needle: Vec<String> = text.chars().map(String::from).collect();
            cells
                .windows(needle.len())
                .position(|window| window.iter().zip(&needle).all(|(a, b)| *a == b))
                .map(|x| (x as u16, y))
        })
    }

    pub fn at(&self, text: &str) -> (u16, u16) {
        self.find(text)
            .unwrap_or_else(|| panic!("{text:?} not on screen:\n{}", self.screen()))
    }
}

/// A unique temporary directory removed when dropped.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("gitplume-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
