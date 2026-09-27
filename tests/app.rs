mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::{Harness, ROOT, TempDir, state};
use crossterm::event::{KeyCode, KeyModifiers};
use gitplume::app::{App, Effect, Event, Focus, Severity, Tab};
use gitplume::git::GitError;
use gitplume::model::{Commit, CommitFile};
use gitplume::runtime::{self, claim_first_launch, default_marker};
use gitplume::ui::format_commit_label;
use ratatui::style::Modifier;

/// The label of the bold tab in the tabs row.
fn active_tab(harness: &Harness) -> String {
    let buffer = harness.buffer();
    let row = harness.line(0);
    let bold: String = row
        .chars()
        .enumerate()
        .filter(|(x, _)| buffer[(*x as u16, 0)].modifier.contains(Modifier::BOLD))
        .map(|(_, c)| c)
        .collect();
    bold.trim().to_string()
}

fn shortcuts_open(harness: &Harness) -> bool {
    harness.find("Keyboard Shortcuts").is_some()
}

fn failure(stderr: &str) -> GitError {
    GitError::Failed {
        code: Some(128),
        stderr: stderr.to_string(),
    }
}

#[test]
fn starts_on_changes_tab_with_status_and_branch() {
    let harness = Harness::new(Ok(state(&["staged.txt"], &["notes.txt"])));
    assert_eq!(active_tab(&harness), "Changes");
    let screen = harness.screen();
    for text in [
        "Staged",
        "Unstaged",
        "Commits",
        "[ ] M staged.txt",
        "[ ] M notes.txt",
    ] {
        assert!(screen.contains(text), "{text:?} missing:\n{screen}");
    }
    assert_eq!(harness.line(29).trim(), "Branch: main");
    assert_eq!(
        *harness.git.calls.lock().unwrap(),
        ["status /repo", "files /repo"]
    );
    assert_eq!(harness.app.focus, Focus::Staged);
}

#[test]
fn startup_focuses_unstaged_when_nothing_is_staged() {
    let harness = Harness::new(Ok(state(&[], &["notes.txt"])));
    assert_eq!(harness.app.focus, Focus::Unstaged);
}

#[test]
fn status_failure_shows_an_error_toast_that_expires() {
    let mut harness = Harness::new(Err(failure("fatal: bad index")));
    let screen = harness.screen();
    assert!(screen.contains("Could not refresh status"));
    assert!(screen.contains("fatal: bad index"));
    assert!(!screen.contains("Loading…"));

    harness.send(Event::Tick(Instant::now()));
    assert!(harness.find("fatal: bad index").is_some());
    harness.send(Event::Tick(Instant::now() + Duration::from_secs(6)));
    assert!(harness.find("fatal: bad index").is_none());
}

#[test]
fn clicking_a_toast_dismisses_it() {
    let mut harness = Harness::new(Err(failure("fatal: bad index")));
    let position = harness.at("fatal: bad index");
    harness.click(position);
    assert!(harness.find("fatal: bad index").is_none());
}

#[test]
fn toasts_wrap_long_text_within_fifty_columns() {
    let message = "word ".repeat(30);
    let harness = Harness::new(Err(failure(&message)));
    let (x, y) = harness.at("Could not refresh status");
    assert!(x >= 100 - 50);
    // The body wraps onto several rows beneath the title.
    assert!(harness.line(y + 1).contains("word word"));
    assert!(harness.line(y + 2).contains("word word"));
}

#[test]
fn number_keys_switch_tabs_and_focus() {
    let mut harness = Harness::new(Ok(state(&["first.txt"], &["unstaged.txt"])));
    harness.press(KeyCode::Char('2'));
    assert_eq!(active_tab(&harness), "Files");
    assert_eq!(harness.app.focus, Focus::FilesTree);
    harness.press(KeyCode::Char('1'));
    assert_eq!(active_tab(&harness), "Changes");
    assert_eq!(harness.app.focus, Focus::Staged);
}

#[test]
fn changes_key_focuses_the_first_non_empty_section() {
    let mut harness = Harness::new(Ok(state(&[], &["unstaged.txt"])));
    harness.press(KeyCode::Tab);
    harness.press(KeyCode::Char('1'));
    assert_eq!(harness.app.focus, Focus::Unstaged);

    let mut harness = Harness::new(Ok(state(&[], &[])));
    harness.press(KeyCode::Char('1'));
    assert_eq!(harness.app.focus, Focus::Commits);
}

#[test]
fn tab_and_shift_tab_cycle_panes_of_the_active_tab() {
    let mut harness = Harness::new(Ok(state(&["a.txt"], &["b.txt"])));
    let mut seen = Vec::new();
    for _ in 0..4 {
        harness.press(KeyCode::Tab);
        seen.push(harness.app.focus);
    }
    assert_eq!(
        seen,
        [Focus::Unstaged, Focus::Commits, Focus::Diff, Focus::Staged]
    );
    harness.press(KeyCode::BackTab);
    assert_eq!(harness.app.focus, Focus::Diff);

    harness.press(KeyCode::Char('2'));
    harness.press(KeyCode::Tab);
    assert_eq!(harness.app.focus, Focus::Preview);
    harness.press(KeyCode::Tab);
    assert_eq!(harness.app.focus, Focus::FilesTree);
}

#[test]
fn clicking_tabs_and_panes_moves_focus() {
    let mut harness = Harness::new(Ok(state(&["a.txt"], &["b.txt"])));
    harness.click(harness.at("b.txt"));
    assert_eq!(harness.app.focus, Focus::Unstaged);
    harness.click((70, 15));
    assert_eq!(harness.app.focus, Focus::Diff);

    harness.click(harness.at("Files"));
    assert_eq!(harness.app.tab, Tab::Files);
    assert_eq!(harness.app.focus, Focus::FilesTree);
    harness.click((70, 15));
    assert_eq!(harness.app.focus, Focus::Preview);

    harness.click(harness.at("Changes"));
    assert_eq!(active_tab(&harness), "Changes");
}

#[test]
fn focus_shows_in_the_highlighted_row_not_the_border() {
    let mut harness = Harness::new(Ok(state(&["a.txt"], &["b.txt"])));
    let (x, staged_row) = harness.at("a.txt");
    let (_, unstaged_row) = harness.at("b.txt");
    let cell = |harness: &Harness, pos: (u16, u16)| harness.buffer()[pos].clone();
    let border = cell(&harness, (0, staged_row)).fg;
    let focused_row = cell(&harness, (x, staged_row)).bg;
    harness.press(KeyCode::Down);
    harness.press(KeyCode::Tab);
    harness.press(KeyCode::Down);
    assert_eq!(cell(&harness, (0, staged_row)).fg, border);
    assert_eq!(cell(&harness, (0, unstaged_row)).fg, border);
    assert_eq!(cell(&harness, (x, unstaged_row)).bg, focused_row);
    assert_ne!(cell(&harness, (x, staged_row)).bg, focused_row);
}

#[test]
fn hovering_a_button_shows_its_hint_in_the_status_bar() {
    let mut harness = Harness::new(Ok(state(&[], &[])));
    let (x, y) = harness.at("↑");
    harness.hover((x, y));
    assert_eq!(harness.line(29).trim(), "Previous Change");
    harness.hover((x + 3, y));
    assert_eq!(harness.line(29).trim(), "Next Change");
    harness.hover((70, 15));
    assert_eq!(harness.line(29).trim(), "Branch: main");
}

#[test]
fn q_and_ctrl_c_quit() {
    let mut harness = Harness::new(Ok(state(&[], &[])));
    harness.press(KeyCode::Char('c'));
    assert!(!harness.quit);
    harness.press(KeyCode::Char('q'));
    assert!(harness.quit);

    let mut harness = Harness::new(Ok(state(&[], &[])));
    harness.press_with(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert!(harness.quit);
}

#[test]
fn shortcut_popup_opens_on_first_launch_and_with_h() {
    let mut harness = Harness::with(Ok(state(&[], &[])), true, 100, 30);
    assert!(shortcuts_open(&harness));
    let screen = harness.screen();
    assert!(screen.contains("Mouse controls are supported throughout."));
    assert!(screen.contains("s           Stage or unstage"));
    // The dialog is 60 columns wide, border included.
    let (x, y) = harness.at("Keyboard Shortcuts");
    let top: Vec<char> = harness.line(y - 2).chars().collect();
    let left = top[..x as usize].iter().rposition(|c| *c == '┌').unwrap();
    let right = left + top[left..].iter().position(|c| *c == '┐').unwrap();
    assert_eq!(right - left + 1, 60);

    harness.press(KeyCode::Char('h'));
    assert!(!shortcuts_open(&harness));
    harness.press(KeyCode::Char('h'));
    assert!(shortcuts_open(&harness));
    harness.press(KeyCode::Esc);
    assert!(!shortcuts_open(&harness));
    harness.press(KeyCode::Char('h'));
    harness.press(KeyCode::Enter);
    assert!(!shortcuts_open(&harness));
    harness.press(KeyCode::Char('h'));
    harness.click(harness.at("Close"));
    assert!(!shortcuts_open(&harness));
}

#[test]
fn hovering_the_close_button_lightens_it() {
    let mut harness = Harness::with(Ok(state(&[], &[])), true, 100, 30);
    let close = harness.at("Close");
    let before = harness.buffer()[close].bg;
    harness.hover(close);
    assert_ne!(harness.buffer()[close].bg, before);
    harness.hover((0, 0));
    assert_eq!(harness.buffer()[close].bg, before);
}

#[test]
fn close_button_is_a_solid_three_row_block_clickable_on_its_edges() {
    let mut harness = Harness::with(Ok(state(&[], &[])), true, 100, 30);
    let (x, y) = harness.at("Close");
    // Sixteen cells wide with the label centred between a ▔ top and ▁ bottom edge.
    let left = x - 5;
    let row = |y: u16| -> String {
        harness
            .line(y)
            .chars()
            .skip(left as usize)
            .take(16)
            .collect()
    };
    assert_eq!(row(y - 1), "▔".repeat(16));
    assert_eq!(row(y), "     Close      ");
    assert_eq!(row(y + 1), "▁".repeat(16));
    // A solid block: the label row has the edges' background, with a bold,
    // unreversed label.
    let buffer = harness.buffer();
    let background = buffer[(left, y - 1)].bg;
    for dx in 0..16 {
        for dy in [y - 1, y, y + 1] {
            assert_eq!(buffer[(left + dx, dy)].bg, background, "({dx}, {dy})");
        }
    }
    let label = &buffer[(x, y)];
    assert!(label.modifier.contains(Modifier::BOLD));
    assert!(!label.modifier.contains(Modifier::REVERSED));
    assert_ne!(label.fg, background);
    // The whole button is clickable, edges included.
    harness.click((left, y + 1));
    assert!(!shortcuts_open(&harness));
}

#[test]
fn tabs_have_an_underline_bar_that_follows_the_active_tab() {
    let mut harness = Harness::new(Ok(state(&[], &[])));
    assert!(harness.line(0).starts_with(" Changes  Files "));
    let bar = |harness: &Harness| -> String { harness.line(1).chars().take(18).collect() };
    assert_eq!(bar(&harness), "━━━━━━━━━╺━━━━━━━━");
    let buffer = harness.buffer();
    assert_ne!(
        buffer[(0, 1)].fg,
        buffer[(12, 1)].fg,
        "active part is highlighted"
    );

    // Clicking the bar under Files switches tabs, and the highlight follows.
    harness.click((12, 1));
    assert_eq!(harness.app.tab, Tab::Files);
    assert_eq!(bar(&harness), "━━━━━━━━╸━━━━━━━╺━");
    let buffer = harness.buffer();
    assert_eq!(buffer[(12, 1)].fg, buffer[(9, 1)].fg);
    assert_ne!(buffer[(0, 1)].fg, buffer[(12, 1)].fg);
}

#[test]
fn shortcut_popup_is_absent_when_not_first_launch() {
    let harness = Harness::new(Ok(state(&[], &[])));
    assert!(!shortcuts_open(&harness));
}

#[test]
fn modal_captures_keys_and_clicks() {
    let mut harness = Harness::with(Ok(state(&["a.txt"], &["b.txt"])), true, 100, 30);
    harness.press(KeyCode::Char('2'));
    harness.press(KeyCode::Tab);
    harness.press(KeyCode::Char('q'));
    assert!(!harness.quit);
    harness.click(harness.at("Files"));
    harness.click((0, 29));
    assert_eq!(harness.app.tab, Tab::Changes);
    assert_eq!(harness.app.focus, Focus::Staged);
    assert!(shortcuts_open(&harness));
}

#[test]
fn modal_dims_the_screen_behind_it() {
    let mut harness = Harness::new(Ok(state(&[], &[])));
    let before = harness.buffer()[(1, 29)].bg;
    harness.press(KeyCode::Char('h'));
    assert_ne!(harness.buffer()[(1, 29)].bg, before);
}

#[test]
fn claim_first_launch_accepts_an_isolated_marker_path() {
    let dir = TempDir::new("isolated-marker");
    let marker = dir.0.join("state").join("welcome-shown");
    assert!(claim_first_launch(&marker));
    assert!(marker.is_file());
    assert!(!claim_first_launch(&marker));
}

#[test]
fn claim_first_launch_shows_help_when_marker_parent_is_invalid() {
    let dir = TempDir::new("invalid-parent");
    let invalid_parent = dir.0.join("not-a-directory");
    std::fs::write(&invalid_parent, "contents").unwrap();
    assert!(claim_first_launch(&invalid_parent.join("shortcuts-shown")));
}

#[test]
fn default_marker_lives_in_the_gitplume_state_directory() {
    let marker = default_marker().unwrap();
    assert!(marker.ends_with("gitplume/shortcuts-shown"), "{marker:?}");
}

#[test]
fn hover_hint_clears_when_the_tab_or_modal_changes() {
    let mut harness = Harness::new(Ok(state(&[], &[])));
    let up = harness.at("↑");
    harness.hover(up);
    harness.press(KeyCode::Char('2'));
    assert_eq!(harness.line(29).trim(), "Branch: main");

    harness.press(KeyCode::Char('1'));
    harness.hover(up);
    harness.press(KeyCode::Char('h'));
    harness.press(KeyCode::Esc);
    assert_eq!(harness.line(29).trim(), "Branch: main");
}

#[test]
fn expired_toasts_disappear_while_input_keeps_arriving() {
    let mut harness = Harness::new(Err(failure("fatal: bad index")));
    harness.app.toasts[0].expires = Instant::now();
    harness.hover((70, 15));
    assert!(harness.find("fatal: bad index").is_none());
}

#[test]
fn clicking_a_toast_dismisses_that_toast_only() {
    let mut harness = Harness::new(Ok(state(&[], &[])));
    for body in ["toast A", "toast B", "toast C"] {
        harness.app.notify(None, body, Severity::Warning);
    }
    harness.draw();
    let b = harness.at("toast B");
    // Only A is due; expire it without redrawing, so the hit map is stale.
    let now = Instant::now();
    harness.app.toasts[0].expires = now;
    harness.app.update(Event::Tick(now));
    harness.click(b);
    assert!(harness.find("toast B").is_none());
    assert!(harness.find("toast C").is_some());
}

fn commit(hash: &str, subject: &str) -> Commit {
    Commit {
        hash: hash.to_string(),
        short_hash: hash[..7].to_string(),
        parent: Some("parent-hash".to_string()),
        subject: subject.to_string(),
    }
}

fn commit_file(commit: &Commit, path: &str) -> CommitFile {
    CommitFile {
        path: path.to_string(),
        status: 'M',
        commit_hash: commit.hash.clone(),
        parent: commit.parent.clone(),
    }
}

/// Serve `commits` as history and reload it with `r`.
fn with_history(harness: &mut Harness, commits: &[Commit]) {
    *harness.git.commits.lock().unwrap() = Ok(commits.to_vec());
    harness.press(KeyCode::Char('r'));
}

/// How many times commit files were loaded.
fn commit_file_loads(harness: &Harness) -> usize {
    harness
        .git
        .calls()
        .iter()
        .filter(|call| call.starts_with("commit_files"))
        .count()
}

#[test]
fn format_commit_label_expands_gitmoji_and_places_hash_last() {
    for (subject, expected) in [
        (":bug: Fix crash", "\u{1f41b} Fix crash abc1234"),
        ("Plain subject", "Plain subject abc1234"),
        (
            ":not_a_gitmoji: Keep unknown",
            ":not_a_gitmoji: Keep unknown abc1234",
        ),
    ] {
        let label = format_commit_label(&commit("abc1234full", subject));
        assert_eq!(label.to_string(), expected);
        let dim: Vec<&str> = label
            .spans
            .iter()
            .filter(|span| span.style.add_modifier.contains(Modifier::DIM))
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(dim, ["abc1234"], "{subject}");
    }
}

#[test]
fn commit_selection_expands_files_and_file_selection_uses_shared_diff() {
    let mut status = state(&["working.txt"], &[]);
    status.branch = "[bold]feature".to_string();
    let mut harness = Harness::new(Ok(status));
    let added = commit("abc1234full", "Add history");
    harness.git.commit_files.lock().unwrap().insert(
        added.hash.clone(),
        vec![commit_file(&added, "[bold]report.txt")],
    );
    harness.git.set_diff(
        "[bold]report.txt",
        Ok(
            "diff --git a/report.txt b/report.txt\n--- a/report.txt\n+++ b/report.txt\n\
            @@ -1,2 +1,2 @@\n context line\n-old report text\n+new report text\n"
                .to_string(),
        ),
    );
    with_history(&mut harness, std::slice::from_ref(&added));
    assert!(harness.find("▶ Add history abc1234").is_some());
    assert_eq!(harness.line(29).trim(), "Branch: [bold]feature");

    // Clicking a commit expands it and loads its files lazily.
    harness.click(harness.at("Add history"));
    assert_eq!(harness.app.focus, Focus::Commits);
    assert!(harness.find("▼ Add history").is_some());
    assert!(harness.find("M [bold]report.txt").is_some());
    assert_eq!(commit_file_loads(&harness), 1);

    // Clicking toggles; loaded files are kept.
    harness.click(harness.at("Add history"));
    assert!(harness.find("M [bold]report.txt").is_none());
    harness.click(harness.at("Add history"));
    assert!(harness.find("M [bold]report.txt").is_some());
    assert_eq!(commit_file_loads(&harness), 1);

    // Selecting the file shows its historical diff in the shared pane.
    harness.click(harness.at("M [bold]report.txt"));
    let screen = harness.screen();
    assert!(screen.contains("│ [bold]report.txt"), "{screen}");
    assert!(screen.contains("new report text"));
    assert!(screen.contains("old report text"));
    assert!(
        harness
            .git
            .calls()
            .contains(&"diff [bold]report.txt".to_string())
    );
}

#[test]
fn commit_tree_keys_move_expand_collapse_and_select() {
    let mut harness = Harness::new(Ok(state(&[], &[])));
    let first = commit("aaaaaaafull", "First");
    let second = commit("bbbbbbbfull", "Second");
    harness
        .git
        .commit_files
        .lock()
        .unwrap()
        .insert(second.hash.clone(), vec![commit_file(&second, "b.txt")]);
    harness.git.set_diff(
        "b.txt",
        Ok("--- a/b.txt\n+++ b/b.txt\n@@ -1 +1 @@\n-was\n+now\n".to_string()),
    );
    with_history(&mut harness, &[first, second]);
    assert_eq!(harness.app.focus, Focus::Commits);
    // The selected first commit starts expanded; Left collapses it.
    assert!(harness.find("▼ First").is_some());
    harness.press(KeyCode::Left);
    assert!(harness.find("▶ First").is_some());

    harness.press(KeyCode::Char('j'));
    harness.press(KeyCode::Right);
    assert!(harness.find("▼ Second").is_some());
    harness.press(KeyCode::Left);
    assert!(harness.find("▶ Second").is_some());
    assert!(harness.find("M b.txt").is_none());
    harness.press(KeyCode::Enter);
    harness.press(KeyCode::Down);
    harness.press(KeyCode::Enter);
    let screen = harness.screen();
    assert!(screen.contains("now"), "{screen}");
    assert!(screen.contains("was"));
    harness.press(KeyCode::Char('k'));
    harness.press(KeyCode::Enter);
    assert!(harness.find("M b.txt").is_none());
}

#[test]
fn commit_without_files_shows_an_inert_leaf() {
    let mut harness = Harness::new(Ok(state(&[], &[])));
    // The only commit is selected and expanded; Enter on its leaf does nothing.
    with_history(&mut harness, &[commit("aaaaaaafull", "Empty")]);
    harness.press(KeyCode::Down);
    harness.press(KeyCode::Enter);
    assert!(harness.find("(no changed files)").is_some());
    assert!(
        !harness
            .git
            .calls()
            .iter()
            .any(|call| call.starts_with("diff"))
    );
}

#[test]
fn history_selects_the_first_commit_only_when_both_lists_are_empty() {
    let mut harness = Harness::new(Ok(state(&[], &["notes.txt"])));
    let first = commit("aaaaaaafull", "First");
    let second = commit("bbbbbbbfull", "Second");
    for commit in [&first, &second] {
        harness
            .git
            .commit_files
            .lock()
            .unwrap()
            .insert(commit.hash.clone(), vec![commit_file(commit, "x.txt")]);
    }
    with_history(&mut harness, &[first]);
    assert_eq!(harness.app.focus, Focus::Unstaged);
    let (x, y) = harness.at("First");
    assert!(!harness.buffer()[(x, y)].modifier.contains(Modifier::BOLD));
    assert!(harness.find("▶ First").is_some());
    assert_eq!(commit_file_loads(&harness), 0);

    // Selecting the first commit also expands it and loads its files.
    *harness.git.status.lock().unwrap() = Ok(state(&[], &[]));
    with_history(&mut harness, &[second]);
    assert_eq!(harness.app.focus, Focus::Commits);
    let (x, y) = harness.at("Second");
    assert!(harness.buffer()[(x, y)].modifier.contains(Modifier::BOLD));
    assert!(harness.find("▼ Second").is_some());
    assert!(harness.find("M x.txt").is_some());
    assert_eq!(commit_file_loads(&harness), 1);
}

/// Launch a fresh app against the harness's fake, as a new start would.
fn relaunch(harness: &mut Harness) {
    harness.app = App::new(PathBuf::from(ROOT), PathBuf::from(ROOT), false);
    for effect in harness.app.start() {
        if let Effect::Git(job) = effect {
            let event = runtime::run_job(&harness.git, job);
            harness.send(event);
        }
    }
}

#[test]
fn manual_refresh_with_equal_history_keeps_the_commit_expanded() {
    let mut harness = Harness::new(Ok(state(&[], &[])));
    let first = commit("aaaaaaafull", "First");
    let second = commit("bbbbbbbfull", "Second");
    for commit in [&first, &second] {
        harness
            .git
            .commit_files
            .lock()
            .unwrap()
            .insert(commit.hash.clone(), vec![commit_file(commit, "x.txt")]);
    }
    *harness.git.commits.lock().unwrap() = Ok(vec![second, first]);

    relaunch(&mut harness);
    assert!(harness.find("▼ Second").is_some(), "{}", harness.screen());
    assert!(harness.find("M x.txt").is_some());
    assert!(harness.find("▶ First").is_some());
    assert_eq!(commit_file_loads(&harness), 1);

    harness.press(KeyCode::Char('r'));
    assert!(harness.find("▼ Second").is_some(), "{}", harness.screen());
    assert!(harness.find("M x.txt").is_some());
    assert_eq!(commit_file_loads(&harness), 1);
}

#[test]
fn unchanged_history_keeps_the_tree_and_changed_history_keeps_the_cursor_commit() {
    let mut harness = Harness::new(Ok(state(&["working.txt"], &[])));
    let first = commit("aaaaaaafull", "First");
    let second = commit("bbbbbbbfull", "Second");
    harness
        .git
        .commit_files
        .lock()
        .unwrap()
        .insert(second.hash.clone(), vec![commit_file(&second, "b.txt")]);
    with_history(&mut harness, &[first.clone(), second.clone()]);
    harness.click(harness.at("Second"));
    harness.press(KeyCode::Down);

    // Same commits: expansion, files, and cursor stay.
    harness.press(KeyCode::Char('r'));
    assert!(harness.find("M b.txt").is_some());
    // The cursor row stands out from the other rows.
    let plain = harness.buffer()[harness.at("First")].bg;
    assert_ne!(harness.buffer()[harness.at("M b.txt")].bg, plain);

    // New commits: rebuilt collapsed, cursor on the file's commit.
    with_history(&mut harness, &[commit("ccccccc0", "Newest"), first, second]);
    assert!(harness.find("M b.txt").is_none());
    let plain = harness.buffer()[harness.at("Newest")].bg;
    assert_ne!(harness.buffer()[harness.at("▶ Second")].bg, plain);
}

#[test]
fn history_failure_shows_an_error_toast() {
    let mut harness = Harness::new(Ok(state(&[], &[])));
    *harness.git.commits.lock().unwrap() = Err(failure("fatal: bad history"));
    harness.press(KeyCode::Char('r'));
    let screen = harness.screen();
    assert!(screen.contains("Could not refresh history"), "{screen}");
    assert!(screen.contains("fatal: bad history"));
}

#[test]
fn tree_ignores_keys_while_commit_files_load() {
    let mut harness = Harness::new(Ok(state(&["working.txt"], &[])));
    let first = commit("aaaaaaafull", "First");
    let second = commit("bbbbbbbfull", "Second");
    harness
        .git
        .commit_files
        .lock()
        .unwrap()
        .insert(first.hash.clone(), vec![commit_file(&first, "a.txt")]);
    with_history(&mut harness, &[first, second]);
    harness.hold_history = true;
    harness.click(harness.at("First"));
    harness.press(KeyCode::Down);
    harness.press(KeyCode::Right);
    assert_eq!(harness.held_history.len(), 1);

    let files = harness.run_history();
    harness.send(files);
    assert!(harness.find("M a.txt").is_some());
    assert_eq!(commit_file_loads(&harness), 1);
}

#[test]
fn shrinking_history_never_renders_a_blank_tree() {
    let mut harness = Harness::new(Ok(state(&["working.txt"], &[])));
    let many: Vec<Commit> = (0..40)
        .map(|i| commit(&format!("{i:07}full"), &format!("Commit {i}")))
        .collect();
    with_history(&mut harness, &many);
    harness.press(KeyCode::Tab);
    harness.press(KeyCode::Tab);
    assert_eq!(harness.app.focus, Focus::Commits);
    for _ in 0..40 {
        harness.press(KeyCode::Down);
    }
    assert!(harness.find("Commit 39").is_some());

    // The cursor commit disappears, so the rebuilt tree has no cursor.
    with_history(&mut harness, &many[..2]);
    assert!(harness.find("Commit 0 ").is_some(), "{}", harness.screen());
    assert!(harness.find("Commit 1 ").is_some());
}

#[test]
fn app_builds_file_tree_from_launch_cwd_and_refreshes_both_views() {
    let root = std::path::Path::new("/work/repository");
    let cwd = root.join("nested");
    let mut harness = Harness::launched(root, &cwd, &["[red]top.txt", "[directory]/example.py"]);
    harness.press(KeyCode::Char('2'));

    let rows: Vec<String> = (3..6).map(|y| harness.line(y)).collect();
    assert!(
        rows[0].contains("\u{e5fe} /work/repository/nested"),
        "{rows:?}"
    );
    // Folders first, each row an icon then the literal name.
    assert!(rows[1].contains("\u{e5ff} [directory]"), "{rows:?}");
    assert!(rows[2].contains("\u{f0219} [red]top.txt"), "{rows:?}");
    assert!(harness.find("example.py").is_none());

    harness.press(KeyCode::Char('j'));
    harness.press(KeyCode::Enter);
    assert!(harness.line(4).contains("\u{e5fe} [directory]"));
    assert!(harness.line(5).contains("example.py"));
    harness.press(KeyCode::Enter);
    assert!(harness.line(4).contains("\u{e5ff} [directory]"));
    assert!(harness.find("example.py").is_none());

    harness.press(KeyCode::Char('r'));
    assert_eq!(
        harness.git.calls(),
        [
            "status /work/repository",
            "files /work/repository/nested",
            "status /work/repository",
            "files /work/repository/nested",
        ]
    );
}

#[test]
fn files_tree_indents_with_guides_and_shows_expand_indicators() {
    let root = std::path::Path::new("/repo");
    let mut harness = Harness::launched(root, root, &["dir/sub/deep.rs", "dir/mid.py", "top.txt"]);
    harness.press(KeyCode::Char('2'));
    assert!(
        harness.line(3).contains("▼ \u{e5fe} /repo"),
        "{}",
        harness.screen()
    );
    assert!(
        harness.line(4).contains("├─ ▶ \u{e5ff} dir"),
        "{}",
        harness.screen()
    );
    assert!(harness.line(5).contains("└─ "), "{}", harness.screen());
    harness.press(KeyCode::Char('j'));
    harness.press(KeyCode::Enter);
    harness.press(KeyCode::Char('j'));
    harness.press(KeyCode::Enter);
    let screen = harness.screen();
    assert!(harness.line(4).contains("├─ ▼ \u{e5fe} dir"), "{screen}");
    assert!(harness.line(5).contains("│  ├─ ▼ \u{e5fe} sub"), "{screen}");
    assert!(harness.line(6).contains("│  │  └─ "), "{screen}");
    assert!(harness.line(6).contains("deep.rs"), "{screen}");
    assert!(harness.line(7).contains("│  └─ "), "{screen}");
    assert!(harness.line(7).contains("mid.py"), "{screen}");
    assert!(harness.line(8).contains("└─ "), "{screen}");
    assert!(harness.line(8).contains("top.txt"), "{screen}");
}

#[test]
fn file_icon_takes_its_color_and_the_name_uses_the_text_color() {
    let mut harness = Harness::launched(
        std::path::Path::new("/repo"),
        std::path::Path::new("/repo"),
        &["main.py", "other.py"],
    );
    harness.press(KeyCode::Char('2'));
    let buffer = harness.buffer();
    // Row 4 is not under the cursor, so it uses plain row colors.
    let (x, y) = harness.at("other.py");
    let icon = &buffer[(x - 2, y)];
    let name = &buffer[(x, y)];
    assert_ne!(icon.fg, name.fg);
    assert_eq!(name.fg, buffer[(x + 1, y)].fg);
}

#[test]
fn files_tree_ignores_input_while_loading_and_refresh_clears_the_preview() {
    let dir = TempDir::new("files-loading");
    std::fs::write(dir.0.join("a.txt"), "alpha\n").unwrap();
    let mut harness = Harness::launched(&dir.0, &dir.0, &["a.txt"]);
    harness.press(KeyCode::Char('2'));
    harness.press(KeyCode::Char('j'));
    harness.press(KeyCode::Enter);
    assert!(harness.screen().contains("alpha"));

    harness.hold_files = true;
    harness.press(KeyCode::Char('r'));
    let screen = harness.screen();
    assert!(screen.contains("Loading…"));
    assert!(!screen.contains("alpha"), "refresh clears the preview");
    assert!(!harness.line(1).contains("a.txt"), "and its title");
    harness.press(KeyCode::Enter);
    harness.press(KeyCode::Char('k'));
    assert!(
        harness.held_files.len() == 1,
        "no preview starts while loading"
    );

    let files = harness.run_files();
    harness.send(files);
    assert!(!harness.screen().contains("Loading…"));
    assert!(harness.find("a.txt").is_some());
}

#[test]
fn files_failure_shows_an_error_toast() {
    let root = std::path::Path::new("/repo");
    let mut harness = Harness::launched(root, root, &[]);
    *harness.git.files.lock().unwrap() = Err(failure("fatal: cannot list"));
    harness.press(KeyCode::Char('r'));
    assert!(harness.find("Could not refresh files").is_some());
    assert!(harness.find("fatal: cannot list").is_some());
}

#[test]
fn clicking_a_file_row_previews_it() {
    let dir = TempDir::new("files-click");
    std::fs::write(dir.0.join("b.txt"), "bravo\n").unwrap();
    let mut harness = Harness::launched(&dir.0, &dir.0, &["b.txt"]);
    harness.press(KeyCode::Char('2'));
    let position = harness.at("b.txt");
    harness.click(position);
    assert!(harness.screen().contains("bravo"));
    assert_eq!(harness.app.focus, Focus::FilesTree);
}
