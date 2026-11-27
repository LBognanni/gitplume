mod common;

use std::path::{Path, PathBuf};

use common::Harness;
use crossterm::event::KeyCode;
use gitplume::app::Event;
use gitplume::watcher::{Invalidation, Watch};

const ROOT: &str = "/repo";

/// The Files tab of a repository with `src/a.rs` tracked, `src/debug.log`
/// ignored, and the wholly ignored `node_modules/` folder.
fn files_tab() -> Harness {
    let mut harness = Harness::launched(Path::new(ROOT), Path::new(ROOT), &["src/a.rs"]);
    *harness.git.ignored.lock().unwrap() =
        vec!["node_modules/".to_string(), "src/debug.log".to_string()];
    serve_dir(&harness, "node_modules", &["left-pad/", "index.js"]);
    serve_dir(&harness, "node_modules/left-pad", &["package.json"]);
    harness.press(KeyCode::Char('2'));
    harness.press(KeyCode::Char('r'));
    harness
}

fn serve_dir(harness: &Harness, dir: &str, entries: &[&str]) {
    harness.git.dirs.lock().unwrap().insert(
        PathBuf::from(ROOT).join(dir),
        entries.iter().map(|e| e.to_string()).collect(),
    );
}

fn listings(harness: &Harness) -> Vec<String> {
    let calls = harness.git.calls();
    calls
        .into_iter()
        .filter(|call| call.starts_with("list_dir"))
        .collect()
}

/// Move the cursor down to the tree row showing `name`.
fn select(harness: &mut Harness, name: &str) {
    for _ in 0..20 {
        let node = &harness.app.files.nodes[harness.app.files.cursor.unwrap()];
        if node.name == name {
            return;
        }
        harness.press(KeyCode::Down);
    }
    panic!("{name:?} not reachable:\n{}", harness.screen());
}

fn fg(harness: &Harness, text: &str) -> ratatui::style::Color {
    let buffer = harness.buffer();
    buffer[harness.at(text)].fg
}

#[test]
fn ignored_files_and_folders_are_shown_grayed_out() {
    let mut harness = files_tab();
    select(&mut harness, "src");
    harness.press(KeyCode::Right);
    harness.press(KeyCode::Up); // keep the cursor highlight off the compared rows

    assert_ne!(fg(&harness, "debug.log"), fg(&harness, "a.rs"));
    assert_eq!(fg(&harness, "node_modules"), fg(&harness, "debug.log"));
    assert_ne!(fg(&harness, "node_modules"), fg(&harness, "src"));
}

#[test]
fn ignored_folders_list_one_level_only_when_expanded() {
    let mut harness = files_tab();
    assert!(listings(&harness).is_empty());

    select(&mut harness, "node_modules");
    harness.press(KeyCode::Right);

    assert_eq!(listings(&harness), ["list_dir /repo/node_modules"]);
    let screen = harness.screen();
    assert!(screen.contains("left-pad"), "{screen}");
    assert!(screen.contains("index.js"), "{screen}");
    assert!(!screen.contains("package.json"), "{screen}");
    assert_ne!(fg(&harness, "index.js"), fg(&harness, "src"));

    select(&mut harness, "left-pad");
    harness.press(KeyCode::Right);

    assert_eq!(
        listings(&harness),
        [
            "list_dir /repo/node_modules",
            "list_dir /repo/node_modules/left-pad"
        ]
    );
    assert!(harness.screen().contains("package.json"));
}

#[test]
fn reexpanding_an_ignored_folder_shows_its_current_entries() {
    let mut harness = files_tab();
    select(&mut harness, "node_modules");
    harness.press(KeyCode::Right);
    harness.press(KeyCode::Left);
    assert!(!harness.screen().contains("index.js"));

    serve_dir(&harness, "node_modules", &["index.js", "new.js"]);
    harness.press(KeyCode::Enter);

    assert_eq!(listings(&harness).len(), 2);
    let screen = harness.screen();
    assert!(screen.contains("new.js"), "{screen}");
    assert!(!screen.contains("left-pad"), "{screen}");
}

#[test]
fn background_refresh_keeps_expanded_ignored_folders_and_cursor() {
    let mut harness = files_tab();
    select(&mut harness, "node_modules");
    harness.press(KeyCode::Right);
    select(&mut harness, "index.js");

    harness.send(Event::Watch(Watch::Changed(Invalidation {
        status: true,
        changed_paths: ["src/b.rs".to_string()].into(),
        ..Invalidation::default()
    })));

    assert!(harness.screen().contains("index.js"));
    let cursor = harness.app.files.cursor.unwrap();
    assert_eq!(
        harness.app.files.nodes[cursor].path,
        "node_modules/index.js"
    );
    assert_eq!(listings(&harness).len(), 1);
}

#[test]
fn a_failed_listing_reports_the_error_and_collapses_the_folder() {
    let mut harness = files_tab();
    harness.git.dirs.lock().unwrap().clear();
    select(&mut harness, "node_modules");
    harness.press(KeyCode::Right);

    assert!(harness.screen().contains("No such file or directory"));
    let cursor = harness.app.files.cursor.unwrap();
    assert!(!harness.app.files.nodes[cursor].expanded);
}

#[test]
fn file_jump_leaves_out_ignored_files() {
    let mut harness = files_tab();
    harness.press(KeyCode::Char('t'));
    for c in "debug".chars() {
        harness.press(KeyCode::Char(c));
    }

    assert!(harness.app.jump_matches().0.is_empty());
}
