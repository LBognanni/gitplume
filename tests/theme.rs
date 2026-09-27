mod common;

use common::{Harness, state};
use crossterm::event::KeyCode;
use gitplume::git::GitError;
use ratatui::style::{Color, Modifier};

const TEXT_FLOOR: f64 = 4.5;
const INDICATOR_FLOOR: f64 = 3.0;

fn luminance(color: Color) -> f64 {
    let Color::Rgb(r, g, b) = color else {
        panic!("expected an explicit RGB color, got {color:?}");
    };
    let linear = |channel: u8| {
        let value = channel as f64 / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

fn contrast(first: Color, second: Color) -> f64 {
    let (a, b) = (luminance(first), luminance(second));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn cell_contrast(harness: &Harness, (x, y): (u16, u16)) -> f64 {
    let cell = &harness.buffer()[(x, y)];
    contrast(cell.fg, cell.bg)
}

fn background(harness: &Harness, (x, y): (u16, u16)) -> Color {
    harness.buffer()[(x, y)].bg
}

#[test]
fn surfaces_and_text_are_readable() {
    let mut harness = Harness::new(Ok(state(&[], &["file0.txt", "file1.txt"])));
    let row = harness.at("file0.txt");
    let panel_title = harness.at("Unstaged");
    let viewer_title = (40, 2);
    let diff_view = (60, 10);
    let status = harness.at("Branch");
    let splitter = (30, 10);
    for (name, position) in [
        ("list row", row),
        ("panel title", panel_title),
        ("viewer title", viewer_title),
        ("diff view", diff_view),
        ("status bar", status),
        ("active tab", harness.at("Changes")),
        ("inactive tab", harness.at("Files")),
    ] {
        assert!(cell_contrast(&harness, position) >= TEXT_FLOOR, "{name}");
    }

    // Panel chrome is visually separate from the canvas it sits on.
    assert_ne!(
        background(&harness, viewer_title),
        background(&harness, splitter)
    );
    assert_ne!(background(&harness, row), background(&harness, splitter));

    harness.press(KeyCode::Char('2'));
    assert!(cell_contrast(&harness, (60, 10)) >= TEXT_FLOOR, "preview");
}

#[test]
fn shortcuts_dialog_and_toasts_are_readable() {
    let error = GitError::Failed {
        code: Some(128),
        stderr: "fatal: bad index".to_string(),
    };
    let harness = Harness::with(Err(error), true, 100, 30);
    for text in [
        "Keyboard Shortcuts",
        "Mouse controls",
        "Close",
        "Could not refresh status",
        "fatal: bad index",
    ] {
        assert!(
            cell_contrast(&harness, harness.at(text)) >= TEXT_FLOOR,
            "{text}"
        );
    }
    // The error title is styled apart from its body.
    let title = &harness.buffer()[harness.at("Could not")];
    let body = &harness.buffer()[harness.at("fatal")];
    assert_ne!(title.fg, body.fg);
    assert!(title.modifier.contains(Modifier::BOLD));
}

#[test]
fn changes_layout_has_fixed_sidebar_and_right_aligned_change_buttons() {
    let harness = Harness::new(Ok(state(&["a.txt"], &["b.txt"])));
    for title in ["Staged", "Unstaged", "Commits"] {
        assert_eq!(harness.at(title).0, 1, "{title}");
    }
    // The vertical splitter sits right after the 30-column sidebar.
    assert_eq!(harness.buffer()[(30, 10)].symbol(), "│");
    let (up, row) = harness.at("↑");
    let (down, _) = harness.at("↓");
    assert_eq!(row, 2);
    assert_eq!(down, up + 3);
    // Right aligned: only the bar's one-cell padding follows the buttons.
    assert_eq!(down + 2, 100 - 1);
}

#[test]
fn long_status_path_occupies_one_row() {
    let long = format!(
        "src/{}final_file_name.py",
        "very_long_directory_name/".repeat(8)
    );
    let harness = Harness::new(Ok(state(&[], &[&long, "short.txt"])));
    let (_, long_row) = harness.at("[ ] M src/very");
    let (_, short_row) = harness.at("short.txt");
    assert_eq!(short_row, long_row + 1);
    // The row is cut at the list border rather than spilling past it.
    assert!(!harness.screen().contains("final_file_name"));
    assert_eq!(harness.buffer()[(29, long_row)].symbol(), "│");
}

/// Hovering, highlighting, and focusing rows gives four distinct, readable states.
#[test]
fn list_row_states_keep_a_readable_hierarchy() {
    let mut harness = Harness::new(Ok(state(&[], &["first.txt", "second.txt", "third.txt"])));
    let first = harness.at("first.txt");
    let second = harness.at("second.txt");
    let third = harness.at("third.txt");
    // Startup highlights the first row; move focus away from the list.
    harness.press(KeyCode::Tab);

    let ordinary = background(&harness, third);
    assert!(cell_contrast(&harness, third) >= TEXT_FLOOR);

    harness.hover(third);
    let hovered = background(&harness, third);
    assert!(cell_contrast(&harness, third) >= TEXT_FLOOR);

    harness.hover(second);
    assert_eq!(background(&harness, third), ordinary);
    let highlighted = background(&harness, first);
    assert!(cell_contrast(&harness, first) >= TEXT_FLOOR);

    harness.press(KeyCode::BackTab);
    let focused = background(&harness, first);
    assert!(cell_contrast(&harness, first) >= TEXT_FLOOR);
    assert!(harness.buffer()[first].modifier.contains(Modifier::BOLD));
    let border = &harness.buffer()[(0, first.1)];
    assert!(contrast(border.fg, border.bg) >= INDICATOR_FLOOR);

    // Hovering the focused selection must not weaken the selection.
    harness.hover(first);
    assert_eq!(background(&harness, first), focused);

    let states = [ordinary, hovered, highlighted, focused];
    for (index, state) in states.iter().enumerate() {
        assert!(!states[index + 1..].contains(state), "{states:?}");
    }
}

/// Row actions stay readable wherever they are revealed.
#[test]
fn row_actions_are_readable_when_revealed() {
    let mut harness = Harness::new(Ok(state(&[], &["first.txt", "second.txt"])));
    let action = |harness: &Harness, y: u16, glyph: &str| {
        let line: Vec<String> = (0..30)
            .map(|x| harness.buffer()[(x, y)].symbol().to_string())
            .collect();
        let x = line.iter().position(|cell| cell == glyph);
        x.map(|x| (x as u16, y))
            .unwrap_or_else(|| panic!("{glyph} not in row {y}:\n{}", harness.screen()))
    };
    let (_, first) = harness.at("first.txt");
    let (_, second) = harness.at("second.txt");

    // Revealed by the highlight in the focused list.
    for glyph in ["↑", "↶"] {
        let cell = action(&harness, first, glyph);
        assert!(
            cell_contrast(&harness, cell) >= TEXT_FLOOR,
            "focused {glyph}"
        );
    }
    // Revealed by hovering the row, then hovering the action itself.
    harness.hover(harness.at("second.txt"));
    for glyph in ["↑", "↶"] {
        let cell = action(&harness, second, glyph);
        assert!(
            cell_contrast(&harness, cell) >= TEXT_FLOOR,
            "hovered row {glyph}"
        );
        harness.hover(cell);
        assert!(
            cell_contrast(&harness, cell) >= TEXT_FLOOR,
            "hovered {glyph}"
        );
    }
}

/// Open `a.txt` at 100x30 with `patch` as its loaded diff.
fn diff_harness(patch: &str) -> Harness {
    let mut harness = Harness::new(Ok(state(&[], &["a.txt"])));
    harness.git.set_diff("a.txt", Ok(patch.to_string()));
    harness.press(KeyCode::Enter);
    harness
}

#[test]
fn diff_rows_style_additions_and_removals() {
    let harness = diff_harness(
        "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1,2 +1,2 @@\n context\n+added\n-removed\n",
    );
    let mut backgrounds = Vec::new();
    for (index, marker) in ["context", "added", "removed"].into_iter().enumerate() {
        let position = harness.at(marker);
        assert_eq!(position.1, 3 + index as u16, "{marker}");
        assert!(cell_contrast(&harness, position) >= TEXT_FLOOR, "{marker}");
        backgrounds.push(background(&harness, position));
    }
    assert_ne!(backgrounds[0], backgrounds[1]);
    assert_ne!(backgrounds[0], backgrounds[2]);
    assert_ne!(backgrounds[1], backgrounds[2]);
    // The marker starts each changed row in the pane.
    assert_eq!(harness.buffer()[(31, 4)].symbol(), "+");
    assert_eq!(harness.buffer()[(31, 5)].symbol(), "-");
}

#[test]
fn selected_diff_text_is_readable() {
    let mut harness =
        diff_harness("--- a/a.txt\n+++ b/a.txt\n@@ -1,1 +1,1 @@\n selected words here\n");
    let start = harness.at("selected");
    let rest = harness.at("here");
    let plain = background(&harness, start);
    harness.drag(start, (start.0 + 8, start.1));
    let selected = background(&harness, start);
    assert_ne!(selected, plain);
    assert_eq!(background(&harness, rest), plain);
    assert!(cell_contrast(&harness, start) >= TEXT_FLOOR);
    assert!(harness.line(start.1).contains("selected words here"));
}

#[test]
fn scrollbars_are_distinguishable_from_the_code_view() {
    let mut patch = String::from("--- a/a.txt\n+++ b/a.txt\n@@ -1,100 +1,100 @@\n");
    for _ in 0..100 {
        patch.push_str(&format!(" {}\n", "x".repeat(300)));
    }
    let harness = diff_harness(&patch);
    let view = harness.app.diff_view.viewport();
    // Thumbs start at the top-left of each track.
    let vertical_thumb = (view.right(), view.y);
    let vertical_track = (view.right(), view.bottom() - 1);
    let horizontal_thumb = (view.x, view.bottom());
    let horizontal_track = (view.right() - 1, view.bottom());
    for (thumb, track) in [
        (vertical_thumb, vertical_track),
        (horizontal_thumb, horizontal_track),
    ] {
        let contrast = contrast(background(&harness, thumb), background(&harness, track));
        assert!(contrast >= INDICATOR_FLOOR, "{thumb:?} {track:?}");
    }
}

#[test]
fn long_diff_title_keeps_navigation_visible_and_right_aligned() {
    let long = format!(
        "src/{}final_file_name.py",
        "very_long_directory_name/".repeat(8)
    );
    let mut harness = Harness::new(Ok(state(&[], &[&long])));
    let mut patch = String::from("--- a/a\n+++ b/a\n@@ -1,18 +1,20 @@\n");
    for index in 0..20 {
        let marker = if index == 2 || index == 10 { '+' } else { ' ' };
        patch.push_str(&format!("{marker}line {index}\n"));
    }
    harness.git.set_diff(&long, Ok(patch));
    harness.press(KeyCode::Enter);

    let title = harness.line(2);
    let pane: String = title.chars().skip(31).collect();
    assert!(pane.starts_with(" src/very_long_directory_name/"), "{pane}");
    assert!(!harness.screen().contains("final_file_name"));
    let up = pane
        .chars()
        .position(|c| c == '↑')
        .expect("previous button");
    let down = pane.chars().position(|c| c == '↓').expect("next button");
    assert_eq!(down, up + 3);
    // Right aligned: only the bar's one-cell padding follows the buttons.
    assert_eq!(31 + down + 2, 100 - 1);
    // Navigation is enabled: the diff has a second change. At the first
    // change, the previous button is the disabled (dimmed) one.
    let buffer = harness.buffer();
    assert!(
        !buffer[(31 + down as u16, 2)]
            .modifier
            .contains(Modifier::DIM)
    );
    assert!(buffer[(31 + up as u16, 2)].modifier.contains(Modifier::DIM));
}
