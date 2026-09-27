mod common;

use common::{Harness, state};
use crossterm::event::KeyCode;
use gitplume::app::Focus;
use ratatui::style::Color;

fn harness(width: u16, height: u16) -> Harness {
    Harness::with(Ok(state(&["a.txt"], &["b.txt"])), false, width, height)
}

/// Sidebar width: the splitter is the first `│` on the sections' title row.
fn sidebar_width(harness: &Harness) -> u16 {
    let (_, row) = harness.at("Staged");
    harness
        .line(row)
        .chars()
        .position(|c| c == '│')
        .expect("sidebar splitter") as u16
}

fn diff_width(harness: &Harness) -> u16 {
    harness.buffer().area.width - sidebar_width(harness) - 1
}

/// Heights of the Staged, Unstaged, and Commits sections, title rows included.
fn section_heights(harness: &Harness) -> [u16; 3] {
    let staged = harness.at("Staged").1;
    let unstaged = harness.at("Unstaged").1;
    let commits = harness.at("Commits").1;
    let status = harness.buffer().area.height - 1;
    [
        unstaged - staged - 1,
        commits - unstaged - 1,
        status - commits,
    ]
}

/// Files tree width: its top-right border corner is the last tree column.
fn tree_width(harness: &Harness) -> u16 {
    harness.at("┐").0 + 1
}

fn sidebar_splitter(harness: &Harness) -> (u16, u16) {
    (sidebar_width(harness), 10)
}

/// The row of the splitter below section `index`.
fn section_splitter(harness: &Harness, index: usize) -> (u16, u16) {
    let title = ["Unstaged", "Commits"][index];
    (5, harness.at(title).1 - 1)
}

fn long_diff() -> String {
    let mut patch = String::from("diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n");
    patch.push_str("@@ -1,100 +1,100 @@\n");
    for index in 0..100 {
        patch.push_str(&format!(" {index} {}\n", "x".repeat(200)));
    }
    patch
}

#[test]
fn dragging_sidebar_splitter_moves_only_adjacent_panes() {
    let mut harness = harness(100, 30);
    let (sidebar, diff) = (sidebar_width(&harness), diff_width(&harness));
    let (x, y) = sidebar_splitter(&harness);
    harness.drag((x, y), (x + 5, y));
    assert_eq!(sidebar_width(&harness), sidebar + 5);
    assert_eq!(diff_width(&harness), diff - 5);
    // The pointer left the splitter without focusing the diff.
    assert_eq!(harness.app.focus, Focus::Staged);
}

#[test]
fn dragging_a_splitter_across_the_diff_never_selects_text() {
    let mut harness = harness(100, 30);
    harness
        .app
        .diff_view
        .set_document(gitplume::document::diff_document("a.txt", &long_diff()));
    harness.draw();
    let (x, y) = sidebar_splitter(&harness);
    harness.mouse_down((x, y));
    // Real button-held drag events that sweep across the diff text.
    for step in 1..=6 {
        harness.drag_to((x + step * 5, y + step));
    }
    harness.mouse_up((x + 30, y + 6));
    assert_eq!(harness.app.diff_view.selected_text(), None);
    assert!(harness.copied.is_empty());
    assert_eq!(harness.app.focus, Focus::Staged);
}

#[test]
fn drag_continues_after_leaving_the_splitter_until_release() {
    let mut harness = harness(100, 30);
    let (x, y) = sidebar_splitter(&harness);
    harness.mouse_down((x, y));
    harness.hover((x + 3, y));
    harness.hover((x + 8, y + 4));
    assert_eq!(sidebar_width(&harness), x + 8);
    harness.mouse_up((x + 8, y));
    harness.hover((x + 20, y));
    // Released: plain motion no longer resizes.
    assert_eq!(sidebar_width(&harness), x + 8);
}

#[test]
fn sidebar_minimums_clamp_in_both_directions() {
    let mut harness = harness(100, 30);
    let (x, y) = sidebar_splitter(&harness);
    harness.drag((x, y), (0, y));
    assert_eq!(sidebar_width(&harness), 15);
    let (x, y) = sidebar_splitter(&harness);
    harness.drag((x, y), (99, y));
    assert_eq!(diff_width(&harness), 15);
}

#[test]
fn section_minimums_clamp_in_both_directions() {
    let mut harness = harness(100, 30);
    let (x, y) = section_splitter(&harness, 0);
    harness.drag((x, y), (x, 0));
    assert_eq!(section_heights(&harness)[0], 3);
    let (x, y) = section_splitter(&harness, 1);
    harness.drag((x, y), (x, 29));
    assert_eq!(section_heights(&harness)[2], 3);
}

#[test]
fn dragging_first_section_splitter_keeps_commits_height() {
    let mut harness = harness(100, 30);
    let before = section_heights(&harness);
    let (x, y) = section_splitter(&harness, 0);
    harness.drag((x, y), (x, y + 2));
    let after = section_heights(&harness);
    assert_eq!(after, [before[0] + 2, before[1] - 2, before[2]]);
}

#[test]
fn sidebar_stays_fixed_on_resize_before_any_drag() {
    let mut harness = harness(100, 30);
    for (width, height) in [(140, 40), (80, 24)] {
        harness.resize(width, height);
        assert_eq!(sidebar_width(&harness), 30);
    }
}

#[test]
fn resizing_keeps_usable_panes_and_scrolling() {
    let mut harness = harness(100, 30);
    harness
        .app
        .diff_view
        .set_document(gitplume::document::diff_document("a.txt", &long_diff()));
    let (x, y) = sidebar_splitter(&harness);
    harness.drag((x, y), (x + 5, y));
    let (x, y) = section_splitter(&harness, 0);
    harness.drag((x, y), (x, y + 2));
    let ratio = sidebar_width(&harness) as f64 / diff_width(&harness) as f64;
    let heights = section_heights(&harness);

    for (width, height) in [(140, 40), (80, 24), (60, 21)] {
        harness.resize(width, height);
        let (sidebar, diff) = (sidebar_width(&harness), diff_width(&harness));
        assert!(sidebar >= 15 && diff >= 10, "{width}x{height}");
        assert_eq!(sidebar + 1 + diff, width);
        let expected = ratio * diff as f64;
        assert!((sidebar as f64 - expected).abs() <= 1.0, "{width}x{height}");
        let now = section_heights(&harness);
        let scale = now.iter().sum::<u16>() as f64 / heights.iter().sum::<u16>() as f64;
        for (was, is) in heights.iter().zip(now) {
            // Each list (section minus its title row) keeps at least 3 rows.
            assert!(is > 3, "{width}x{height}: {now:?}");
            assert!((*was as f64 * scale - is as f64).abs() <= 1.0, "{now:?}");
        }
        let view = &harness.app.diff_view;
        assert!(view.viewport().height > 0);
        let (max_x, max_y) = view.max_scroll();
        assert!(max_x > 0 && max_y > 0, "{width}x{height}");
    }

    harness.press(KeyCode::Char('w'));
    assert!(harness.app.diff_view.wrapped());
    let view = &harness.app.diff_view;
    assert_eq!(view.max_scroll().0, 0);
    assert!(view.max_scroll().1 > 0);
    assert!(view.virtual_size().0 <= view.viewport().width as usize);

    harness.resize(140, 40);
    let view = &harness.app.diff_view;
    assert_eq!(view.max_scroll().0, 0);
    assert!(view.max_scroll().1 > 0);
}

#[test]
fn resizing_without_a_drag_keeps_usable_panes_and_scrolling() {
    let mut harness = harness(100, 30);
    harness
        .app
        .diff_view
        .set_document(gitplume::document::diff_document("a.txt", &long_diff()));

    for (width, height) in [(140, 40), (80, 24), (60, 20)] {
        harness.resize(width, height);
        let (sidebar, diff) = (sidebar_width(&harness), diff_width(&harness));
        assert!(sidebar >= 15 && diff >= 10, "{width}x{height}");
        for section in section_heights(&harness) {
            // Each list (section minus its title row) keeps at least 3 rows.
            assert!(section > 3, "{width}x{height}: {section}");
        }
        let view = harness.app.diff_view.viewport();
        assert!(view.height > 0);
        let (max_x, max_y) = harness.app.diff_view.max_scroll();
        assert!(max_x > 0 && max_y > 0, "{width}x{height}");
        // Both scrollbars take space beside the text, above the status bar.
        assert!(view.right() < width, "vertical scrollbar {width}x{height}");
        assert!(
            view.bottom() < height - 1,
            "horizontal scrollbar {width}x{height}"
        );
        // Both thumbs start at the scroll origin and share one color.
        let buffer = harness.buffer();
        let thumb = buffer[(view.right(), view.y)].bg;
        assert_ne!(thumb, buffer[(view.x, view.y)].bg, "{width}x{height}");
        assert_eq!(
            buffer[(view.x, view.bottom())].bg,
            thumb,
            "{width}x{height}"
        );
    }

    harness.press(KeyCode::Char('w'));
    assert!(harness.app.diff_view.wrapped());
    let view = &harness.app.diff_view;
    assert_eq!(view.max_scroll().0, 0);
    assert!(view.max_scroll().1 > 0);
    assert!(view.virtual_size().0 <= view.viewport().width as usize);
    // No horizontal scrollbar: the row below the text is the status bar.
    let status_row = harness.buffer().area.height - 1;
    assert_eq!(view.viewport().bottom(), status_row);

    harness.resize(140, 40);
    let view = &harness.app.diff_view;
    assert_eq!(view.max_scroll().0, 0);
    assert!(view.max_scroll().1 > 0);
}

#[test]
fn tiny_terminals_never_panic() {
    let mut harness = harness(100, 30);
    let (x, y) = sidebar_splitter(&harness);
    harness.drag((x, y), (x + 5, y));
    for (width, height) in [(20, 8), (5, 3), (1, 1), (0, 0), (100, 30)] {
        harness.resize(width, height);
    }
    assert_eq!(sidebar_width(&harness), 35);
}

#[test]
fn files_splitter_resizes_tree_and_preview() {
    let mut harness = harness(100, 30);
    harness.press(KeyCode::Char('2'));
    let tree = tree_width(&harness);
    assert_eq!(tree, 30);
    harness.drag((tree, 10), (tree + 5, 10));
    assert_eq!(tree_width(&harness), 35);
    harness.drag((35, 10), (0, 10));
    assert_eq!(tree_width(&harness), 15);
    harness.resize(200, 30);
    assert_eq!(tree_width(&harness), 30);
    // The Changes sidebar is a separate group.
    harness.press(KeyCode::Char('1'));
    assert_eq!(sidebar_width(&harness), 30);
}

#[test]
fn splitters_render_directional_lines() {
    let harness = harness(100, 30);
    let x = sidebar_width(&harness);
    for y in 2..29 {
        assert_eq!(harness.buffer()[(x, y)].symbol(), "│", "row {y}");
    }
    let (_, y) = section_splitter(&harness, 0);
    assert_eq!(
        harness.line(y)[..]
            .chars()
            .take(x as usize)
            .collect::<String>(),
        "─".repeat(x as usize)
    );
}

fn splitter_color(harness: &Harness, position: (u16, u16)) -> Color {
    harness.buffer()[position].fg
}

#[test]
fn hovered_and_dragged_splitters_render_differently() {
    let mut harness = harness(100, 30);
    let position = sidebar_splitter(&harness);
    let section = section_splitter(&harness, 0);
    let idle = splitter_color(&harness, position);

    harness.hover(position);
    assert_ne!(splitter_color(&harness, position), idle);
    assert_eq!(splitter_color(&harness, section), idle);

    harness.hover((60, 10));
    assert_eq!(splitter_color(&harness, position), idle);

    harness.mouse_down(position);
    harness.hover((position.0 + 3, 20));
    let moved = sidebar_splitter(&harness);
    assert_ne!(splitter_color(&harness, moved), idle);
    harness.mouse_up((moved.0, 20));
    harness.hover((60, 10));
    assert_eq!(splitter_color(&harness, sidebar_splitter(&harness)), idle);
}
