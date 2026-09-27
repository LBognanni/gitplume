mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use common::{FakeGit, Harness, ROOT, state};
use crossterm::event::KeyCode;
use gitplume::app::{Action, Event, Job};
use gitplume::model::{Commit, FileEntry, Side};
use gitplume::runtime;

fn status_job(token: u64) -> Job {
    Job::Status {
        root: PathBuf::from(ROOT),
        token,
    }
}

fn stage_job(path: &str, token: u64) -> Job {
    Job::Mutate {
        root: PathBuf::from(ROOT),
        action: Action::Stage,
        entries: vec![FileEntry::new(path, Side::Unstaged, 'M')],
        token,
    }
}

/// A running Git queue over a gated fake.
struct Queue {
    git: Arc<FakeGit>,
    jobs: Sender<Job>,
    events: Receiver<Event>,
    started: Receiver<String>,
    release: Sender<()>,
}

impl Queue {
    fn new() -> Self {
        let (git, started, release) = FakeGit::gated(Ok(state(&[], &[])));
        let git = Arc::new(git);
        let (tx, events) = mpsc::channel();
        let jobs = runtime::spawn_git(git.clone(), tx);
        Self {
            git,
            jobs,
            events,
            started,
            release,
        }
    }

    /// Wait for the next Git call to start and return it; it stays blocked.
    fn next_call(&self) -> String {
        self.started.recv().unwrap()
    }

    fn release(&self) {
        self.release.send(()).unwrap();
    }

    fn token(&self) -> u64 {
        match self.events.recv().unwrap() {
            Event::Status { token, .. } => token,
            other => panic!("unexpected event {other:?}"),
        }
    }
}

#[test]
fn status_refresh_runs_git_off_the_event_thread() {
    // The app only queues the read; the lists show loading meanwhile.
    let mut harness = Harness::held(Ok(state(&[], &[])));
    assert!(harness.git.calls().is_empty());
    let screen = harness.screen();
    assert_eq!(screen.matches("Loading…").count(), 2, "{screen}");

    // The queue runs the read on its own thread: this thread keeps running
    // while the read is blocked inside Git.
    let queue = Queue::new();
    queue.jobs.send(status_job(1)).unwrap();
    assert_eq!(queue.next_call(), "status /repo");
    queue.release();
    assert_eq!(queue.token(), 1);

    let mut worker = state(&[], &[]);
    worker.branch = "worker".to_string();
    *harness.git.status.lock().unwrap() = Ok(worker);
    let result = harness.run_next();
    harness.send(result);
    assert_eq!(harness.line(29).trim(), "Branch: worker");
    assert!(!harness.screen().contains("Loading…"));
}

/// Start held, queue a second read with `r`, and run both reads with
/// different results: returns (older, newer) result events.
fn two_reads(harness: &mut Harness) -> (Event, Event) {
    harness.press(KeyCode::Char('r'));
    let mut older = state(&["old.txt"], &[]);
    older.branch = "older".to_string();
    *harness.git.status.lock().unwrap() = Ok(older);
    let first = harness.run_next();
    let mut newer = state(&[], &["new.txt"]);
    newer.branch = "newer".to_string();
    *harness.git.status.lock().unwrap() = Ok(newer);
    let second = harness.run_next();
    (first, second)
}

#[test]
fn superseded_refresh_results_are_never_displayed() {
    let mut harness = Harness::held(Ok(state(&[], &[])));
    let (older, newer) = two_reads(&mut harness);

    harness.send(older);
    let screen = harness.screen();
    assert!(!screen.contains("older"), "{screen}");
    assert!(!screen.contains("old.txt"));
    assert_eq!(screen.matches("Loading…").count(), 2);

    harness.send(newer);
    let screen = harness.screen();
    assert_eq!(harness.line(29).trim(), "Branch: newer");
    assert!(screen.contains("[ ] M new.txt"));
    assert!(!screen.contains("old.txt"));
}

fn commit(hash: &str, subject: &str) -> Commit {
    Commit {
        hash: hash.to_string(),
        short_hash: hash[..7].to_string(),
        parent: None,
        subject: subject.to_string(),
    }
}

#[test]
fn superseded_history_results_are_never_displayed() {
    let mut harness = Harness::held(Ok(state(&[], &[])));
    harness.hold_history = true;
    harness.press(KeyCode::Char('r'));
    harness.press(KeyCode::Char('r'));
    *harness.git.commits.lock().unwrap() = Ok(vec![commit("aaaaaaafull", "Older commit")]);
    let older = harness.run_history();
    *harness.git.commits.lock().unwrap() = Ok(vec![commit("bbbbbbbfull", "Newer commit")]);
    let newer = harness.run_history();

    harness.send(older);
    let screen = harness.screen();
    assert!(!screen.contains("Older commit"), "{screen}");
    assert_eq!(screen.matches("Loading…").count(), 3);

    harness.send(newer);
    // With both lists empty, the first commit is selected and loads its files.
    let files = harness.run_history();
    harness.send(files);
    let screen = harness.screen();
    assert!(screen.contains("Newer commit"), "{screen}");
    assert!(!screen.contains("Older commit"));
    assert_eq!(screen.matches("Loading…").count(), 2);
}

#[test]
fn newest_result_wins_when_results_complete_out_of_order() {
    let mut harness = Harness::held(Ok(state(&[], &[])));
    let (older, newer) = two_reads(&mut harness);

    harness.send(newer);
    harness.send(older);
    let screen = harness.screen();
    assert_eq!(harness.line(29).trim(), "Branch: newer");
    assert!(screen.contains("[ ] M new.txt"));
    assert!(!screen.contains("old.txt"));
}

#[test]
fn repeated_status_refreshes_do_not_overlap() {
    let queue = Queue::new();
    queue.jobs.send(status_job(1)).unwrap();
    queue.jobs.send(status_job(2)).unwrap();
    assert_eq!(queue.next_call(), "status /repo");
    // The first read is still blocked, so the second has not started.
    assert_eq!(queue.git.calls(), ["status /repo"]);
    queue.release();
    assert_eq!(queue.token(), 1);
    assert_eq!(queue.next_call(), "status /repo");
    queue.release();
    assert_eq!(queue.token(), 2);
    assert_eq!(queue.git.calls().len(), 2);
}

#[test]
fn mutations_are_serialized_and_no_status_read_interleaves() {
    let queue = Queue::new();
    queue.jobs.send(stage_job("one.txt", 1)).unwrap();
    queue.jobs.send(status_job(2)).unwrap();
    queue.jobs.send(stage_job("two.txt", 3)).unwrap();

    assert_eq!(queue.next_call(), "stage one.txt");
    assert_eq!(queue.git.calls(), ["stage one.txt"]);
    queue.release();
    // The mutation job's own status read runs before the queued read.
    assert_eq!(queue.next_call(), "status /repo");
    queue.release();
    assert_eq!(queue.token(), 1);
    assert_eq!(queue.next_call(), "status /repo");
    queue.release();
    assert_eq!(queue.token(), 2);
    assert_eq!(queue.next_call(), "stage two.txt");
    queue.release();
    assert_eq!(queue.next_call(), "status /repo");
    queue.release();
    assert_eq!(queue.token(), 3);
    assert_eq!(
        queue.git.calls(),
        [
            "stage one.txt",
            "status /repo",
            "status /repo",
            "stage two.txt",
            "status /repo",
        ]
    );
}

#[test]
fn mutations_are_queued_in_request_order_and_the_last_result_wins() {
    let mut harness = Harness::held(Ok(state(&[], &["one.txt", "two.txt"])));
    let startup = harness.run_next();
    harness.send(startup);

    harness.press(KeyCode::Char('s'));
    harness.press(KeyCode::Down);
    harness.press(KeyCode::Char('s'));
    assert_eq!(harness.git.calls(), ["status /repo"]);
    assert_eq!(harness.screen().matches("Loading…").count(), 2);

    *harness.git.status.lock().unwrap() = Ok(state(&["one.txt"], &["two.txt"]));
    let first = harness.run_next();
    *harness.git.status.lock().unwrap() = Ok(state(&["one.txt", "two.txt"], &[]));
    let second = harness.run_next();
    assert_eq!(
        harness.git.calls()[1..],
        [
            "stage one.txt",
            "status /repo",
            "stage two.txt",
            "status /repo"
        ]
    );

    // The first mutation's read was superseded by the second request.
    harness.send(first);
    assert_eq!(harness.screen().matches("Loading…").count(), 2);
    harness.send(second);
    let screen = harness.screen();
    assert!(screen.contains("[ ] M one.txt"));
    assert!(screen.contains("[ ] M two.txt"));
    assert!(!screen.contains("Loading…"));
}

// Files half of test_superseded_refresh_results_are_never_displayed.
#[test]
fn superseded_files_results_are_never_displayed() {
    let root = std::path::Path::new("/repo");
    let mut harness = Harness::launched(root, root, &[]);
    harness.press(KeyCode::Char('2'));
    harness.hold_files = true;
    harness.press(KeyCode::Char('r'));
    harness.press(KeyCode::Char('r'));
    *harness.git.files.lock().unwrap() = Ok(vec!["older.txt".to_string()]);
    let older = harness.run_files();
    *harness.git.files.lock().unwrap() = Ok(vec!["newer.txt".to_string()]);
    let newer = harness.run_files();

    harness.send(older);
    assert!(harness.find("older.txt").is_none());
    assert!(harness.find("Loading…").is_some());
    harness.send(newer);
    assert!(harness.find("newer.txt").is_some());
    assert!(harness.find("older.txt").is_none());
}

#[test]
fn only_the_newest_preview_applies_and_refresh_drops_pending_previews() {
    let dir = common::TempDir::new("stale-preview");
    std::fs::write(dir.0.join("a.txt"), "alpha\n").unwrap();
    std::fs::write(dir.0.join("b.txt"), "bravo\n").unwrap();
    let mut harness = Harness::launched(&dir.0, &dir.0, &["a.txt", "b.txt"]);
    harness.press(KeyCode::Char('2'));
    harness.hold_files = true;
    harness.press(KeyCode::Char('j'));
    harness.press(KeyCode::Enter);
    harness.press(KeyCode::Char('j'));
    harness.press(KeyCode::Enter);
    let alpha = harness.run_files();
    let bravo = harness.run_files();
    harness.send(bravo);
    harness.send(alpha);
    assert!(harness.screen().contains("bravo"));
    assert!(!harness.screen().contains("alpha"));

    harness.press(KeyCode::Enter);
    let pending = harness.run_files();
    harness.press(KeyCode::Char('r'));
    harness.send(pending);
    assert!(!harness.screen().contains("bravo"));
}
