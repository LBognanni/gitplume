mod common;

use std::path::Path;

use common::{Harness, TempDir};
use crossterm::event::KeyCode;
use gitplume::app::{Event, Focus, Tab};
use gitplume::watcher::{Invalidation, Watch};

const ROOT: &str = "/repo";

fn changed(paths: &[&str]) -> Invalidation {
    Invalidation {
        status: true,
        changed_paths: paths.iter().map(|p| p.to_string()).collect(),
        ..Invalidation::default()
    }
}

fn metadata_only() -> Invalidation {
    Invalidation {
        status: true,
        index_changed: true,
        ..Invalidation::default()
    }
}

fn watch(harness: &mut Harness, invalidation: Invalidation) {
    harness.send(Event::Watch(Watch::Changed(invalidation)));
}

fn count(harness: &Harness, prefix: &str) -> usize {
    harness
        .git
        .calls()
        .iter()
        .filter(|c| c.starts_with(prefix))
        .count()
}

#[test]
fn live_refresh_while_files_tab_is_active_shows_no_loading_and_keeps_tree_state() {
    let mut harness =
        Harness::launched(Path::new(ROOT), Path::new(ROOT), &["src/a.rs", "src/b.rs"]);
    harness.press(KeyCode::Char('2')); // Files tab
    harness.press(KeyCode::Down); // root -> "src"
    harness.press(KeyCode::Right); // expand "src"
    harness.press(KeyCode::Down); // a.rs
    harness.press(KeyCode::Down); // b.rs
    assert_eq!(harness.app.focus, Focus::FilesTree);
    let cursor_path_before = harness.app.files.nodes[harness.app.files.cursor.unwrap()]
        .path
        .clone();
    assert_eq!(cursor_path_before, "src/b.rs");
    let offset_before = harness.app.files.offset;

    harness.hold_files = true;
    let calls_before = count(&harness, "files");
    *harness.git.files.lock().unwrap() = Ok(vec![
        "src/a.rs".to_string(),
        "src/b.rs".to_string(),
        "src/c.rs".to_string(),
    ]);
    watch(&mut harness, changed(&["src/c.rs"]));

    assert!(
        !harness.app.files_tree_loading,
        "a background refresh must not show the loading state"
    );
    let files_event = harness.run_files();
    harness.send(files_event);
    harness.hold_files = false;

    assert_eq!(
        count(&harness, "files"),
        calls_before + 1,
        "the live refresh ran immediately, without switching tabs"
    );
    assert!(harness.screen().contains("c.rs"), "{}", harness.screen());
    // "src" is still expanded and the cursor is still on the same file.
    let cursor_path_after = harness.app.files.nodes[harness.app.files.cursor.unwrap()]
        .path
        .clone();
    assert_eq!(cursor_path_after, "src/b.rs");
    assert_eq!(harness.app.files.offset, offset_before);
}

#[test]
fn a_change_while_inactive_is_deferred_until_switching_to_files() {
    let mut harness = Harness::launched(Path::new(ROOT), Path::new(ROOT), &["a.rs"]);
    assert_eq!(harness.app.tab, Tab::Changes);
    let calls_before = count(&harness, "files");

    *harness.git.files.lock().unwrap() = Ok(vec!["a.rs".to_string(), "b.rs".to_string()]);
    watch(&mut harness, changed(&["b.rs"]));

    assert_eq!(
        count(&harness, "files"),
        calls_before,
        "nothing refreshes while Files/Find aren't active"
    );

    harness.press(KeyCode::Char('2')); // switch to Files

    assert_eq!(
        count(&harness, "files"),
        calls_before + 1,
        "switching to a dirty tab refreshes it exactly once"
    );
    assert!(harness.screen().contains("b.rs"), "{}", harness.screen());
}

#[test]
fn an_unrelated_change_does_not_disturb_the_open_preview() {
    let dir = TempDir::new("files-refresh-preview");
    std::fs::write(dir.0.join("a_shown.txt"), "shown content\n").unwrap();
    std::fs::write(dir.0.join("b_other.txt"), "other content\n").unwrap();
    let mut harness = Harness::launched(&dir.0, &dir.0, &["a_shown.txt", "b_other.txt"]);
    harness.press(KeyCode::Char('2'));
    harness.press(KeyCode::Down); // "a_shown.txt", the first file under root
    harness.press(KeyCode::Enter);
    assert!(
        harness.screen().contains("shown content"),
        "{}",
        harness.screen()
    );
    let title_before = harness.line(2);

    watch(&mut harness, changed(&["b_other.txt"]));

    assert_eq!(harness.line(2), title_before);
    assert!(
        harness.screen().contains("shown content"),
        "{}",
        harness.screen()
    );
}

#[test]
fn status_refresh_is_deferred_while_on_files_and_applied_on_switching_back() {
    let mut harness = Harness::launched(Path::new(ROOT), Path::new(ROOT), &["a.rs"]);
    harness.press(KeyCode::Char('2')); // Files tab
    let calls_before = count(&harness, "status");

    watch(&mut harness, metadata_only());
    assert_eq!(
        count(&harness, "status"),
        calls_before,
        "no automatic status read while Files is active"
    );

    harness.press(KeyCode::Char('1')); // back to Changes
    assert_eq!(
        count(&harness, "status"),
        calls_before + 1,
        "switching back to Changes catches up the pending read"
    );
}

#[test]
fn a_metadata_only_change_does_not_touch_files_while_on_the_files_tab() {
    let mut harness = Harness::launched(Path::new(ROOT), Path::new(ROOT), &["a.rs"]);
    harness.press(KeyCode::Char('2'));
    let calls_before = count(&harness, "files");

    watch(&mut harness, metadata_only());

    assert_eq!(
        count(&harness, "files"),
        calls_before,
        "an index-only change must not touch Files"
    );
}
