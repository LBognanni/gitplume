mod common;

use common::{Harness, TempDir};
use crossterm::event::KeyCode;
use gitplume::code_view::scrollbar_click_target;

#[test]
fn file_selection_shows_repository_relative_path() {
    let dir = TempDir::new("preview-title");
    let cwd = dir.0.join("nested");
    std::fs::create_dir_all(cwd.join("deeper")).unwrap();
    std::fs::write(cwd.join("deeper/[bold]report.txt"), "answer = 42\n").unwrap();
    let mut harness = Harness::launched(&dir.0, &cwd, &["deeper/[bold]report.txt"]);
    harness.press(KeyCode::Char('2'));
    // Root, then the folder: expand it, then open its file.
    harness.press(KeyCode::Char('j'));
    harness.press(KeyCode::Enter);
    harness.press(KeyCode::Char('j'));
    harness.press(KeyCode::Enter);

    let title = harness.line(2);
    assert!(title.contains("nested/deeper/[bold]report.txt"), "{title}");
    assert!(harness.screen().contains("answer = 42"));
}

#[test]
fn loaded_preview_uses_current_wrap_setting() {
    let dir = TempDir::new("preview-wrap");
    let long = format!("start {} tail-marker", "x".repeat(150));
    std::fs::write(dir.0.join("long.txt"), format!("{long}\n")).unwrap();
    let mut harness = Harness::launched(&dir.0, &dir.0, &["long.txt"]);
    harness.press(KeyCode::Char('2'));
    harness.press(KeyCode::Char('w'));

    harness.press(KeyCode::Char('j'));
    harness.press(KeyCode::Enter);

    assert!(harness.app.preview_view.wrapped());
    assert!(!harness.app.diff_view.wrapped());
    // Wrapped, the end of the long line is visible without scrolling.
    assert!(
        harness.screen().contains("tail-marker"),
        "{}",
        harness.screen()
    );
}

#[test]
fn file_preview_scrollbar_track_click_jumps_to_clicked_position() {
    let dir = TempDir::new("preview-scrollbar");
    let text: String = (0..1000).map(|n| format!("{n}\n")).collect();
    std::fs::write(dir.0.join("numbers.txt"), text).unwrap();
    let mut harness = Harness::launched(&dir.0, &dir.0, &["numbers.txt"]);
    harness.press(KeyCode::Char('2'));
    harness.press(KeyCode::Char('j'));
    harness.press(KeyCode::Enter);

    let view = harness.app.preview_view.viewport();
    let height = view.height as usize;
    let click_y = height * 3 / 4;
    let (_, virtual_height) = harness.app.preview_view.virtual_size();
    let expected = scrollbar_click_target(click_y, height, virtual_height, height);
    harness.click((view.right(), view.y + click_y as u16));

    assert!(expected > 0);
    assert_eq!(harness.app.preview_view.scroll_offset().1, expected);
}
