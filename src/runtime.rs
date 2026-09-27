use std::fs::{self, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Instant;

use ratatui::DefaultTerminal;

use crate::app::{Action, App, Effect, Event, Job};
use crate::document::{diff_document, load_preview};
use crate::git::{GitApi, GitError};
use crate::model::{DiffEntry, FileEntry};
use crate::ui;
use crate::watcher;

/// The platform location of the first-launch marker, if one can be determined.
pub fn default_marker() -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "gitplume")?;
    let dir = dirs.state_dir().unwrap_or_else(|| dirs.data_local_dir());
    Some(dir.join("shortcuts-shown"))
}

/// Claim the first launch by creating `marker`; true when the shortcuts should show.
pub fn claim_first_launch(marker: &Path) -> bool {
    if let Some(parent) = marker.parent()
        && fs::create_dir_all(parent).is_err()
    {
        return true;
    }
    match OpenOptions::new().write(true).create_new(true).open(marker) {
        Err(error) => error.kind() != ErrorKind::AlreadyExists,
        Ok(_) => true,
    }
}

/// Run one background job to completion and return its result event.
pub fn run_job(git: &dyn GitApi, job: Job) -> Event {
    match job {
        Job::Status { root, token } => Event::Status {
            token,
            result: git.status(&root),
            failed: None,
        },
        Job::AutoStatus { root, token } => Event::AutoStatus {
            token,
            result: git.status(&root),
        },
        Job::Mutate {
            root,
            action,
            entries,
            token,
        } => {
            let failed = mutate(git, &root, action, &entries)
                .err()
                .map(|error| (action, error));
            Event::Status {
                token,
                result: git.status(&root),
                failed,
            }
        }
        Job::Diff { root, entry, token } => {
            let path = match &entry {
                DiffEntry::File(file) => &file.path,
                DiffEntry::Commit(file) => &file.path,
            };
            Event::Diff {
                token,
                result: git
                    .diff(&root, &entry)
                    .map(|patch| diff_document(path, &patch)),
            }
        }
        Job::History { root, token } => Event::History {
            token,
            result: git.commits(&root),
        },
        Job::CommitFiles {
            root,
            commit,
            token,
        } => Event::CommitFiles {
            token,
            result: git.commit_files(&root, &commit),
        },
        Job::Files { cwd, token } => Event::Files {
            token,
            result: git.files(&cwd),
        },
        Job::Preview { path, token } => Event::Preview {
            token,
            doc: load_preview(&path),
        },
    }
}

/// Apply `action`; discard restores tracked entries and cleans untracked ones.
fn mutate(
    git: &dyn GitApi,
    root: &Path,
    action: Action,
    entries: &[FileEntry],
) -> Result<(), GitError> {
    let paths = |keep: fn(&FileEntry) -> bool| -> Vec<&str> {
        entries
            .iter()
            .filter(|entry| keep(entry))
            .map(|entry| entry.path.as_str())
            .collect()
    };
    match action {
        Action::Stage => git.stage(root, &paths(|_| true)),
        Action::Unstage => git.unstage(root, &paths(|_| true)),
        Action::Discard => {
            let tracked = paths(|entry| entry.status != '?');
            let untracked = paths(|entry| entry.status == '?');
            if !tracked.is_empty() {
                git.restore(root, &tracked)?;
            }
            if !untracked.is_empty() {
                git.clean(root, &untracked)?;
            }
            Ok(())
        }
    }
}

/// Forwards terminal input to the event channel until the receiver is gone.
fn spawn_input(tx: Sender<Event>) {
    thread::spawn(move || {
        while let Ok(input) = crossterm::event::read() {
            if tx.send(Event::Input(input)).is_err() {
                break;
            }
        }
    });
}

/// The FIFO Git queue: runs jobs in order on one thread and posts their results.
pub fn spawn_git(git: Arc<dyn GitApi>, tx: Sender<Event>) -> Sender<Job> {
    let (jobs, rx) = mpsc::channel::<Job>();
    thread::spawn(move || {
        for job in rx {
            if tx.send(run_job(git.as_ref(), job)).is_err() {
                break;
            }
        }
    });
    jobs
}

/// A latest-only worker: skips to the newest queued job before running it.
fn spawn_latest(git: Arc<dyn GitApi>, tx: Sender<Event>) -> Sender<Job> {
    let (jobs, rx) = mpsc::channel::<Job>();
    thread::spawn(move || {
        while let Ok(job) = rx.recv() {
            let job = rx.try_iter().last().unwrap_or(job);
            if tx.send(run_job(git.as_ref(), job)).is_err() {
                break;
            }
        }
    });
    jobs
}

/// Job senders: the FIFO Git queue and the latest-only workers.
struct Workers {
    git: Sender<Job>,
    diff: Sender<Job>,
    history: Sender<Job>,
    commit_files: Sender<Job>,
    files: Sender<Job>,
    preview: Sender<Job>,
}

pub fn run(terminal: &mut DefaultTerminal, git: Arc<dyn GitApi>, mut app: App) -> io::Result<()> {
    let (tx, rx) = mpsc::channel();
    spawn_input(tx.clone());
    watcher::spawn(git.clone(), app.root.clone(), tx.clone());
    let jobs = Workers {
        git: spawn_git(git.clone(), tx.clone()),
        diff: spawn_latest(git.clone(), tx.clone()),
        history: spawn_latest(git.clone(), tx.clone()),
        commit_files: spawn_latest(git.clone(), tx.clone()),
        files: spawn_latest(git.clone(), tx.clone()),
        preview: spawn_latest(git, tx),
    };
    terminal.draw(|frame| ui::render(&mut app, frame))?;
    if execute(app.start(), &jobs) {
        return Ok(());
    }
    loop {
        let first = match app.deadline() {
            Some(deadline) => {
                match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(event) => event,
                    Err(RecvTimeoutError::Timeout) => Event::Tick(Instant::now()),
                    Err(RecvTimeoutError::Disconnected) => return Ok(()),
                }
            }
            None => match rx.recv() {
                Ok(event) => event,
                Err(_) => return Ok(()),
            },
        };
        if apply(&mut app, first, &rx, &jobs) {
            return Ok(());
        }
        terminal.draw(|frame| ui::render(&mut app, frame))?;
    }
}

/// Applies `first` and every queued event; returns true when the app should quit.
fn apply(app: &mut App, first: Event, rx: &Receiver<Event>, jobs: &Workers) -> bool {
    for event in std::iter::once(first).chain(rx.try_iter()) {
        if execute(app.update(event), jobs) {
            return true;
        }
    }
    // Continuous input never times out the wait, so expire toasts here too.
    app.expire_due(Instant::now());
    false
}

/// Executes effects; returns true when one of them is `Quit`.
fn execute(effects: Vec<Effect>, jobs: &Workers) -> bool {
    for effect in effects {
        match effect {
            Effect::Quit => return true,
            Effect::Git(job @ Job::Diff { .. }) => {
                let _ = jobs.diff.send(job);
            }
            Effect::Git(job @ Job::History { .. }) => {
                let _ = jobs.history.send(job);
            }
            Effect::Git(job @ Job::CommitFiles { .. }) => {
                let _ = jobs.commit_files.send(job);
            }
            Effect::Git(job @ Job::Files { .. }) => {
                let _ = jobs.files.send(job);
            }
            Effect::Git(job @ Job::Preview { .. }) => {
                let _ = jobs.preview.send(job);
            }
            Effect::Git(job) => {
                let _ = jobs.git.send(job);
            }
            Effect::Copy(text) => {
                // OSC 52: ask the terminal to set the clipboard.
                let mut out = io::stdout();
                let _ = write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes()));
                let _ = out.flush();
            }
        }
    }
    false
}

/// Standard base64 with padding.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().fold(0u32, |n, &b| n << 8 | b as u32) << (8 * (3 - chunk.len()));
        for i in 0..4 {
            out.push(if i <= chunk.len() {
                ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn base64_encodes_with_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64("sélection".as_bytes()), "c8OpbGVjdGlvbg==");
    }
}
