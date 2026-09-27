mod common;

use common::{Harness, failure, state};
use crossterm::event::{KeyCode, KeyModifiers, MouseEventKind};
use gitplume::app::{Event, Focus, Severity};
use gitplume::document::diff_document;
use gitplume::model::{FileEntry, Side};
use gitplume::watcher::{Invalidation, Watch};
use ratatui::style::Modifier;

/// First column of the diff pane (sidebar plus splitter).
const PANE_X: u16 = 31;
/// Width of the diff gutter for documents under 10,000 lines.
const GUTTER: u16 = 12;

/// A one-hunk patch of `lines` where the rows at `changes` are additions.
fn patch(lines: &[String], changes: &[usize]) -> String {
    let context = lines.len() - changes.len();
    let mut text = format!(
        "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1,{context} +1,{} @@\n",
        lines.len()
    );
    for (index, line) in lines.iter().enumerate() {
        let marker = if changes.contains(&index) { '+' } else { ' ' };
        text.push_str(&format!("{marker}{line}\n"));
    }
    text
}

fn numbered(count: usize, format: impl Fn(usize) -> String) -> Vec<String> {
    (0..count).map(format).collect()
}

/// A harness with one unstaged `a.txt` whose diff is `patch`, already loaded.
fn open(width: u16, height: u16, patch: String) -> Harness {
    let mut harness = Harness::with(Ok(state(&[], &["a.txt"])), false, width, height);
    load(&mut harness, patch);
    harness
}

/// Serve `patch` for `a.txt` and press Enter on its highlighted row, which
/// loads the diff. The Unstaged list must have focus.
fn load(harness: &mut Harness, patch: String) {
    harness.git.set_diff("a.txt", Ok(patch));
    assert_eq!(harness.app.focus, Focus::Unstaged);
    harness.press(KeyCode::Enter);
}

fn scroll(harness: &Harness) -> (usize, usize) {
    harness.app.diff_view.scroll_offset()
}

/// Visible text of the diff pane.
fn diff_text(harness: &Harness) -> String {
    let height = harness.buffer().area.height;
    (3..height - 1)
        .map(|y| harness.line(y).chars().skip(PANE_X as usize).collect())
        .collect::<Vec<String>>()
        .join("\n")
}

/// Position of a change button in the diff title row.
fn button(harness: &Harness, glyph: char) -> (u16, u16) {
    let x = harness
        .line(2)
        .chars()
        .enumerate()
        .skip(PANE_X as usize)
        .find(|(_, c)| *c == glyph)
        .map(|(x, _)| x as u16)
        .unwrap_or_else(|| panic!("{glyph} not in the diff title:\n{}", harness.screen()));
    (x, 2)
}

fn enabled(harness: &Harness, glyph: char) -> bool {
    !harness.buffer()[button(harness, glyph)]
        .modifier
        .contains(Modifier::DIM)
}

#[test]
fn wrap_toggle_applies_independently_to_each_viewer() {
    let mut harness = Harness::new(Ok(state(&[], &[])));
    harness.press(KeyCode::Char('w'));
    assert!(harness.app.diff_view.wrapped());
    assert!(!harness.app.preview_view.wrapped());

    harness.press(KeyCode::Char('2'));
    harness.press(KeyCode::Char('w'));
    assert!(harness.app.diff_view.wrapped());
    assert!(harness.app.preview_view.wrapped());

    harness.press(KeyCode::Char('w'));
    assert!(harness.app.diff_view.wrapped());
    assert!(!harness.app.preview_view.wrapped());
}

#[test]
fn completed_text_selection_is_copied_to_clipboard() {
    let mut harness = open(80, 12, patch(&["selected text".to_string()], &[]));
    let start = PANE_X + GUTTER;
    harness.drag((start, 3), (start + 8, 3));
    assert_eq!(harness.copied, ["selected"]);
    assert_eq!(harness.app.focus, Focus::Diff);

    // The viewer keeps the drag when the pointer leaves it, until release.
    harness.mouse_down((start, 3));
    harness.drag_to((start + 4, 3));
    harness.drag_to((5, 3));
    harness.mouse_up((5, 3));
    assert_eq!(harness.copied, ["selected", "     1    1 "]);
}

#[test]
fn diff_and_preview_display_replace_scroll_and_wrap_independently() {
    let diff = numbered(500, |i| format!("diff {i} {}", "d".repeat(120)));
    let mut harness = open(80, 20, patch(&diff, &[]));
    let preview = numbered(500, |i| format!("preview {i} {}", "p".repeat(120)));
    harness
        .app
        .preview_view
        .set_document(diff_document("p.txt", &patch(&preview, &[])));

    harness.press(KeyCode::Char('2'));
    assert!(harness.screen().contains("preview 0 "));
    harness.press(KeyCode::Char('1'));
    assert!(harness.screen().contains("diff 0 "));

    harness.app.diff_view.scroll_to(0, 300);
    harness.draw();
    assert!(harness.screen().contains("diff 300 "));
    harness.press(KeyCode::Char('2'));
    assert!(harness.screen().contains("preview 0 "));
    assert_eq!(harness.app.preview_view.scroll_offset(), (0, 0));

    harness.press(KeyCode::Char('w'));
    assert!(harness.app.preview_view.wrapped());
    assert!(!harness.app.diff_view.wrapped());

    harness
        .app
        .preview_view
        .set_document(diff_document("p.txt", &patch(&["replacement".into()], &[])));
    harness.draw();
    assert!(harness.screen().contains("replacement"));
    assert!(!harness.screen().contains("preview 0 "));
    harness.press(KeyCode::Char('1'));
    assert!(harness.screen().contains("diff 300 "));
}

#[test]
fn diff_first_change_scrolls_past_leading_context() {
    let harness = open(80, 12, patch(&numbered(80, |i| format!("line {i}")), &[17]));
    assert_eq!(scroll(&harness), (0, 13));
    assert!(diff_text(&harness).contains("line 17"));
}

#[test]
fn diff_change_buttons_navigate_and_disable_at_endpoints() {
    let lines = numbered(100, |i| format!("line {i} {}", "x".repeat(100)));
    let mut harness = open(80, 12, patch(&lines, &[10, 30, 70]));
    assert_eq!(scroll(&harness).1, 6);
    assert!(!enabled(&harness, '↑'));
    assert!(enabled(&harness, '↓'));

    // A disabled button ignores clicks.
    harness.click(button(&harness, '↑'));
    assert_eq!(scroll(&harness).1, 6);

    harness.click(button(&harness, '↓'));
    assert_eq!(scroll(&harness).1, 26);
    assert!(enabled(&harness, '↑'));
    assert!(enabled(&harness, '↓'));

    harness.press(KeyCode::Char('n'));
    assert_eq!(scroll(&harness).1, 66);
    assert!(enabled(&harness, '↑'));
    assert!(!enabled(&harness, '↓'));
    harness.press(KeyCode::Char('n'));
    assert_eq!(scroll(&harness).1, 66);

    harness.click(button(&harness, '↑'));
    assert_eq!(scroll(&harness).1, 26);

    harness.press(KeyCode::Char('w'));
    harness.press(KeyCode::Char('p'));
    let wrapped_row = harness.app.diff_view.source_to_visual_row(6);
    assert!(wrapped_row > 6);
    assert_eq!(scroll(&harness), (0, wrapped_row));
    assert!(!enabled(&harness, '↑'));
    assert!(enabled(&harness, '↓'));
}

#[test]
fn change_buttons_show_hints() {
    let mut harness = open(80, 12, patch(&numbered(40, |i| format!("{i}")), &[2, 20]));
    harness.hover(button(&harness, '↓'));
    assert!(harness.line(11).contains("Next Change"));
    harness.hover(button(&harness, '↑'));
    assert!(harness.line(11).contains("Previous Change"));
}

#[test]
fn change_navigation_only_responds_on_the_changes_tab() {
    let lines = numbered(100, |i| format!("line {i}"));
    let mut harness = open(80, 12, patch(&lines, &[10, 30]));
    harness.press(KeyCode::Char('2'));
    harness.press(KeyCode::Char('n'));
    harness.press(KeyCode::Char('1'));
    assert_eq!(scroll(&harness).1, 6);
}

#[test]
fn requesting_a_diff_disables_navigation_until_it_loads() {
    let mut harness = Harness::held(Ok(state(&[], &["a.txt"])));
    let status = harness.run_next();
    harness.send(status);
    harness.git.set_diff(
        "a.txt",
        Ok(patch(&numbered(40, |i| format!("{i}")), &[2, 20])),
    );
    harness.press(KeyCode::Enter);
    let loaded = harness.run_next();
    harness.send(loaded);
    assert!(enabled(&harness, '↓'));
    assert!(!diff_text(&harness).contains("Loading…"));

    harness.press(KeyCode::Enter);
    assert!(!enabled(&harness, '↑'));
    assert!(!enabled(&harness, '↓'));
    assert!(diff_text(&harness).contains("Loading…"));
    assert_eq!(harness.git.calls().last().unwrap(), "diff a.txt");
}

#[test]
fn unsupported_entry_shows_its_reason_without_loading() {
    let mut repo = state(&[], &[]);
    repo.unstaged.push(FileEntry::unsupported(
        "sub",
        Side::Unstaged,
        'M',
        "Submodules are not supported",
    ));
    let mut harness = Harness::new(Ok(repo));
    harness.press(KeyCode::Enter);
    assert!(diff_text(&harness).contains("Submodules are not supported"));
    assert!(!diff_text(&harness).contains("Loading…"));
    assert!(
        !harness
            .git
            .calls()
            .iter()
            .any(|call| call.starts_with("diff"))
    );
}

#[test]
fn diff_view_supports_line_page_jump_and_long_horizontal_navigation() {
    let lines = numbered(100, |i| format!("{i:03} {}", "x".repeat(200)));
    let mut harness = open(80, 12, patch(&lines, &[]));
    // Unstaged -> Commits -> diff viewer.
    harness.press(KeyCode::Tab);
    harness.press(KeyCode::Tab);
    assert_eq!(harness.app.focus, Focus::Diff);

    harness.press(KeyCode::Down);
    assert_eq!(scroll(&harness).1, 1);
    harness.press(KeyCode::Char('j'));
    assert_eq!(scroll(&harness).1, 2);
    harness.press(KeyCode::Char('k'));
    assert_eq!(scroll(&harness).1, 1);

    let page = harness.app.diff_view.viewport().height as usize;
    harness.press(KeyCode::PageDown);
    assert_eq!(scroll(&harness).1, 1 + page);

    let viewport = harness.app.diff_view.viewport();
    harness.click((viewport.right(), viewport.bottom() - 1));
    assert_eq!(scroll(&harness).1, harness.app.diff_view.max_scroll().1);

    harness.press(KeyCode::Right);
    assert_eq!(scroll(&harness).0, 1);

    let max_x = harness.app.diff_view.max_scroll().0;
    let y = scroll(&harness).1;
    harness.app.diff_view.scroll_to(max_x - 1, y);
    for _ in 0..2 {
        harness.press(KeyCode::Right);
        assert_eq!(scroll(&harness).0, max_x);
    }
}

#[test]
fn mouse_wheel_scrolls_the_diff() {
    let lines = numbered(100, |i| format!("{i:03} {}", "x".repeat(200)));
    let mut harness = open(80, 12, patch(&lines, &[]));
    harness.scroll(MouseEventKind::ScrollDown, (50, 5));
    assert_eq!(scroll(&harness), (0, 3));
    harness.scroll(MouseEventKind::ScrollRight, (50, 5));
    assert_eq!(scroll(&harness), (3, 3));
    // Over the sidebar, the wheel leaves the diff alone.
    harness.scroll(MouseEventKind::ScrollDown, (5, 5));
    assert_eq!(scroll(&harness), (3, 3));
}

#[test]
fn app_shortcuts_win_over_a_focused_viewer() {
    let lines = numbered(100, |i| format!("line {i}"));
    let mut harness = open(80, 12, patch(&lines, &[10, 30]));
    harness.press(KeyCode::Tab);
    harness.press(KeyCode::Tab);
    assert_eq!(harness.app.focus, Focus::Diff);

    harness.press_with(KeyCode::Char('j'), KeyModifiers::CONTROL);
    assert_eq!(scroll(&harness).1, 6);
    harness.press(KeyCode::Char('n'));
    assert_eq!(scroll(&harness).1, 26);
    harness.press(KeyCode::Char('q'));
    assert!(harness.quit);
}

#[test]
fn diff_request_and_refresh_clear_viewer() {
    let mut harness = Harness::held(Ok(state(&[], &["a.txt"])));
    let status = harness.run_next();
    harness.send(status);
    let lines = numbered(80, |i| format!("{i:03} {}", "x".repeat(120)));
    harness.git.set_diff("a.txt", Ok(patch(&lines, &[17])));

    harness.press(KeyCode::Enter);
    assert!(diff_text(&harness).contains("Loading…"));
    let loaded = harness.run_next();
    harness.send(loaded);
    assert_eq!(scroll(&harness), (0, 13));
    assert!(!diff_text(&harness).contains("Loading…"));
    assert!(harness.line(2).contains("a.txt"));

    harness.press(KeyCode::Char('w'));
    assert!(harness.app.diff_view.wrapped());
    harness.app.diff_view.scroll_to(0, 20);
    harness
        .git
        .set_diff("a.txt", Ok(patch(&["replacement".into()], &[0])));
    harness.press(KeyCode::Enter);
    assert!(diff_text(&harness).contains("Loading…"));

    harness.press(KeyCode::Char('r'));
    let pending = harness.run_next();
    let refreshed = harness.run_next();
    // A refresh drops the pending load without clearing the diff.
    harness.send(pending);
    assert!(harness.line(2).contains("a.txt"));
    assert_eq!(harness.app.diff_view.document().rows.len(), 80);

    harness.send(refreshed);
    assert!(!harness.line(2).contains("a.txt"));
    assert!(harness.app.diff_view.document().rows.is_empty());
    assert_eq!(scroll(&harness), (0, 0));
    assert!(!diff_text(&harness).contains("Loading…"));
    assert!(!enabled(&harness, '↑') && !enabled(&harness, '↓'));

    // Wrapping persists across the refresh.
    harness.press(KeyCode::Char('w'));
    assert!(!harness.app.diff_view.wrapped());
}

#[test]
fn empty_and_context_only_diffs_stay_at_origin() {
    let wide = numbered(80, |_| "x".repeat(120));
    for replacement in [
        String::new(),
        patch(&numbered(80, |i| format!("context {i}")), &[]),
    ] {
        let mut harness = open(80, 12, patch(&wide, &[]));
        harness.app.diff_view.scroll_to(20, 20);
        harness.draw();
        assert_eq!(scroll(&harness), (20, 20));

        load(&mut harness, replacement);
        assert_eq!(scroll(&harness), (0, 0));
    }
}

#[test]
fn replacing_a_scrolled_diff_resets_offsets_and_scrolls_to_first_change() {
    let old = numbered(80, |_| format!("old {}", "x".repeat(120)));
    let mut harness = open(80, 12, patch(&old, &[0]));
    harness.app.diff_view.scroll_to(20, 30);
    harness.draw();

    load(
        &mut harness,
        patch(&numbered(80, |i| format!("new {i}")), &[9]),
    );
    assert!(diff_text(&harness).contains("new 9"));
    assert!(!diff_text(&harness).contains("old"));
    assert_eq!(scroll(&harness), (0, 5));
}

#[test]
fn diff_wrap_transitions_preserve_progress_and_reset_horizontal_scroll() {
    let lines = numbered(100, |i| format!("{i:03} {}", "x".repeat(120)));
    let mut harness = open(80, 12, patch(&lines, &[]));
    harness.app.diff_view.scroll_to(20, 30);
    harness.draw();
    let progress = |harness: &Harness| {
        let (_, max_y) = harness.app.diff_view.max_scroll();
        (scroll(harness).1 as f64 / max_y as f64, 1.0 / max_y as f64)
    };
    let (before, _) = progress(&harness);

    harness.press(KeyCode::Char('w'));
    let (after, tolerance) = progress(&harness);
    assert_eq!(scroll(&harness).0, 0);
    assert!((after - before).abs() <= tolerance, "{before} {after}");

    let (_, max_y) = harness.app.diff_view.max_scroll();
    harness.app.diff_view.scroll_to(0, max_y / 2);
    harness.draw();
    let (before, _) = progress(&harness);
    harness.press(KeyCode::Char('w'));
    let (after, tolerance) = progress(&harness);
    assert_eq!(scroll(&harness).0, 0);
    assert!((after - before).abs() <= tolerance, "{before} {after}");
}

#[test]
fn loading_a_new_diff_while_wrapped_updates_the_viewer() {
    let mut harness = Harness::with(Ok(state(&[], &["a.txt"])), false, 80, 12);
    harness.press(KeyCode::Char('w'));
    load(
        &mut harness,
        patch(&numbered(40, |i| format!("new {i}")), &[17]),
    );
    assert!(harness.app.diff_view.wrapped());
    assert!(diff_text(&harness).contains("new 17"));
    assert_eq!(
        scroll(&harness).1,
        harness.app.diff_view.source_to_visual_row(13)
    );
    assert!(!diff_text(&harness).contains("Loading…"));
}

/// A 100-line one-hunk patch changing line `changed`, marked with `marker`.
fn marked_patch(marker: &str, changed: usize) -> String {
    let mut lines = numbered(100, |i| format!("{marker} context {i}"));
    lines[changed] = format!("{marker} new {changed}");
    patch(&lines, &[changed])
}

#[test]
fn older_diff_completing_late_leaves_the_newer_diff_visible() {
    let mut harness = Harness::held(Ok(state(&[], &["old.py", "new.py"])));
    let status = harness.run_next();
    harness.send(status);
    harness.git.set_diff("old.py", Ok(marked_patch("older", 5)));
    harness
        .git
        .set_diff("new.py", Ok(marked_patch("newer", 60)));

    harness.press(KeyCode::Enter);
    harness.press(KeyCode::Down);
    harness.press(KeyCode::Enter);
    let older = harness.run_next();
    let newer = harness.run_next();

    harness.send(newer);
    harness.press(KeyCode::Char('w'));
    harness.app.diff_view.scroll_to(0, 10);
    harness.draw();
    let visible = diff_text(&harness);
    let offset = scroll(&harness);
    assert!(visible.contains("newer"));
    assert!(offset.1 > 0);

    harness.send(older);
    assert!(harness.line(2).contains("new.py"));
    assert!(harness.app.diff_view.wrapped());
    assert_eq!(scroll(&harness), offset);
    assert_eq!(diff_text(&harness), visible);
    assert!(!diff_text(&harness).contains("older"));
}

#[test]
fn diff_failure_clears_loading_and_reports_error() {
    let mut harness = Harness::new(Ok(state(&[], &["a.txt"])));
    harness
        .git
        .set_diff("a.txt", Err(failure("fatal: diff failed\n")));
    harness.press(KeyCode::Enter);
    let screen = harness.screen();
    assert!(screen.contains("Could not load diff"), "{screen}");
    assert!(screen.contains("fatal: diff failed"));
    assert!(!diff_text(&harness).contains("Loading…"));
}

#[test]
fn status_failure_clears_diff_loading_and_reports_error() {
    let mut harness = Harness::held(Ok(state(&[], &["a.txt"])));
    let status = harness.run_next();
    harness.send(status);
    harness.press(KeyCode::Enter);
    assert!(diff_text(&harness).contains("Loading…"));

    *harness.git.status.lock().unwrap() = Err(failure("fatal: bad index"));
    harness.press(KeyCode::Char('r'));
    let _pending_diff = harness.run_next();
    let failed = harness.run_next();
    harness.send(failed);
    assert!(harness.screen().contains("Could not refresh status"));
    assert!(!harness.screen().contains("Loading…"));
}

#[test]
fn stale_diff_failure_leaves_the_newer_diff_without_a_toast() {
    let mut harness = Harness::held(Ok(state(&[], &["old.py", "new.py"])));
    let status = harness.run_next();
    harness.send(status);
    harness
        .git
        .set_diff("old.py", Err(failure("fatal: old diff failed")));
    harness
        .git
        .set_diff("new.py", Ok(marked_patch("newer", 60)));

    harness.press(KeyCode::Enter);
    harness.press(KeyCode::Down);
    harness.press(KeyCode::Enter);
    let older = harness.run_next();
    let newer = harness.run_next();
    harness.send(newer);
    let visible = diff_text(&harness);
    assert!(visible.contains("newer"));

    harness.send(older);
    let screen = harness.screen();
    assert!(!screen.contains("Could not load diff"), "{screen}");
    assert!(!screen.contains("old diff failed"));
    assert!(harness.app.toasts.is_empty());
    assert!(harness.line(2).contains("new.py"));
    assert_eq!(diff_text(&harness), visible);
}

/// Deliver a watcher edit of `a.txt`, which quietly reloads its open diff.
fn edit_a(harness: &mut Harness) {
    let invalidation = Invalidation {
        status: true,
        changed_paths: ["a.txt".to_string()].into(),
        ..Invalidation::default()
    };
    harness.send(Event::Watch(Watch::Changed(invalidation)));
}

#[test]
fn quiet_reload_keeps_scroll_clamped_wrap_and_change_index() {
    let lines = |count| numbered(count, |i| format!("line {i}"));
    let mut harness = Harness::with(Ok(state(&[], &["a.txt"])), false, 80, 12);
    harness.press(KeyCode::Char('w'));
    load(&mut harness, patch(&lines(100), &[10, 30, 70]));
    harness.press(KeyCode::Char('n'));
    harness.app.diff_view.scroll_to(0, 40);
    harness.draw();

    harness
        .git
        .set_diff("a.txt", Ok(patch(&lines(120), &[10, 30, 70])));
    edit_a(&mut harness);
    assert_eq!(scroll(&harness), (0, 40));
    assert!(
        enabled(&harness, '↑') && enabled(&harness, '↓'),
        "still on change 2"
    );
    assert!(harness.app.diff_view.wrapped());
    assert!(!diff_text(&harness).contains("Loading…"));

    harness.git.set_diff("a.txt", Ok(patch(&lines(30), &[10])));
    edit_a(&mut harness);
    let (_, max_y) = harness.app.diff_view.max_scroll();
    assert_eq!(scroll(&harness).1, max_y);
    assert!(max_y < 40);
    assert!(
        !enabled(&harness, '↑') && !enabled(&harness, '↓'),
        "reset to change 1"
    );
    assert!(diff_text(&harness).contains("line 29"));
}

#[test]
fn quiet_reload_of_an_equal_diff_leaves_the_document_untouched() {
    let text = patch(&numbered(100, |i| format!("line {i}")), &[10]);
    let mut harness = open(80, 12, text.clone());
    harness.app.diff_view.scroll_to(0, 20);
    harness.draw();
    let start = PANE_X + GUTTER;
    harness.drag((start, 4), (start + 4, 4));
    let before = harness.buffer();

    edit_a(&mut harness);
    assert_eq!(scroll(&harness), (0, 20));
    assert_eq!(harness.buffer(), before, "the text selection survives");
}

#[test]
fn quiet_reload_failure_keeps_the_document_and_scroll() {
    let mut harness = open(
        80,
        12,
        patch(&numbered(100, |i| format!("line {i}")), &[10]),
    );
    harness.app.diff_view.scroll_to(0, 20);
    harness.draw();
    // The rows above the warning toast.
    let top = |harness: &Harness| {
        diff_text(harness)
            .lines()
            .take(5)
            .collect::<Vec<_>>()
            .join("\n")
    };
    let before = top(&harness);
    harness
        .git
        .set_diff("a.txt", Err(failure("fatal: diff failed")));

    edit_a(&mut harness);
    assert_eq!(scroll(&harness), (0, 20));
    assert_eq!(top(&harness), before);
    assert!(before.contains("line 20"));
    let severities: Vec<Severity> = harness.app.toasts.iter().map(|t| t.severity).collect();
    assert_eq!(severities, [Severity::Warning]);
}
