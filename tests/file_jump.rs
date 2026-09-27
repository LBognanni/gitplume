mod common;

use common::{Harness, TempDir};
use crossterm::event::{KeyCode, MouseEventKind};
use gitplume::app::{Focus, matching_files};

fn index(paths: &[&str]) -> Vec<(String, String)> {
    paths
        .iter()
        .map(|path| (path.to_string(), path.to_lowercase()))
        .collect()
}

#[test]
fn matching_files_requires_three_characters_and_matches_case_insensitively() {
    let files = index(&["docs/Report.md", "src/reporting.py", "src/other.py"]);
    let expected = vec!["docs/Report.md".to_string(), "src/reporting.py".to_string()];

    assert_eq!(matching_files(&files, "re"), (vec![], false));
    assert_eq!(matching_files(&files, "REP"), (expected.clone(), false));
    assert_eq!(matching_files(&files, "REPO"), (expected, false));
}

#[test]
fn matching_files_reports_only_actual_truncation() {
    let names: Vec<String> = (0..100).map(|i| format!("match-{i}.txt")).collect();
    let mut paths: Vec<&str> = names.iter().map(String::as_str).collect();

    let (matches, truncated) = matching_files(&index(&paths), "match");
    assert_eq!(matches.len(), 100);
    assert!(!truncated);

    paths.push("match-extra.txt");
    let (matches, truncated) = matching_files(&index(&paths), "match");
    assert_eq!(matches.len(), 100);
    assert!(truncated);
}

fn jump_open(harness: &Harness) -> bool {
    harness.find("Jump to file").is_some()
}

fn type_text(harness: &mut Harness, text: &str) {
    for c in text.chars() {
        harness.press(KeyCode::Char(c));
    }
}

#[test]
fn quick_file_jump_is_memory_backed_and_reveals_nested_file() {
    let dir = TempDir::new("file-jump");
    let summary = format!("src/reports/summary-{}.py", "x".repeat(80));
    let summer = format!("src/reports/summer-{}.py", "y".repeat(80));
    std::fs::create_dir_all(dir.0.join("src/reports")).unwrap();
    std::fs::write(dir.0.join(&summary), "answer = 42\n").unwrap();
    let mut harness = Harness::launched(&dir.0, &dir.0, &["README.md", &summary, &summer]);
    let files_calls = |h: &Harness| {
        h.git
            .calls()
            .iter()
            .filter(|c| c.starts_with("files"))
            .count()
    };

    // Ignored outside the Files tab, and while the tree loads.
    harness.press(KeyCode::Char('t'));
    assert!(!jump_open(&harness));
    harness.press(KeyCode::Char('2'));
    harness.hold_files = true;
    harness.press(KeyCode::Char('r'));
    harness.press(KeyCode::Char('t'));
    assert!(!jump_open(&harness));
    let files = harness.run_files();
    harness.send(files);
    harness.hold_files = false;
    let calls = files_calls(&harness);

    harness.press(KeyCode::Char('t'));
    let (_, input_row) = harness.at("Jump to file");
    assert_eq!(
        input_row, 4,
        "the dialog sits three rows from the top, its text below one row of padding"
    );
    assert!(harness.line(0).contains("Changes"), "tabs stay in place");

    type_text(&mut harness, "su");
    assert!(harness.find("summary-").is_none());
    type_text(&mut harness, "m");
    assert!(harness.find("summary-").is_some());
    assert!(harness.find("summer-").is_some());
    // Exactly the two results sit under the three-row input, and the dialog ends.
    assert!(harness.line(input_row + 2).contains("summary-"));
    assert!(harness.line(input_row + 3).contains("summer-"));
    assert!(!harness.line(input_row + 4).contains("summ"));
    assert!(harness.find("Showing first").is_none());

    harness.press(KeyCode::Enter);
    assert!(!jump_open(&harness));
    assert_eq!(harness.app.focus, Focus::FilesTree);
    let screen = harness.screen();
    // Ancestors are expanded and the chosen file is previewed.
    assert!(screen.contains(" src"), "{screen}");
    assert!(screen.contains(" reports"), "{screen}");
    assert!(screen.contains("answer = 42"), "{screen}");
    let title = harness.line(2);
    assert!(title.contains(&summary[..40]), "{title}");
    assert_eq!(
        files_calls(&harness),
        calls,
        "jumping never lists files again"
    );

    // A click outside the dialog cancels, leaving the selection.
    harness.press(KeyCode::Char('t'));
    assert!(jump_open(&harness));
    harness.click((0, 29));
    assert!(!jump_open(&harness));
    harness.press(KeyCode::Char('t'));
    harness.press(KeyCode::Esc);
    assert!(!jump_open(&harness));
    assert!(harness.screen().contains("answer = 42"));
}

#[test]
fn down_moves_into_results_and_enter_picks_the_highlighted_one() {
    let dir = TempDir::new("file-jump-down");
    std::fs::write(dir.0.join("beta-two.txt"), "second file\n").unwrap();
    let mut harness = Harness::launched(&dir.0, &dir.0, &["beta-one.txt", "beta-two.txt"]);
    harness.press(KeyCode::Char('2'));
    harness.press(KeyCode::Char('t'));
    type_text(&mut harness, "beta");
    harness.press(KeyCode::Down);
    harness.press(KeyCode::Down);
    harness.press(KeyCode::Enter);
    assert!(harness.screen().contains("second file"));
}

#[test]
fn clicking_a_result_jumps_to_it_and_backspace_edits_the_query() {
    let dir = TempDir::new("file-jump-click");
    std::fs::write(dir.0.join("gamma.txt"), "gamma body\n").unwrap();
    let mut harness = Harness::launched(&dir.0, &dir.0, &["gamma.txt"]);
    harness.press(KeyCode::Char('2'));
    harness.press(KeyCode::Char('t'));
    type_text(&mut harness, "gamx");
    assert!(harness.find("gamx").is_some());
    harness.press(KeyCode::Backspace);
    // The dialog's input text is row 4; the first result follows the input.
    assert!(harness.line(6).contains("gamma.txt"));
    harness.click((50, 6));
    assert!(!jump_open(&harness));
    assert!(harness.screen().contains("gamma body"));
}

#[test]
fn more_than_a_hundred_matches_shows_the_truncation_note() {
    let names: Vec<String> = (0..101).map(|i| format!("match-{i:03}.txt")).collect();
    let paths: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut harness = Harness::launched(
        std::path::Path::new("/repo"),
        std::path::Path::new("/repo"),
        &paths,
    );
    harness.press(KeyCode::Char('2'));
    harness.press(KeyCode::Char('t'));
    type_text(&mut harness, "match");
    assert!(harness.find("Showing first 100 matches").is_some());
}

#[test]
fn highlighted_result_stays_visible_when_moving_past_the_bottom() {
    let names: Vec<String> = (0..60).map(|i| format!("match-{i:03}.txt")).collect();
    let paths: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut harness = Harness::launched(
        std::path::Path::new("/repo"),
        std::path::Path::new("/repo"),
        &paths,
    );
    harness.press(KeyCode::Char('2'));
    harness.press(KeyCode::Char('t'));
    type_text(&mut harness, "match");
    assert!(harness.find("match-059.txt").is_none());
    for _ in 0..60 {
        harness.press(KeyCode::Down);
    }
    assert!(
        harness.find("match-059.txt").is_some(),
        "{}",
        harness.screen()
    );
    for _ in 0..59 {
        harness.press(KeyCode::Up);
    }
    assert!(
        harness.find("match-000.txt").is_some(),
        "{}",
        harness.screen()
    );
}

#[test]
fn the_wheel_scrolls_the_results() {
    let names: Vec<String> = (0..60).map(|i| format!("match-{i:03}.txt")).collect();
    let paths: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut harness = Harness::launched(
        std::path::Path::new("/repo"),
        std::path::Path::new("/repo"),
        &paths,
    );
    harness.press(KeyCode::Char('2'));
    harness.press(KeyCode::Char('t'));
    type_text(&mut harness, "match");
    let first = harness.at("match-000.txt");
    for _ in 0..100 {
        harness.scroll(MouseEventKind::ScrollDown, first);
    }
    assert!(harness.find("match-000.txt").is_none());
    assert!(
        harness.find("match-059.txt").is_some(),
        "{}",
        harness.screen()
    );
    harness.scroll(MouseEventKind::ScrollUp, first);
    assert!(harness.find("match-058.txt").is_some());
    assert!(harness.find("match-059.txt").is_none());
}

#[test]
fn jump_dialog_is_borderless_with_a_padded_input_and_highlighted_result() {
    let mut harness = Harness::launched(
        std::path::Path::new("/repo"),
        std::path::Path::new("/repo"),
        &["alpha-one.txt", "alpha-two.txt"],
    );
    harness.press(KeyCode::Char('2'));
    harness.press(KeyCode::Char('t'));
    let (x, y) = harness.at("Jump to file");
    // Row 3 is the input's top padding; nothing frames the dialog.
    let top: String = harness
        .line(3)
        .chars()
        .skip(x as usize - 1)
        .take(20)
        .collect();
    assert_eq!(top.trim(), "");
    let buffer = harness.buffer();
    assert_eq!(buffer[(x - 1, y)].bg, buffer[(x - 1, 3)].bg);
    type_text(&mut harness, "alpha");
    harness.press(KeyCode::Down);
    let (rx, ry) = harness.at("alpha-one.txt");
    assert_eq!(
        (rx, ry),
        (x - 1, y + 2),
        "results follow the three-row input"
    );
    let buffer = harness.buffer();
    assert_ne!(buffer[(rx, ry)].bg, buffer[(rx, ry + 1)].bg, "highlighted");
    assert!(
        buffer[(rx, ry)]
            .modifier
            .contains(ratatui::style::Modifier::BOLD)
    );
}

#[test]
fn typing_after_scrolling_shows_the_first_match_again() {
    let names: Vec<String> = (0..60).map(|i| format!("match-{i:03}.txt")).collect();
    let paths: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut harness = Harness::launched(
        std::path::Path::new("/repo"),
        std::path::Path::new("/repo"),
        &paths,
    );
    harness.press(KeyCode::Char('2'));
    harness.press(KeyCode::Char('t'));
    type_text(&mut harness, "match");
    let first = harness.at("match-000.txt");
    for _ in 0..10 {
        harness.scroll(MouseEventKind::ScrollDown, first);
    }
    assert!(harness.find("match-000.txt").is_none());
    // "match-" still matches all sixty files, more than fit.
    type_text(&mut harness, "-");
    assert!(
        harness.find("match-000.txt").is_some(),
        "{}",
        harness.screen()
    );
}

/// A jump dialog over `count` matching files, with the column right of the
/// results where a scrollbar would sit and the first result row.
fn jump_with(count: usize, query: &str) -> (Harness, u16, u16) {
    let names: Vec<String> = (0..count).map(|i| format!("match-{i:03}.txt")).collect();
    let paths: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut harness = Harness::launched(
        std::path::Path::new("/repo"),
        std::path::Path::new("/repo"),
        &paths,
    );
    harness.press(KeyCode::Char('2'));
    harness.press(KeyCode::Char('t'));
    let (bar, top) = bar_column(&harness);
    type_text(&mut harness, query);
    (harness, bar, top)
}

fn bar_column(harness: &Harness) -> (u16, u16) {
    let (x, y) = harness.at("Jump to file");
    // The input row's background spans the dialog's full width.
    let buffer = harness.buffer();
    let mut right = x;
    while buffer[(right + 1, y)].bg == buffer[(x - 1, y)].bg {
        right += 1;
    }
    (right, y + 2)
}

#[test]
fn results_scrollbar_appears_only_when_results_overflow() {
    let (harness, bar, top) = jump_with(60, "match");
    let buffer = harness.buffer();
    assert_ne!(
        buffer[(bar, top)].bg,
        buffer[(bar - 1, top)].bg,
        "thumb at the top of the track"
    );
    let (few, _, _) = jump_with(3, "match");
    let buffer = few.buffer();
    assert_eq!(buffer[(bar, top)].bg, buffer[(bar - 1, top)].bg, "no bar");
}

#[test]
fn clicking_the_results_scrollbar_track_scrolls() {
    let (mut harness, bar, top) = jump_with(60, "match");
    harness.click((bar, top + harness_rows(&harness) - 1));
    assert!(
        harness.find("match-000.txt").is_none(),
        "{}",
        harness.screen()
    );
    assert!(
        harness.find("match-059.txt").is_some(),
        "{}",
        harness.screen()
    );
    assert!(
        harness.line(top - 2).contains("match"),
        "the dialog stays open"
    );
}

fn harness_rows(harness: &Harness) -> u16 {
    (0..60)
        .filter(|i| harness.find(&format!("match-{i:03}.txt")).is_some())
        .count() as u16
}

#[test]
fn dragging_the_results_scrollbar_thumb_scrolls_without_choosing() {
    let (mut harness, bar, top) = jump_with(60, "match");
    harness.press(KeyCode::Down);
    let highlight = harness.buffer()[harness.at("match-000.txt")].bg;
    let last = top + harness_rows(&harness) - 1;
    harness.mouse_down((bar, top));
    // Drag past the dialog: the thumb keeps the mouse until release.
    harness.drag_to((0, last + 10));
    harness.mouse_up((0, last + 10));
    assert!(
        harness.line(top - 2).contains("match"),
        "no file was chosen"
    );
    assert!(
        harness.find("match-000.txt").is_none(),
        "{}",
        harness.screen()
    );
    assert!(
        harness.find("match-059.txt").is_some(),
        "{}",
        harness.screen()
    );
    // Scroll back: the first result is still the highlighted one.
    for _ in 0..100 {
        harness.scroll(MouseEventKind::ScrollUp, (bar - 2, top));
    }
    assert_eq!(harness.buffer()[harness.at("match-000.txt")].bg, highlight);
}

#[test]
fn closing_the_dialog_mid_drag_releases_the_results_thumb() {
    let (mut harness, bar, top) = jump_with(60, "match");
    harness.mouse_down((bar, top));
    harness.press(KeyCode::Esc);
    harness.press(KeyCode::Char('t'));
    type_text(&mut harness, "match");
    harness.hover((0, top + 40));
    assert!(
        harness.find("match-000.txt").is_some(),
        "{}",
        harness.screen()
    );
}
