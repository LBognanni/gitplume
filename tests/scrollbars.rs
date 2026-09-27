//! Scrollbars on the sidebar lists and trees, shared with the viewers.
mod common;

use common::{Harness, state};
use crossterm::event::{KeyCode, MouseEventKind};
use gitplume::model::Commit;
use ratatui::style::{Color, Modifier};

const LONG_ROOT: &str = "/a/very/long/launch/directory/path/that/overflows";

/// The thumb color at the top of a long diff's vertical scrollbar.
fn viewer_thumb() -> Color {
    let mut patch = String::from("--- a/a.txt\n+++ b/a.txt\n@@ -1,100 +1,100 @@\n");
    for _ in 0..100 {
        patch.push_str(" line\n");
    }
    let mut harness = Harness::new(Ok(state(&[], &["a.txt"])));
    harness.git.set_diff("a.txt", Ok(patch));
    harness.press(KeyCode::Enter);
    let view = harness.app.diff_view.viewport();
    harness.buffer()[(view.right(), view.y)].bg
}

fn files_tab(root: &str, count: usize) -> Harness {
    let files: Vec<String> = (0..count).map(|i| format!("file{i:02}.txt")).collect();
    let files: Vec<&str> = files.iter().map(String::as_str).collect();
    let root = std::path::Path::new(root);
    let mut harness = Harness::launched(root, root, &files);
    harness.press(KeyCode::Char('2'));
    harness
}

fn bg(harness: &Harness, position: (u16, u16)) -> Color {
    harness.buffer()[position].bg
}

/// The last column inside the list or tree border at `row`.
fn bar_column(harness: &Harness, row: u16) -> u16 {
    let line: Vec<char> = harness.line(row).chars().collect();
    let right = (1..line.len()).find(|&x| line[x] == '│').unwrap();
    right as u16 - 1
}

#[test]
fn scrollbar_thumbs_share_one_blue_color() {
    let thumb = viewer_thumb();
    let Color::Rgb(r, _, b) = thumb else {
        panic!("expected an RGB thumb, got {thumb:?}");
    };
    assert!(b > r.saturating_add(64), "{thumb:?}");
    let harness = files_tab("/repo", 40);
    let x = bar_column(&harness, 3);
    assert_eq!(bg(&harness, (x, 3)), thumb);
}

#[test]
fn files_tree_shows_scrollbars_only_when_it_overflows() {
    let small = files_tab("/repo", 3);
    let x = bar_column(&small, 4);
    assert_eq!(bg(&small, (x, 4)), bg(&small, (x - 5, 4)));
    let bottom = small.find("└──").unwrap().1 - 1;
    assert_eq!(small.buffer()[(1, bottom)], small.buffer()[(5, bottom)]);

    let thumb = viewer_thumb();
    let tall = files_tab("/repo", 40);
    assert_eq!(bg(&tall, (x, 4)), thumb, "vertical thumb");
    let wide = files_tab(LONG_ROOT, 3);
    let bottom = wide.find("└──").unwrap().1 - 1;
    assert_eq!(wide.buffer()[(1, bottom)].fg, thumb, "horizontal thumb");
}

#[test]
fn files_tree_horizontal_bar_is_a_half_height_bar_on_the_pane() {
    let wide = files_tab(LONG_ROOT, 3);
    let (_, row) = wide.at("file00.txt");
    let bottom = wide.find("└──").unwrap().1 - 1;
    let pane = wide.buffer()[(bar_column(&wide, row) - 1, row)].bg;
    let thumb = &wide.buffer()[(1, bottom)];
    let track = &wide.buffer()[(bar_column(&wide, row), bottom)];
    assert_eq!(thumb.symbol(), "▄");
    assert_eq!(track.symbol(), "▄");
    assert_ne!(thumb.fg, track.fg);
    assert_eq!(thumb.bg, pane);
    assert_eq!(track.bg, pane);
}

#[test]
fn wheel_scrolls_the_files_tree_without_moving_the_cursor() {
    let mut harness = files_tab(LONG_ROOT, 40);
    assert!(harness.line(3).contains("/a/very"));
    harness.scroll(MouseEventKind::ScrollDown, (10, 10));
    assert!(
        harness.line(3).contains("file02.txt"),
        "{}",
        harness.screen()
    );
    let x = bar_column(&harness, 3);
    assert_ne!(bg(&harness, (x, 3)), viewer_thumb(), "the thumb moved down");
    harness.scroll(MouseEventKind::ScrollUp, (10, 10));
    harness.scroll(MouseEventKind::ScrollRight, (10, 10));
    harness.scroll(MouseEventKind::ScrollRight, (10, 10));
    assert!(!harness.line(3).contains("/a/very"), "{}", harness.screen());
    assert!(harness.line(3).contains("long"), "{}", harness.screen());

    // The cursor stayed on the root: the next key moves it to the first file
    // and scrolls that row into view (cut by the horizontal scroll).
    harness.scroll(MouseEventKind::ScrollDown, (10, 10));
    harness.scroll(MouseEventKind::ScrollDown, (10, 10));
    harness.press(KeyCode::Char('j'));
    assert!(
        harness.line(3).contains("ile00.txt"),
        "{}",
        harness.screen()
    );
}

#[test]
fn commit_tree_and_status_lists_show_scrollbars_when_they_overflow() {
    let unstaged: Vec<String> = (0..40).map(|i| format!("file{i:02}.txt")).collect();
    let unstaged: Vec<&str> = unstaged.iter().map(String::as_str).collect();
    let mut harness = Harness::new(Ok(state(&[], &unstaged)));
    let commits: Vec<Commit> = (0..40)
        .map(|i| Commit {
            hash: format!("{i:040}"),
            short_hash: format!("{i:07}"),
            parent: None,
            subject: format!("Commit {i:02} with a subject far too long for the sidebar"),
        })
        .collect();
    *harness.git.commits.lock().unwrap() = Ok(commits);
    harness.press(KeyCode::Char('r'));
    let thumb = viewer_thumb();

    let (_, row) = harness.at("file00.txt");
    assert_eq!(bg(&harness, (bar_column(&harness, row), row)), thumb);

    let (_, row) = harness.at("Commit 00");
    assert_eq!(bg(&harness, (bar_column(&harness, row), row)), thumb);
    let bottom = (row..30)
        .find(|&y| harness.line(y).starts_with('└'))
        .unwrap()
        - 1;
    assert_eq!(harness.buffer()[(1, bottom)].fg, thumb, "horizontal thumb");
}

#[test]
fn section_splitters_sit_on_the_sidebar_surface() {
    let harness = Harness::new(Ok(state(&["a.txt"], &["b.txt", "c.txt"])));
    let (_, title) = harness.at("Unstaged");
    let splitter = (5, title - 1);
    assert_eq!(harness.buffer()[splitter].symbol(), "─");
    // An unhighlighted row shows the sidebar's surface.
    let row = harness.at("c.txt");
    assert_eq!(bg(&harness, splitter), bg(&harness, row));
}

#[test]
fn unfocused_tree_cursor_uses_the_inactive_selection() {
    let mut harness = files_tab("/repo", 3);
    let root = harness.at("/repo");
    let focused = harness.buffer()[root].clone();
    harness.press(KeyCode::Tab);
    let unfocused = harness.buffer()[root].clone();
    assert_ne!(unfocused.bg, focused.bg);
    assert_ne!(unfocused.bg, bg(&harness, harness.at("file01.txt")));
    assert!(!unfocused.modifier.contains(Modifier::BOLD));
}

#[test]
fn wide_glyphs_cut_by_the_tree_edge_never_cover_the_border() {
    let mut harness = Harness::new(Ok(state(&[], &[])));
    let commits: Vec<Commit> = (20..32)
        .map(|i| Commit {
            hash: format!("{i:040}"),
            short_hash: format!("{i:07}"),
            parent: None,
            subject: format!("{}🐛🐛", "x".repeat(i)),
        })
        .collect();
    *harness.git.commits.lock().unwrap() = Ok(commits);
    harness.press(KeyCode::Char('r'));
    let (_, first) = harness.at("xxxxxxxxxxxxxxxxxxxx");
    let top: Vec<char> = harness.line(first - 1).chars().collect();
    let border = top.iter().position(|&c| c == '┐').unwrap() as u16;
    for y in (first..).take_while(|&y| !harness.line(y).starts_with('└')) {
        assert_eq!(
            harness.buffer()[(border, y)].symbol(),
            "│",
            "{}",
            harness.screen()
        );
    }
}

/// A harness with 40 unstaged files, and the (column, first row, last row)
/// of the Unstaged list's vertical scrollbar.
fn long_unstaged() -> (Harness, (u16, u16, u16)) {
    let files: Vec<String> = (0..40).map(|i| format!("file{i:02}.txt")).collect();
    let files: Vec<&str> = files.iter().map(String::as_str).collect();
    let harness = Harness::new(Ok(state(&[], &files)));
    let (_, top) = harness.at("file00.txt");
    let x = bar_column(&harness, top);
    let bottom = (top..).find(|&y| harness.line(y).starts_with('└')).unwrap() - 1;
    (harness, (x, top, bottom))
}

/// The Files tree's (vertical bar column, first row, last row).
fn tree_bar(harness: &Harness) -> (u16, u16, u16) {
    let (_, top) = harness.at("/");
    let x = bar_column(harness, top);
    let bottom = (top..).find(|&y| harness.line(y).starts_with('└')).unwrap() - 1;
    (x, top, bottom)
}

/// The scroll position for a thumb dragged `moved` cells on a `track` showing
/// `window` of `size`, like the viewers' scrollbars.
fn dragged(moved: usize, track: usize, size: usize, window: usize) -> usize {
    let len = (track * window / size).max(1);
    moved * (size - window) / (track - len)
}

#[test]
fn clicking_a_list_track_jumps_without_selecting_a_row() {
    let (mut harness, (x, _, bottom)) = long_unstaged();
    let highlight = harness.app.unstaged.highlight;
    harness.click((x, bottom));
    harness.mouse_up((x, bottom));
    assert!(harness.find("file39.txt").is_some(), "{}", harness.screen());
    assert!(harness.find("file00.txt").is_none());
    assert_eq!(harness.app.unstaged.highlight, highlight);
    assert!(!harness.git.calls().iter().any(|c| c.starts_with("diff")));
}

#[test]
fn clicking_a_tree_track_jumps_without_moving_the_cursor() {
    let mut harness = files_tab("/repo", 40);
    let (x, _, bottom) = tree_bar(&harness);
    let cursor = harness.app.files.cursor;
    harness.click((x, bottom));
    harness.mouse_up((x, bottom));
    assert!(harness.find("file39.txt").is_some(), "{}", harness.screen());
    assert!(harness.find("/repo").is_none());
    assert_eq!(harness.app.files.cursor, cursor);
}

#[test]
fn dragging_a_list_thumb_scrolls_proportionally() {
    let (mut harness, (x, top, bottom)) = long_unstaged();
    let track = (bottom - top + 1) as usize;
    harness.mouse_down((x, top));
    harness.drag_to((x, top + 3));
    let first = dragged(3, track, 40, track);
    assert!(first > 0);
    assert!(
        harness.line(top).contains(&format!("file{first:02}.txt")),
        "{}",
        harness.screen()
    );
    harness.mouse_up((x, top + 3));
}

#[test]
fn dragging_a_tree_thumb_scrolls_both_axes_proportionally() {
    let mut harness = files_tab("/repo", 40);
    let (x, top, bottom) = tree_bar(&harness);
    let track = (bottom - top + 1) as usize;
    harness.mouse_down((x, top));
    harness.drag_to((x, top + 3));
    harness.mouse_up((x, top + 3));
    // The root row comes first, so row `n` shows file `n - 1`.
    let file = dragged(3, track, 41, track) - 1;
    assert!(
        harness.line(top).contains(&format!("file{file:02}.txt")),
        "{}",
        harness.screen()
    );

    // The horizontal thumb follows the pointer cell for cell.
    let mut wide = files_tab(LONG_ROOT, 3);
    let (_, root) = wide.at("/a/very");
    let bottom = (root..).find(|&y| wide.line(y).starts_with('└')).unwrap() - 1;
    let thumb = viewer_thumb();
    let thumb_start = |h: &Harness| {
        (1..)
            .find(|&x| h.buffer()[(x, bottom)].fg == thumb)
            .unwrap()
    };
    assert_eq!(thumb_start(&wide), 1);
    wide.mouse_down((1, bottom));
    wide.drag_to((4, bottom));
    wide.mouse_up((4, bottom));
    assert!((3..=4).contains(&thumb_start(&wide)), "{}", wide.screen());
    assert!(!wide.line(root).contains("/a/very"), "{}", wide.screen());
}

#[test]
fn a_thumb_drag_outside_the_bar_keeps_scrolling_and_selects_nothing() {
    let (mut harness, (x, top, _)) = long_unstaged();
    let highlight = harness.app.unstaged.highlight;
    harness.mouse_down((x, top));
    // Wander over the rows and past the bottom of the screen.
    harness.drag_to((5, top + 2));
    assert!(harness.find("file00.txt").is_none(), "{}", harness.screen());
    harness.drag_to((5, 29));
    assert!(harness.find("file39.txt").is_some(), "{}", harness.screen());
    harness.mouse_up((5, 29));
    assert_eq!(harness.app.unstaged.highlight, highlight);
    assert!(!harness.git.calls().iter().any(|c| c.starts_with("diff")));
    // The release ended the drag.
    harness.hover((x, top));
    assert!(harness.find("file39.txt").is_some());
}

#[test]
fn a_wide_glyph_cut_by_the_left_edge_keeps_the_cursor_row_background() {
    let mut harness = Harness::new(Ok(state(&[], &[])));
    let commits = vec![Commit {
        hash: "a".repeat(40),
        short_hash: "a".repeat(7),
        parent: None,
        subject: format!("commit{}", "🐛".repeat(40)),
    }];
    *harness.git.commits.lock().unwrap() = Ok(commits);
    harness.press(KeyCode::Char('r'));
    let (x, y) = harness.at("commit🐛");
    harness.click((x, y));
    let cursor = bg(&harness, (x, y));
    let left = (0..x)
        .rev()
        .find(|&c| harness.line(y).chars().nth(c as usize) == Some('│'))
        .unwrap()
        + 1;
    // Wheel steps of three cells cut a glyph on one of two neighbouring steps.
    let mut cut = false;
    for _ in 0..8 {
        harness.scroll(MouseEventKind::ScrollRight, (x, y));
        let cell = &harness.buffer()[(left, y)];
        assert_eq!(cell.bg, cursor, "{}", harness.screen());
        cut |= cell.symbol() == " ";
    }
    assert!(cut, "{}", harness.screen());
}
