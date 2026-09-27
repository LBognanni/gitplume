mod common;

use std::path::PathBuf;

use common::{Harness, ROOT, failure};
use crossterm::event::KeyCode;
use gitplume::app::{Event, Focus, Job, MISSING, Severity};
use gitplume::model::{Commit, CommitFile, FileEntry, RepoState, Side};
use gitplume::runtime;
use gitplume::watcher::{Invalidation, Watch};
use ratatui::style::Modifier;

/// Paths whose diffs the fake serves.
const PATHS: [&str; 5] = ["a.txt", "b.txt", "c.txt", "s.txt", "x.txt"];
/// First column of the diff pane.
const PANE_X: usize = 31;

fn repo(staged: &[(&str, char)], unstaged: &[(&str, char)], branch: &str) -> RepoState {
    let entries = |side, list: &[(&str, char)]| {
        list.iter()
            .map(|(path, status)| FileEntry::new(path, side, *status))
            .collect()
    };
    RepoState {
        root: PathBuf::from(ROOT),
        staged: entries(Side::Staged, staged),
        unstaged: entries(Side::Unstaged, unstaged),
        branch: branch.to_string(),
    }
}

const ABC: [(&str, char); 3] = [("a.txt", 'M'), ("b.txt", 'M'), ("c.txt", 'M')];

/// A realistic 20-line patch of `path` whose changed line names `version`.
fn patch(path: &str, version: u32) -> String {
    let mut body: Vec<String> = (0..20).map(|i| format!(" context {i}")).collect();
    body[3] = format!("-old\n+{path} version {version}");
    format!(
        "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -1,20 +1,20 @@\n{}\n",
        body.join("\n")
    )
}

fn set_version(harness: &Harness, version: u32) {
    for path in PATHS {
        harness.git.set_diff(path, Ok(patch(path, version)));
    }
}

fn set_status(harness: &Harness, state: RepoState) {
    *harness.git.status.lock().unwrap() = Ok(state);
}

/// Start on `state`, serving version 1 of every diff.
fn start(state: RepoState) -> Harness {
    let mut harness = Harness::new(Ok(state));
    set_version(&harness, 1);
    harness.draw();
    harness
}

fn invalidation(history: bool, index_changed: bool, paths: &[&str]) -> Invalidation {
    Invalidation {
        status: true,
        history,
        changed_paths: paths.iter().map(|p| p.to_string()).collect(),
        index_changed,
    }
}

fn edit(path: &str) -> Invalidation {
    invalidation(false, false, &[path])
}

fn watch(harness: &mut Harness, invalidation: Invalidation) {
    harness.send(Event::Watch(Watch::Changed(invalidation)));
}

/// Highlight b.txt (row 1 of Unstaged), check it, and open its diff.
fn select_checked_b(harness: &mut Harness) {
    assert_eq!(harness.app.focus, Focus::Unstaged);
    harness.press(KeyCode::Char('j'));
    harness.press(KeyCode::Char(' '));
    harness.press(KeyCode::Enter);
}

fn highlighted(harness: &Harness, side: Side) -> Option<String> {
    let list = harness.app.list(side);
    list.highlight.map(|index| list.entries[index].path.clone())
}

fn has(harness: &Harness, text: &str) -> bool {
    harness.screen().contains(text)
}

/// Visible text of the diff pane, title row included.
fn diff_text(harness: &Harness) -> String {
    let height = harness.buffer().area.height;
    (1..height - 1)
        .map(|y| harness.line(y).chars().skip(PANE_X).collect())
        .collect::<Vec<String>>()
        .join("\n")
}

fn warnings(harness: &Harness) -> Vec<String> {
    harness
        .app
        .toasts
        .iter()
        .filter(|toast| toast.severity == Severity::Warning && toast.title.is_none())
        .map(|toast| toast.body.clone())
        .collect()
}

fn status_calls(harness: &Harness) -> usize {
    harness
        .git
        .calls()
        .iter()
        .filter(|call| call.starts_with("status"))
        .count()
}

fn held_auto_reads(harness: &Harness) -> usize {
    harness
        .held
        .iter()
        .filter(|job| matches!(job, Job::AutoStatus { .. }))
        .count()
}

/// Run the oldest held Git job and deliver its result.
fn complete_next(harness: &mut Harness) {
    let event = harness.run_next();
    harness.send(event);
}

// Automatic reads and their ordering

#[test]
fn burst_during_read_produces_one_follow_up() {
    let mut harness = start(repo(&[], &ABC, "startup"));
    harness.hold = true;
    watch(&mut harness, edit("a.txt"));
    assert_eq!(held_auto_reads(&harness), 1);
    for _ in 0..5 {
        watch(&mut harness, edit("a.txt"));
    }
    assert_eq!(held_auto_reads(&harness), 1);

    set_status(&harness, repo(&[], &ABC, "first"));
    complete_next(&mut harness);
    assert_eq!(held_auto_reads(&harness), 1, "one follow-up for the burst");
    set_status(&harness, repo(&[], &ABC, "follow-up"));
    complete_next(&mut harness);
    assert!(harness.held.is_empty());
    assert!(has(&harness, "Branch: follow-up"));
    assert_eq!(status_calls(&harness), 3);
}

#[test]
fn events_without_status_invalidation_do_not_refresh() {
    let mut harness = start(repo(&[], &ABC, "main"));
    watch(&mut harness, Invalidation::default());
    watch(
        &mut harness,
        Invalidation {
            history: true,
            ..Invalidation::default()
        },
    );
    assert_eq!(status_calls(&harness), 1);
}

#[test]
fn manual_refresh_supersedes_older_automatic_result() {
    let mut harness = start(repo(&[], &ABC, "startup"));
    harness.hold = true;
    watch(&mut harness, edit("a.txt"));
    harness.press(KeyCode::Char('r'));

    set_status(&harness, repo(&[], &ABC, "automatic"));
    complete_next(&mut harness);
    assert!(!has(&harness, "Branch: automatic"));
    set_status(&harness, repo(&[], &ABC, "manual"));
    complete_next(&mut harness);
    assert!(has(&harness, "Branch: manual"));
}

#[test]
fn mutation_is_not_interleaved_with_automatic_status() {
    let mut harness = start(repo(&[], &ABC, "main"));
    let start = harness.git.calls().len();
    harness.hold = true;
    watch(&mut harness, edit("a.txt"));
    harness.press(KeyCode::Char('s'));
    while !harness.held.is_empty() {
        complete_next(&mut harness);
    }
    let calls: Vec<String> = harness.git.calls()[start..]
        .iter()
        .filter(|call| !call.starts_with("diff"))
        .cloned()
        .collect();
    assert_eq!(calls[..3], ["status /repo", "stage a.txt", "status /repo"]);
}

#[test]
fn invalidation_survives_a_superseded_automatic_read() {
    let mut harness = start(repo(&[], &ABC, "main"));
    harness.hold = true;
    harness.hold_history = true;
    harness.press(KeyCode::Char('r'));
    watch(&mut harness, invalidation(true, false, &[]));
    harness.press(KeyCode::Char('r'));
    // First manual read (stale), automatic read (stale, retried), second manual read.
    for _ in 0..3 {
        complete_next(&mut harness);
    }
    harness.held_history.clear();
    complete_next(&mut harness);
    assert!(harness.held.is_empty());
    assert_eq!(
        harness.held_history.len(),
        1,
        "the retried read kept `history`"
    );
}

#[test]
fn invalidation_survives_a_failed_automatic_read() {
    let mut harness = start(repo(&[], &ABC, "main"));
    harness.hold_history = true;
    *harness.git.status.lock().unwrap() = Err(failure("fatal: status failed"));
    watch(&mut harness, invalidation(true, false, &[]));
    assert_eq!(status_calls(&harness), 2);
    assert!(harness.held_history.is_empty());

    set_status(&harness, repo(&[], &ABC, "main"));
    watch(&mut harness, edit("b.txt"));
    assert_eq!(status_calls(&harness), 3);
    assert_eq!(
        harness.held_history.len(),
        1,
        "the failed `history` was kept"
    );
}

#[test]
fn automatic_failure_is_reported_with_its_reason_and_retried_on_the_next_event() {
    let mut harness = start(repo(&[], &ABC, "main"));
    *harness.git.status.lock().unwrap() = Err(failure("fatal: boom"));
    watch(&mut harness, edit("a.txt"));
    assert_eq!(
        warnings(&harness),
        ["Automatic refresh failed: fatal: boom. Press r to refresh manually."]
    );
    assert_eq!(status_calls(&harness), 2, "no retry until the next event");

    set_status(&harness, repo(&[], &ABC, "recovered"));
    watch(&mut harness, edit("a.txt"));
    assert!(has(&harness, "Branch: recovered"));
}

#[test]
fn automatic_request_does_not_supersede_in_flight_manual_refresh() {
    let mut harness = start(repo(&[], &ABC, "startup"));
    harness.hold = true;
    harness.press(KeyCode::Char('r'));
    watch(&mut harness, edit("a.txt"));

    set_status(&harness, repo(&[], &ABC, "manual"));
    complete_next(&mut harness);
    assert!(has(&harness, "Branch: manual"));
    assert!(!has(&harness, "Loading…"));

    set_status(&harness, repo(&[], &ABC, "automatic"));
    complete_next(&mut harness);
    assert!(has(&harness, "Branch: automatic"));
}

#[test]
fn superseded_automatic_apply_does_not_reset_failure_streak() {
    let mut harness = start(repo(&[], &ABC, "main"));
    *harness.git.status.lock().unwrap() = Err(failure("fatal: failed"));
    watch(&mut harness, edit("a.txt"));
    assert_eq!(warnings(&harness).len(), 1);

    harness.hold = true;
    set_status(&harness, repo(&[], &ABC, "main"));
    watch(&mut harness, edit("a.txt"));
    harness.press(KeyCode::Char('r'));
    complete_next(&mut harness); // automatic, superseded: retried
    complete_next(&mut harness); // manual, applied
    *harness.git.status.lock().unwrap() = Err(failure("fatal: failed"));
    complete_next(&mut harness); // retried automatic read fails again
    assert!(harness.held.is_empty());
    assert_eq!(warnings(&harness).len(), 1);
}

// Watcher lifecycle

#[test]
fn watcher_setup_failure_notifies_once_and_manual_refresh_works() {
    let mut harness = start(repo(&[], &ABC, "main"));
    harness.send(Event::Watch(Watch::Stopped));
    assert_eq!(
        warnings(&harness),
        ["Automatic refresh could not start. Press r to refresh manually."]
    );
    set_status(&harness, repo(&[], &ABC, "manual"));
    harness.press(KeyCode::Char('r'));
    assert!(has(&harness, "Branch: manual"));
    assert_eq!(warnings(&harness).len(), 1);
}

#[test]
fn watcher_stopping_after_startup_notifies_once() {
    let mut harness = start(repo(&[], &ABC, "main"));
    harness.send(Event::Watch(Watch::Started));
    watch(&mut harness, edit("a.txt"));
    harness.send(Event::Watch(Watch::Stopped));
    assert_eq!(
        warnings(&harness),
        ["Automatic refresh stopped. Press r to refresh manually."]
    );
}

// Quiet apply

#[test]
fn equal_status_leaves_lists_selection_focus_and_diff_untouched() {
    let mut harness = start(repo(&[], &ABC, "main"));
    select_checked_b(&mut harness);
    assert!(has(&harness, "b.txt version 1"));

    set_version(&harness, 2);
    watch(&mut harness, edit("z"));

    assert!(has(&harness, "[x] M b.txt"));
    assert_eq!(
        highlighted(&harness, Side::Unstaged).as_deref(),
        Some("b.txt")
    );
    assert_eq!(harness.app.focus, Focus::Unstaged);
    assert!(!has(&harness, "Loading…"));
    assert!(has(&harness, "b.txt version 1"));
}

#[test]
fn changed_status_updates_lists_and_restores_surviving_state() {
    let mut harness = start(repo(&[], &ABC, "main"));
    select_checked_b(&mut harness);
    harness.press(KeyCode::Char('j'));
    harness.press(KeyCode::Char(' '));
    harness.press(KeyCode::Char('k'));
    for label in ["[ ] M a.txt", "[x] M b.txt", "[x] M c.txt"] {
        assert!(has(&harness, label), "{label}");
    }

    // a.txt is staged externally, c.txt disappears, d.txt is new,
    // and b.txt changes from M to D.
    set_status(
        &harness,
        repo(&[("a.txt", 'M')], &[("b.txt", 'D'), ("d.txt", '?')], "main"),
    );
    watch(&mut harness, edit("b.txt"));

    for label in ["[ ] M a.txt", "[x] D b.txt", "[ ] ? d.txt"] {
        assert!(has(&harness, label), "{label}\n{}", harness.screen());
    }
    assert!(!has(&harness, "c.txt"));
    assert_eq!(
        highlighted(&harness, Side::Unstaged).as_deref(),
        Some("b.txt")
    );
    assert_eq!(highlighted(&harness, Side::Staged), None);
    assert_eq!(harness.app.focus, Focus::Unstaged);
    assert!(harness.app.list(Side::Unstaged).any_checked());
    assert!(!has(&harness, "Loading…"));
}

#[test]
fn background_refresh_does_not_steal_focus_or_select_first_entry() {
    let mut harness = start(repo(&[], &ABC, "main"));
    harness.press(KeyCode::Char('j'));
    harness.press(KeyCode::Tab);
    assert_eq!(harness.app.focus, Focus::Commits);

    set_status(&harness, repo(&[("x.txt", 'M')], &ABC, "main"));
    watch(&mut harness, edit("x.txt"));

    assert!(has(&harness, "[ ] M x.txt"));
    assert_eq!(harness.app.focus, Focus::Commits);
    assert_eq!(highlighted(&harness, Side::Staged), None);
    assert_eq!(
        highlighted(&harness, Side::Unstaged).as_deref(),
        Some("b.txt")
    );
}

#[test]
fn changed_status_keeps_restored_highlight_visible_in_scrolled_list() {
    let names: Vec<String> = (0..40).map(|i| format!("f{i:02}.txt")).collect();
    let all: Vec<(&str, char)> = names.iter().map(|n| (n.as_str(), 'M')).collect();
    let mut harness = start(repo(&[], &all, "main"));
    for _ in 0..30 {
        harness.press(KeyCode::Char('j'));
    }
    assert!(has(&harness, "f30.txt"));
    assert!(!has(&harness, "f00.txt"), "the list scrolled");

    set_status(&harness, repo(&[], &all[10..], "main"));
    watch(&mut harness, edit("f00.txt"));

    assert_eq!(
        highlighted(&harness, Side::Unstaged).as_deref(),
        Some("f30.txt")
    );
    assert!(has(&harness, "f30.txt"), "{}", harness.screen());
}

// Open diff reconciliation

#[test]
fn editing_the_selected_path_reloads_its_diff_only() {
    let mut harness = start(repo(&[], &ABC, "main"));
    select_checked_b(&mut harness);
    set_version(&harness, 2);

    watch(&mut harness, edit("a.txt"));
    assert!(has(&harness, "b.txt version 1"));

    watch(&mut harness, edit("b.txt"));
    assert!(has(&harness, "b.txt version 2"));
    assert!(!has(&harness, "Loading…"));
    assert!(has(&harness, "[x] M b.txt"));
    assert_eq!(
        highlighted(&harness, Side::Unstaged).as_deref(),
        Some("b.txt")
    );
}

#[test]
fn index_change_reloads_selected_staged_diff_only() {
    let mut harness = start(repo(&[("s.txt", 'M')], &ABC, "main"));
    harness.press(KeyCode::Enter);
    assert!(has(&harness, "s.txt version 1"));

    set_version(&harness, 2);
    watch(&mut harness, edit("s.txt"));
    assert!(has(&harness, "s.txt version 1"));

    watch(&mut harness, invalidation(false, true, &[]));
    assert!(has(&harness, "s.txt version 2"));
}

#[test]
fn soft_reset_history_change_reloads_selected_staged_diff() {
    let mut harness = start(repo(&[("s.txt", 'M')], &ABC, "main"));
    harness.press(KeyCode::Enter);
    set_version(&harness, 2);
    watch(&mut harness, invalidation(true, false, &[]));
    assert!(has(&harness, "s.txt version 2"));
}

#[test]
fn index_change_reloads_a_tracked_unstaged_diff() {
    let mut harness = start(repo(&[], &ABC, "main"));
    select_checked_b(&mut harness);
    set_version(&harness, 2);
    watch(&mut harness, invalidation(false, true, &[]));
    assert!(has(&harness, "b.txt version 2"));
    assert_eq!(
        highlighted(&harness, Side::Unstaged).as_deref(),
        Some("b.txt")
    );
}

#[test]
fn index_change_does_not_reload_an_untracked_selection() {
    let mut harness = start(repo(
        &[],
        &[("a.txt", 'M'), ("b.txt", '?'), ("c.txt", 'M')],
        "main",
    ));
    select_checked_b(&mut harness);
    assert!(has(&harness, "b.txt version 1"));
    set_version(&harness, 2);
    watch(&mut harness, invalidation(false, true, &[]));
    assert!(has(&harness, "b.txt version 1"));
}

#[test]
fn committed_selection_is_cleared_without_selecting_a_replacement() {
    let mut harness = start(repo(&[("b.txt", 'M')], &ABC, "main"));
    harness.press(KeyCode::Enter);
    assert!(has(&harness, "b.txt version 1"));

    // Committed: the staged entry is gone; the unstaged b.txt remains.
    set_status(&harness, repo(&[], &ABC, "main"));
    watch(&mut harness, invalidation(true, true, &[]));

    let text = diff_text(&harness);
    assert!(text.contains(MISSING), "{text}");
    assert!(!text.contains("version"));
    assert!(!text.contains("b.txt"), "the title is cleared");
    assert_eq!(harness.app.selection, None);
}

#[test]
fn surviving_selection_stays_after_status_changes() {
    let mut harness = start(repo(&[], &ABC, "main"));
    select_checked_b(&mut harness);
    set_status(
        &harness,
        repo(&[], &[("b.txt", 'D'), ("d.txt", '?')], "main"),
    );
    watch(&mut harness, edit("a.txt"));
    assert!(has(&harness, "b.txt version 1"));
    assert!(!has(&harness, MISSING));
}

#[test]
fn superseded_diff_result_cannot_replace_reconciled_state() {
    let mut harness = start(repo(&[], &ABC, "main"));
    select_checked_b(&mut harness);
    harness.hold = true;
    set_version(&harness, 2);
    watch(&mut harness, edit("b.txt"));
    complete_next(&mut harness); // the automatic read queues a quiet diff reload
    assert!(matches!(harness.held.front(), Some(Job::Diff { .. })));
    let reload = harness.run_next();

    // The change is committed while the reload is still in flight.
    set_status(
        &harness,
        repo(&[], &[("a.txt", 'M'), ("c.txt", 'M')], "main"),
    );
    watch(&mut harness, edit("x"));
    complete_next(&mut harness);
    assert!(has(&harness, MISSING));

    harness.send(reload);
    assert!(has(&harness, MISSING));
    assert!(!has(&harness, "version"));
}

#[test]
fn quiet_reload_superseding_a_normal_request_clears_loading() {
    let mut harness = start(repo(&[], &ABC, "main"));
    harness.hold = true;
    harness.press(KeyCode::Enter);
    assert!(diff_text(&harness).contains("Loading…"));
    let normal = harness.run_next();

    set_version(&harness, 2);
    watch(&mut harness, edit("a.txt"));
    complete_next(&mut harness); // the automatic read
    assert!(!diff_text(&harness).contains("Loading…"));
    complete_next(&mut harness); // the quiet reload
    harness.send(normal); // the older normal load arrives late
    assert!(!diff_text(&harness).contains("Loading…"));
    assert!(has(&harness, "a.txt version 2"));
}

#[test]
fn quiet_reload_does_not_show_loading_while_in_flight() {
    let mut harness = start(repo(&[], &ABC, "main"));
    select_checked_b(&mut harness);
    harness.hold = true;
    watch(&mut harness, edit("b.txt"));
    complete_next(&mut harness);
    assert!(matches!(harness.held.front(), Some(Job::Diff { .. })));
    assert!(!has(&harness, "Loading…"));
    assert!(has(&harness, "b.txt version 1"));
}

// Failure streaks

#[test]
fn automatic_failure_notifies_once_per_streak_and_keeps_last_state() {
    let mut harness = start(repo(&[], &ABC, "main"));
    select_checked_b(&mut harness);
    let before = harness.screen();

    *harness.git.status.lock().unwrap() = Err(failure("git failed"));
    watch(&mut harness, edit("b.txt"));
    watch(&mut harness, edit("b.txt"));
    let messages = warnings(&harness);
    assert_eq!(messages.len(), 1);
    assert!(messages[0].contains("Automatic refresh failed"));
    assert!(messages[0].contains("Press r"));
    assert!(has(&harness, "[x] M b.txt"));
    assert!(has(&harness, "b.txt version 1"));

    set_status(&harness, repo(&[], &ABC, "main"));
    watch(&mut harness, edit("b.txt"));
    assert_eq!(warnings(&harness).len(), 1);

    *harness.git.status.lock().unwrap() = Err(failure("git failed"));
    watch(&mut harness, edit("b.txt"));
    assert_eq!(warnings(&harness).len(), 2);
    let sidebar = |screen: &str| -> Vec<String> {
        screen
            .lines()
            .map(|line| line.chars().take(PANE_X).collect())
            .filter(|line: &String| line.contains(".txt"))
            .collect()
    };
    assert_eq!(sidebar(&harness.screen()), sidebar(&before));
}

fn commit(hash: &str, subject: &str, parent: Option<&str>) -> Commit {
    Commit {
        hash: hash.to_string(),
        short_hash: hash[..7].to_string(),
        parent: parent.map(str::to_string),
        subject: subject.to_string(),
    }
}

fn first() -> Commit {
    commit(&"a".repeat(40), "first", None)
}

fn second() -> Commit {
    commit(&"b".repeat(40), "second", Some(&"a".repeat(40)))
}

fn third() -> Commit {
    commit(&"c".repeat(40), "third", Some(&"b".repeat(40)))
}

fn set_commits(harness: &Harness, commits: &[Commit]) {
    *harness.git.commits.lock().unwrap() = Ok(commits.to_vec());
}

/// Start with `commits` in history and b.txt selected and checked.
fn start_with_history(commits: &[Commit]) -> Harness {
    let mut harness = start(repo(&[], &ABC, "main"));
    set_commits(&harness, commits);
    harness.press(KeyCode::Char('r'));
    select_checked_b(&mut harness);
    harness
}

fn bold(harness: &Harness, text: &str) -> bool {
    harness.buffer()[harness.at(text)]
        .modifier
        .contains(Modifier::BOLD)
}

#[test]
fn quiet_history_failure_warns_once_and_keeps_last_state() {
    let mut harness = start_with_history(&[first()]);
    *harness.git.commits.lock().unwrap() = Err(failure("fatal: log failed"));
    for _ in 0..2 {
        watch(&mut harness, invalidation(true, false, &[]));
    }
    assert_eq!(
        warnings(&harness),
        ["Automatic refresh failed: fatal: log failed. Press r to refresh manually."]
    );
    assert!(
        harness
            .app
            .toasts
            .iter()
            .all(|t| t.severity == Severity::Warning)
    );
    assert!(has(&harness, "first"));
    assert!(!has(&harness, "Loading…"));
    assert!(has(&harness, "[x] M b.txt"));
}

#[test]
fn ref_change_refreshes_history_without_moving_focus_or_selecting() {
    let mut harness = start_with_history(&[first()]);
    assert_eq!(harness.app.focus, Focus::Unstaged);

    set_commits(&harness, &[second(), first()]);
    watch(&mut harness, invalidation(true, false, &[]));

    let (_, second_row) = harness.at("second");
    let (_, first_row) = harness.at("first");
    assert!(second_row < first_row);
    assert!(!has(&harness, "Loading…"));
    assert_eq!(harness.app.focus, Focus::Unstaged);
    assert_eq!(harness.app.commits.cursor, None);
    assert!(has(&harness, "b.txt version 1"));
}

#[test]
fn ordinary_edit_does_not_refresh_history() {
    let mut harness = start(repo(&[], &ABC, "main"));
    select_checked_b(&mut harness);
    harness.hold_history = true;
    watch(&mut harness, edit("b.txt"));
    assert!(harness.held_history.is_empty());
}

#[test]
fn unrelated_ref_event_keeps_expanded_commit_files_and_cursor() {
    let mut harness = start(repo(&[], &ABC, "main"));
    let file = CommitFile {
        path: "x.txt".to_string(),
        status: 'M',
        commit_hash: second().hash,
        parent: second().parent,
    };
    harness
        .git
        .commit_files
        .lock()
        .unwrap()
        .insert(second().hash, vec![file]);
    set_commits(&harness, &[second(), first()]);
    harness.press(KeyCode::Char('r'));
    harness.click(harness.at("second"));
    harness.press(KeyCode::Down);
    harness.press(KeyCode::Down);
    assert!(has(&harness, "M x.txt"));
    assert!(bold(&harness, "first"));

    watch(&mut harness, invalidation(true, false, &[]));

    assert!(has(&harness, "M x.txt"));
    assert!(bold(&harness, "first"));
    assert!(!has(&harness, "Loading…"));
}

#[test]
fn new_commit_keeps_the_selected_commit_under_the_cursor() {
    let mut harness = start_with_history(&[second(), first()]);
    harness.press(KeyCode::Tab);
    harness.press(KeyCode::Down);
    harness.press(KeyCode::Down);
    assert!(bold(&harness, "first"));
    let diff = diff_text(&harness);
    assert!(diff.contains("b.txt version 1"));

    set_commits(&harness, &[third(), second(), first()]);
    watch(&mut harness, invalidation(true, false, &[]));

    let rows: Vec<u16> = ["third", "second", "first"]
        .iter()
        .map(|subject| harness.at(subject).1)
        .collect();
    assert!(rows.is_sorted());
    assert!(bold(&harness, "first"));
    assert_eq!(diff_text(&harness), diff);
}

#[test]
fn commit_files_survive_an_equal_history_refresh() {
    let mut harness = start(repo(&[], &ABC, "main"));
    let file = CommitFile {
        path: "x.txt".to_string(),
        status: 'M',
        commit_hash: second().hash,
        parent: second().parent,
    };
    harness
        .git
        .commit_files
        .lock()
        .unwrap()
        .insert(second().hash, vec![file]);
    set_commits(&harness, &[second(), first()]);
    harness.press(KeyCode::Char('r'));
    harness.hold_history = true;
    harness.click(harness.at("second"));
    assert_eq!(harness.held_history.len(), 1, "commit files are loading");

    watch(&mut harness, invalidation(true, false, &[]));
    let history = harness
        .held_history
        .pop_back()
        .expect("a quiet history job");
    assert!(matches!(history, Job::History { .. }));
    let event = runtime::run_job(&harness.git, history);
    harness.send(event);
    let event = harness.run_history();
    harness.send(event);

    assert!(has(&harness, "M x.txt"), "{}", harness.screen());
    assert!(!has(&harness, "Loading…"));
}

// Quiet diff reload failures

#[test]
fn quiet_diff_failure_warns_once_and_keeps_the_old_diff() {
    let mut harness = start(repo(&[], &ABC, "main"));
    select_checked_b(&mut harness);
    harness
        .git
        .set_diff("b.txt", Err(failure("fatal: diff failed")));
    watch(&mut harness, edit("b.txt"));
    watch(&mut harness, edit("b.txt"));
    assert_eq!(
        warnings(&harness),
        ["Automatic refresh failed: fatal: diff failed. Press r to refresh manually."]
    );
    assert_eq!(harness.app.toasts.len(), 1);
    assert!(has(&harness, "b.txt version 1"));
    assert!(!has(&harness, "Loading…"));
    assert!(has(&harness, "[x] M b.txt"));

    set_version(&harness, 2);
    watch(&mut harness, edit("b.txt"));
    assert!(has(&harness, "b.txt version 2"));
    assert_eq!(warnings(&harness).len(), 1);

    harness
        .git
        .set_diff("b.txt", Err(failure("fatal: diff failed")));
    watch(&mut harness, edit("b.txt"));
    assert_eq!(warnings(&harness).len(), 2);
}

#[test]
fn quiet_history_never_selects_the_first_commit_or_moves_focus() {
    let mut harness = start(repo(&[], &[], "main"));
    let focus = harness.app.focus;
    set_commits(&harness, &[first()]);
    watch(&mut harness, invalidation(true, false, &[]));
    assert!(has(&harness, "first"));
    assert_eq!(harness.app.commits.cursor, None);
    assert_eq!(harness.app.focus, focus);
    // A quiet load leaves the first commit collapsed and loads no files.
    assert!(has(&harness, "▶ first"));
    assert!(
        !harness
            .git
            .calls()
            .iter()
            .any(|call| call.starts_with("commit_files"))
    );
}
