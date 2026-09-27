mod common;

use std::path::PathBuf;

use common::{FakeGit, Harness, ROOT, failure, state};
use crossterm::event::KeyCode;
use gitplume::app::{Action, Focus, Job};
use gitplume::model::{FileEntry, RepoState, Side};
use gitplume::runtime;
use ratatui::style::Modifier;

const SIDEBAR: usize = 30;

fn repo(staged: Vec<FileEntry>, unstaged: Vec<FileEntry>) -> RepoState {
    RepoState {
        root: PathBuf::from(ROOT),
        staged,
        unstaged,
        branch: "main".to_string(),
    }
}

/// Position of `glyph` in the sidebar part of row `y`, if drawn there.
fn sidebar_glyph(harness: &Harness, y: u16, glyph: char) -> Option<(u16, u16)> {
    harness
        .line(y)
        .chars()
        .take(SIDEBAR)
        .position(|c| c == glyph)
        .map(|x| (x as u16, y))
}

fn row_of(harness: &Harness, text: &str) -> u16 {
    harness.at(text).1
}

/// The discard dialog's Discard button, on the same row as Cancel.
fn discard_button(harness: &Harness) -> (u16, u16) {
    let (_, y) = harness.at("Cancel");
    let x = harness.line(y).chars().position(|c| c == 'D').unwrap();
    (x as u16, y)
}

fn calls_since(harness: &Harness, start: usize) -> Vec<String> {
    harness.git.calls()[start..].to_vec()
}

/// Widen the sidebar so long labels are not truncated.
fn widen_sidebar(harness: &mut Harness) {
    harness.drag((30, 10), (90, 10));
}

#[test]
fn labels_preserve_plain_paths() {
    let mut harness = Harness::new(Ok(repo(
        vec![FileEntry::new("path with spaces.txt", Side::Staged, 'M')],
        vec![
            FileEntry::new("-leading-dash.txt", Side::Unstaged, '?'),
            FileEntry::new("[brackets].txt", Side::Unstaged, 'M'),
        ],
    )));
    widen_sidebar(&mut harness);
    let screen = harness.screen();
    for label in [
        "[ ] M path with spaces.txt",
        "[ ] ? -leading-dash.txt",
        "[ ] M [brackets].txt",
    ] {
        assert!(screen.contains(label), "{label}\n{screen}");
    }
}

#[test]
fn space_marks_the_highlighted_row_checked() {
    let mut harness = Harness::new(Ok(state(&[], &["example.py"])));
    harness.press(KeyCode::Char(' '));
    assert!(harness.screen().contains("[x] M example.py"));
    harness.press(KeyCode::Char(' '));
    assert!(harness.screen().contains("[ ] M example.py"));
}

#[test]
fn unsupported_label_explains_the_reason() {
    let mut harness = Harness::new(Ok(repo(
        vec![FileEntry::unsupported(
            "renamed.py",
            Side::Staged,
            'R',
            "Rename from original.py is not supported.",
        )],
        Vec::new(),
    )));
    widen_sidebar(&mut harness);
    assert!(
        harness
            .screen()
            .contains("[!] R renamed.py - Rename from original.py is not supported.")
    );
}

#[test]
fn discard_restores_tracked_and_cleans_untracked() {
    let git = FakeGit::new(Ok(state(&[], &[])));
    runtime::run_job(
        &git,
        Job::Mutate {
            root: PathBuf::from(ROOT),
            action: Action::Discard,
            entries: vec![
                FileEntry::new("tracked.txt", Side::Unstaged, 'M'),
                FileEntry::new("new.txt", Side::Unstaged, '?'),
            ],
            token: 1,
        },
    );
    assert_eq!(
        git.calls(),
        ["restore tracked.txt", "clean new.txt", "status /repo"]
    );
}

#[test]
fn status_actions_support_single_bulk_and_confirmed_discard() {
    let mut harness = Harness::new(Ok(repo(
        vec![FileEntry::new("staged.txt", Side::Staged, 'M')],
        vec![
            FileEntry::new("modified.txt", Side::Unstaged, 'M'),
            FileEntry::new("new.txt", Side::Unstaged, '?'),
        ],
    )));

    // The focused, highlighted staged row reveals its unstage action.
    let staged_row = row_of(&harness, "staged.txt");
    let unstage = sidebar_glyph(&harness, staged_row, '↓').expect("row unstage action");
    harness.hover(unstage);
    assert_eq!(harness.line(29).trim(), "Unstage Changes");

    // Clicking the checkbox columns checks the row without selecting it.
    assert_eq!(sidebar_glyph(&harness, 2, '↓'), None);
    harness.click((1, staged_row));
    assert!(harness.screen().contains("[x] M staged.txt"));
    assert!(!harness.line(2).contains("staged.txt"));

    // The bulk unstage button appears in the Staged title bar.
    let bulk_unstage = sidebar_glyph(&harness, 2, '↓').expect("bulk unstage");
    harness.hover(bulk_unstage);
    assert_eq!(harness.line(29).trim(), "Unstage Selected Changes");
    let start = harness.git.calls().len();
    harness.click(bulk_unstage);
    assert_eq!(
        calls_since(&harness, start),
        ["unstage staged.txt", "status /repo"]
    );
    // The rebuilt lists start unchecked, so the bulk button hides again.
    assert!(harness.screen().contains("[ ] M staged.txt"));
    assert_eq!(sidebar_glyph(&harness, 2, '↓'), None);

    // Hovering an unstaged row reveals its stage action.
    let modified_row = row_of(&harness, "modified.txt");
    assert_eq!(sidebar_glyph(&harness, modified_row, '↑'), None);
    harness.hover(harness.at("modified.txt"));
    let stage = sidebar_glyph(&harness, modified_row, '↑').expect("row stage action");
    harness.hover(stage);
    assert_eq!(harness.line(29).trim(), "Stage Changes");
    let start = harness.git.calls().len();
    harness.click(stage);
    assert_eq!(
        calls_since(&harness, start),
        ["stage modified.txt", "status /repo"]
    );

    // Check both unstaged rows, then discard them in bulk after confirming.
    harness.press(KeyCode::Tab);
    assert_eq!(harness.app.focus, Focus::Unstaged);
    for _ in 0..2 {
        harness.press(KeyCode::Down);
        harness.press(KeyCode::Char(' '));
    }
    let unstaged_title = row_of(&harness, "Unstaged");
    assert!(sidebar_glyph(&harness, unstaged_title, '↑').is_some());
    let discard = sidebar_glyph(&harness, unstaged_title, '↶').expect("bulk discard");
    let start = harness.git.calls().len();
    harness.click(discard);
    let screen = harness.screen();
    assert!(screen.contains("Discard Changes?"));
    assert!(screen.contains("Discard changes to 2 files? This cannot be undone."));
    assert!(screen.contains("Untracked files will be permanently deleted."));
    assert!(
        harness.git.calls().len() == start,
        "nothing runs before confirming"
    );
    // Each button is one solid block: its label row matches its edge rows.
    // Focus tints the focused button instead of reversing its label.
    let block = |harness: &Harness, (x, y): (u16, u16)| {
        let buffer = harness.buffer();
        let label = &buffer[(x, y)];
        assert!(!label.modifier.contains(Modifier::REVERSED));
        assert_eq!(label.bg, buffer[(x, y - 1)].bg);
        assert_eq!(label.bg, buffer[(x, y + 1)].bg);
        label.bg
    };
    let focused_cancel = block(&harness, harness.at("Cancel"));
    let unfocused_discard = block(&harness, discard_button(&harness));
    harness.press(KeyCode::Tab);
    assert_ne!(block(&harness, harness.at("Cancel")), focused_cancel);
    assert_ne!(block(&harness, discard_button(&harness)), unfocused_discard);
    harness.press(KeyCode::Tab);
    // Both are three-row, sixteen-cell buttons with ▔ and ▁ edges.
    let (x, y) = harness.at("Cancel");
    let edges = |y: u16| -> String {
        harness
            .line(y)
            .chars()
            .skip(x as usize - 5)
            .take(33)
            .collect()
    };
    assert_eq!(
        edges(y - 1),
        format!("{} {}", "▔".repeat(16), "▔".repeat(16))
    );
    assert_eq!(
        edges(y + 1),
        format!("{} {}", "▁".repeat(16), "▁".repeat(16))
    );

    harness.click(discard_button(&harness));
    assert!(!harness.screen().contains("Discard Changes?"));
    assert_eq!(
        calls_since(&harness, start),
        ["restore modified.txt", "clean new.txt", "status /repo"]
    );

    // `d` asks about the highlighted row alone; cancelling does nothing.
    harness.press(KeyCode::Tab);
    harness.press(KeyCode::Down);
    let start = harness.git.calls().len();
    harness.press(KeyCode::Char('d'));
    assert!(
        harness
            .screen()
            .contains("Discard changes to 1 file? This cannot be undone.")
    );
    harness.click(harness.at("Cancel"));
    assert!(!harness.screen().contains("Discard Changes?"));
    assert_eq!(harness.git.calls().len(), start);
}

#[test]
fn row_unstage_button_unstages_its_file() {
    let mut harness = Harness::new(Ok(state(&["staged.txt"], &["other.txt"])));
    // The focused, highlighted staged row reveals its unstage action.
    let row = row_of(&harness, "staged.txt");
    let unstage = sidebar_glyph(&harness, row, '↓').expect("row unstage action");
    *harness.git.status.lock().unwrap() = Ok(state(&[], &["other.txt", "staged.txt"]));
    let start = harness.git.calls().len();
    harness.click(unstage);
    assert_eq!(
        calls_since(&harness, start),
        ["unstage staged.txt", "status /repo"]
    );
    // The refreshed lists show the file under Unstaged.
    assert!(row_of(&harness, "staged.txt") > row_of(&harness, "Unstaged"));
}

#[test]
fn row_discard_button_asks_before_discarding_its_file() {
    let mut harness = Harness::new(Ok(state(&[], &["a.txt", "b.txt"])));
    let row = row_of(&harness, "b.txt");
    harness.hover(harness.at("b.txt"));
    let discard = sidebar_glyph(&harness, row, '↶').expect("row discard action");
    harness.hover(discard);
    assert_eq!(harness.line(29).trim(), "Discard Changes");

    let start = harness.git.calls().len();
    harness.click(discard);
    assert!(
        harness
            .screen()
            .contains("Discard changes to 1 file? This cannot be undone.")
    );
    assert_eq!(
        harness.git.calls().len(),
        start,
        "nothing runs before confirming"
    );
    harness.click(discard_button(&harness));
    assert_eq!(
        calls_since(&harness, start),
        ["restore b.txt", "status /repo"]
    );
}

#[test]
fn bulk_stage_button_stages_the_checked_rows() {
    let mut harness = Harness::new(Ok(state(&[], &["a.txt", "b.txt", "c.txt"])));
    harness.press(KeyCode::Char(' '));
    harness.press(KeyCode::Down);
    harness.press(KeyCode::Down);
    harness.press(KeyCode::Char(' '));

    // Both title-bar actions show their hints while hovered.
    let title = row_of(&harness, "Unstaged");
    let stage = sidebar_glyph(&harness, title, '↑').expect("bulk stage");
    let discard = sidebar_glyph(&harness, title, '↶').expect("bulk discard");
    harness.hover(stage);
    assert_eq!(harness.line(29).trim(), "Stage Selected Changes");
    harness.hover(discard);
    assert_eq!(harness.line(29).trim(), "Discard Selected Changes");

    *harness.git.status.lock().unwrap() = Ok(state(&["a.txt", "c.txt"], &["b.txt"]));
    let start = harness.git.calls().len();
    harness.click(stage);
    assert_eq!(
        calls_since(&harness, start),
        ["stage a.txt c.txt", "status /repo"]
    );
    // The refreshed lists show the staged files above Unstaged.
    let unstaged = row_of(&harness, "Unstaged");
    assert!(row_of(&harness, "a.txt") < unstaged);
    assert!(row_of(&harness, "c.txt") < unstaged);
}

#[test]
fn discard_dialog_keys_move_between_buttons_and_confirm() {
    let mut harness = Harness::new(Ok(state(&[], &["a.txt"])));
    harness.press(KeyCode::Char('d'));
    let start = harness.git.calls().len();
    // Enter on the initially focused Cancel does nothing.
    harness.press(KeyCode::Enter);
    assert!(!harness.screen().contains("Discard Changes?"));
    assert_eq!(harness.git.calls().len(), start);

    harness.press(KeyCode::Char('d'));
    harness.press(KeyCode::Esc);
    assert!(!harness.screen().contains("Discard Changes?"));

    harness.press(KeyCode::Char('d'));
    harness.press(KeyCode::Right);
    // The modal captures keys meant for the app.
    harness.press(KeyCode::Char('q'));
    assert!(!harness.quit);
    harness.press(KeyCode::Enter);
    assert_eq!(
        calls_since(&harness, start),
        ["restore a.txt", "status /repo"]
    );
}

#[test]
fn s_stages_or_unstages_the_highlighted_row() {
    let mut harness = Harness::new(Ok(state(&["a.txt"], &["b.txt"])));
    harness.press(KeyCode::Char('s'));
    // `d` never applies to staged rows.
    harness.press(KeyCode::Char('d'));
    assert!(!harness.screen().contains("Discard Changes?"));
    harness.press(KeyCode::Tab);
    harness.press(KeyCode::Char('j'));
    harness.press(KeyCode::Char('s'));
    assert_eq!(
        harness.git.calls()[2..],
        [
            "unstage a.txt",
            "status /repo",
            "stage b.txt",
            "status /repo"
        ]
    );
}

#[test]
fn keys_move_the_highlight_and_enter_selects() {
    let mut harness = Harness::new(Ok(state(&[], &["a.txt", "b.txt"])));
    harness.press(KeyCode::Char('j'));
    // Moving the highlight alone never selects.
    assert!(!harness.line(2).contains("b.txt"));
    harness.press(KeyCode::Down);
    harness.press(KeyCode::Char(' '));
    assert!(harness.screen().contains("[x] M b.txt"));
    harness.press(KeyCode::Char('k'));
    harness.press(KeyCode::Up);
    harness.press(KeyCode::Char(' '));
    assert!(harness.screen().contains("[x] M a.txt"));
    harness.press(KeyCode::Enter);
    assert!(harness.line(2).contains("a.txt"));
}

#[test]
fn clicking_a_row_selects_it_and_only_checkbox_columns_toggle() {
    let mut harness = Harness::new(Ok(state(&["a.txt"], &["b.txt"])));
    let (_, y) = harness.at("b.txt");
    // Columns 0-2 of the row (x 1-3 inside the border) toggle.
    harness.click((1, y));
    assert!(harness.screen().contains("[x] M b.txt"));
    harness.click((3, y));
    assert!(harness.screen().contains("[ ] M b.txt"));
    assert_eq!(harness.app.focus, Focus::Unstaged);
    assert!(!harness.line(2).contains("b.txt"));
    // Column 3 selects the row instead.
    harness.click((4, y));
    assert!(harness.screen().contains("[ ] M b.txt"));
    assert!(harness.line(2).contains("b.txt"));
    // The border is outside the row.
    harness.click((0, y));
    assert!(harness.screen().contains("[ ] M b.txt"));
}

#[test]
fn unsupported_status_entries_are_visible_and_not_actionable() {
    let reason = "Conflict (UU) resolution is not supported.";
    let mut harness = Harness::new(Ok(repo(
        Vec::new(),
        vec![FileEntry::unsupported(
            "conflict.txt",
            Side::Unstaged,
            'U',
            reason,
        )],
    )));
    let (_, y) = harness.at("[!] U conflict.txt");
    harness.hover((5, y));
    assert_eq!(sidebar_glyph(&harness, y, '↑'), None);
    assert_eq!(sidebar_glyph(&harness, y, '↶'), None);

    let start = harness.git.calls().len();
    harness.press(KeyCode::Char(' '));
    harness.press(KeyCode::Char('s'));
    harness.press(KeyCode::Char('d'));
    assert!(harness.screen().contains("[!] U conflict.txt"));
    assert!(!harness.screen().contains("Discard Changes?"));
    let title = row_of(&harness, "Unstaged");
    assert_eq!(sidebar_glyph(&harness, title, '↑'), None);
    assert_eq!(harness.git.calls().len(), start);

    // Clicking the checkbox columns selects it instead, showing the reason.
    harness.click((1, y));
    assert!(harness.line(2).contains("conflict.txt"));
    assert!(harness.screen().contains(reason));
    assert!(harness.screen().contains("[!] U conflict.txt"));
    assert_eq!(harness.git.calls().len(), start);
}

#[test]
fn mutation_failures_are_reported_and_status_still_refreshes() {
    for (key, action) in [('s', "stage"), ('d', "discard changes")] {
        let mut harness = Harness::new(Ok(state(&[], &["a.txt"])));
        *harness.git.mutation_error.lock().unwrap() = Some(failure("fatal: index.lock exists"));
        *harness.git.status.lock().unwrap() = Ok(state(&[], &["after.txt"]));
        harness.press(KeyCode::Char(key));
        if key == 'd' {
            harness.press(KeyCode::Right);
            harness.press(KeyCode::Enter);
        }
        let screen = harness.screen();
        assert!(screen.contains(&format!("Could not {action}")), "{screen}");
        assert!(screen.contains("fatal: index.lock exists"));
        assert!(screen.contains("[ ] M after.txt"));
    }
}

#[test]
fn unstage_failure_names_the_action() {
    let mut harness = Harness::new(Ok(state(&["a.txt"], &[])));
    *harness.git.mutation_error.lock().unwrap() = Some(failure("fatal: nope"));
    harness.press(KeyCode::Char('s'));
    assert!(harness.screen().contains("Could not unstage"));
}

#[test]
fn r_refreshes_status_and_refocuses_the_first_entry() {
    let mut harness = Harness::new(Ok(state(&[], &["a.txt"])));
    assert_eq!(harness.app.focus, Focus::Unstaged);
    *harness.git.status.lock().unwrap() = Ok(state(&["new.txt"], &["a.txt"]));
    harness.press(KeyCode::Char('r'));
    assert!(harness.screen().contains("[ ] M new.txt"));
    assert_eq!(harness.app.focus, Focus::Staged);
    // The first staged row is highlighted: `s` acts on it.
    harness.press(KeyCode::Char('s'));
    assert!(harness.git.calls().contains(&"unstage new.txt".to_string()));
}

#[test]
fn status_failure_after_a_refresh_clears_loading() {
    let mut harness = Harness::new(Ok(state(&["a.txt"], &[])));
    *harness.git.status.lock().unwrap() = Err(failure("fatal: bad"));
    harness.press(KeyCode::Char('r'));
    let screen = harness.screen();
    assert!(!screen.contains("Loading…"));
    assert!(screen.contains("Could not refresh status"));
}

#[test]
fn hovering_discard_dialog_buttons_lightens_them() {
    let mut harness = Harness::new(Ok(state(&[], &["a.txt"])));
    harness.press(KeyCode::Char('d'));
    for button in [harness.at("Cancel"), discard_button(&harness)] {
        let before = harness.buffer()[button].bg;
        harness.hover(button);
        assert_ne!(harness.buffer()[button].bg, before);
        harness.hover((0, 0));
        assert_eq!(harness.buffer()[button].bg, before);
    }
}
