mod common;

use common::{Harness, TempDir};
use crossterm::event::{KeyCode, KeyModifiers};
use gitplume::app::{Effect, Event, Focus};
use std::path::PathBuf;

/// Capture external editor requests rather than launching an interactive process.
fn edit(harness: &mut Harness, code: char, modifiers: KeyModifiers) -> Vec<PathBuf> {
    use crossterm::event::{Event as Input, KeyEvent};
    let effects = harness.app.update(Event::Input(Input::Key(KeyEvent::new(
        KeyCode::Char(code),
        modifiers,
    ))));
    harness.draw();
    effects
        .into_iter()
        .map(|effect| match effect {
            Effect::Edit(path) => path,
            other => panic!("unexpected effect: {other:?}"),
        })
        .collect()
}

fn request(harness: &mut Harness) -> Vec<PathBuf> {
    edit(harness, 'e', KeyModifiers::NONE)
}

#[test]
fn lowercase_e_edits_the_open_file_only_on_the_files_tab() {
    let dir = TempDir::new("editor-shortcut");
    let cwd = dir.0.join("subdir");
    std::fs::create_dir(&cwd).unwrap();
    std::fs::write(cwd.join("a file.txt"), "original content").unwrap();
    std::fs::write(cwd.join("other.txt"), "other content").unwrap();
    let mut harness = Harness::launched(&dir.0, &cwd, &["a file.txt", "other.txt"]);

    assert!(request(&mut harness).is_empty());
    harness.press(KeyCode::Char('2'));
    assert!(request(&mut harness).is_empty());

    harness.click(harness.at("a file.txt"));
    assert!(harness.screen().contains("original content"));
    assert!(edit(&mut harness, 'E', KeyModifiers::SHIFT).is_empty());
    assert!(edit(&mut harness, 'e', KeyModifiers::CONTROL).is_empty());

    // Moving the tree cursor must not change the file sent to the editor.
    harness.press(KeyCode::Down);
    assert_eq!(request(&mut harness), [cwd.join("a file.txt")]);
    harness.press(KeyCode::Tab);
    assert_eq!(harness.app.focus, Focus::Preview);
    assert_eq!(request(&mut harness), [cwd.join("a file.txt")]);

    harness.press(KeyCode::Char('h'));
    assert!(request(&mut harness).is_empty());
    harness.press(KeyCode::Esc);
    harness.press(KeyCode::Char('1'));
    assert!(request(&mut harness).is_empty());
    harness.press(KeyCode::Char('3'));
    harness.press(KeyCode::Tab);
    harness.press(KeyCode::Tab);
    assert!(request(&mut harness).is_empty());
}

#[test]
fn returning_from_editor_reloads_preview_and_keeps_files_mode() {
    let dir = TempDir::new("editor-return");
    let file = dir.0.join("file.txt");
    std::fs::write(&file, "before editing").unwrap();
    let mut harness = Harness::launched(&dir.0, &dir.0, &["file.txt"]);
    harness.press(KeyCode::Char('2'));
    harness.click(harness.at("file.txt"));
    harness.press(KeyCode::Tab);
    assert_eq!(request(&mut harness), std::slice::from_ref(&file));
    std::fs::write(&file, "after editing").unwrap();
    harness.send(Event::EditorClosed(Ok(())));
    assert!(harness.screen().contains("after editing"));
    assert!(!harness.screen().contains("before editing"));
    assert_eq!(harness.app.focus, Focus::Preview);
    assert_eq!(request(&mut harness), [file]);

    harness.send(Event::EditorClosed(Err(
        "editor executable not found".into()
    )));
    assert!(harness.screen().contains("Could not edit file"));
    assert!(harness.screen().contains("editor executable not found"));
    assert!(harness.screen().contains("after editing"));

    // Status refresh is deferred until returning to Changes, even without a watcher event.
    let mut status = common::state(&[], &["file.txt"]);
    status.root = dir.0.clone();
    *harness.git.status.lock().unwrap() = Ok(status);
    harness.press(KeyCode::Char('1'));
    assert!(harness.screen().contains("file.txt"));
}

#[test]
fn editor_shortcut_is_disabled_during_loading_and_after_refresh() {
    let dir = TempDir::new("editor-loading");
    std::fs::write(dir.0.join("file.txt"), "content").unwrap();
    let mut harness = Harness::launched(&dir.0, &dir.0, &["file.txt"]);
    harness.press(KeyCode::Char('2'));
    harness.hold_files = true;
    harness.click(harness.at("file.txt"));
    assert!(request(&mut harness).is_empty());
    let event = harness.run_files();
    harness.send(event);
    assert_eq!(request(&mut harness), [dir.0.join("file.txt")]);
    harness.hold_files = false;
    harness.press(KeyCode::Char('r'));
    assert!(request(&mut harness).is_empty());
}
