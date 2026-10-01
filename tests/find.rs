mod common;

use std::path::Path;

use common::{Harness, TempDir, failure};
use crossterm::event::{KeyCode, KeyModifiers};
use gitplume::app::{Focus, Tab};
use gitplume::model::GrepMatch;

const ROOT: &str = "/repo";

fn found(path: &str, line: usize, text: &str) -> GrepMatch {
    GrepMatch {
        path: path.to_string(),
        line,
        text: text.to_string(),
    }
}

fn type_text(harness: &mut Harness, text: &str) {
    for c in text.chars() {
        harness.press(KeyCode::Char(c));
    }
}

fn open_find(harness: &mut Harness) {
    harness.press_with(KeyCode::Char('f'), KeyModifiers::CONTROL);
}

/// The sidebar is `SIDE_WIDTH` (30) columns wide at the default layout used
/// by every test here; the preview pane starts just past its divider.
const PREVIEW_LEFT_EDGE: u16 = 31;

/// Cell position of `text`'s first occurrence at or past column `min_x`.
fn find_from(harness: &Harness, text: &str, min_x: u16) -> Option<(u16, u16)> {
    let buffer = harness.buffer();
    let needle: Vec<String> = text.chars().map(String::from).collect();
    (0..buffer.area.height).find_map(|y| {
        let cells: Vec<&str> = (min_x..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect();
        cells
            .windows(needle.len())
            .position(|window| window.iter().zip(&needle).all(|(a, b)| *a == b))
            .map(|x| (min_x + x as u16, y))
    })
}

#[test]
fn ctrl_f_opens_the_find_tab_with_the_query_box_focused() {
    let mut harness = Harness::launched(Path::new(ROOT), Path::new(ROOT), &["a.txt"]);

    open_find(&mut harness);

    assert_eq!(harness.app.tab, Tab::Find);
    assert_eq!(harness.app.focus, Focus::FindQuery);
    assert!(harness.line(0).contains("Find"));
}

#[test]
fn shortcut_letters_type_into_the_query_box_instead_of_triggering_them() {
    let mut harness = Harness::launched(Path::new(ROOT), Path::new(ROOT), &["a.txt"]);
    open_find(&mut harness);
    let calls_before = harness.git.calls().len();

    type_text(&mut harness, "qr");

    assert!(!harness.quit, "q must not quit while typing");
    assert_eq!(harness.app.find.query, "qr");
    assert_eq!(
        harness.git.calls().len(),
        calls_before,
        "r must not trigger a refresh"
    );
}

#[test]
fn fewer_than_three_characters_runs_no_search() {
    let mut harness = Harness::launched(Path::new(ROOT), Path::new(ROOT), &["a.rs"]);
    harness
        .git
        .grep_matches
        .lock()
        .unwrap()
        .push(found("a.rs", 1, "catnap"));
    open_find(&mut harness);

    type_text(&mut harness, "ca");

    assert!(harness.find("catnap").is_none());
    assert!(
        !harness.git.calls().iter().any(|c| c.starts_with("grep ")),
        "two characters must not call grep"
    );
    assert!(harness.find("Type at least 3 characters").is_some());
}

#[test]
fn three_characters_search_and_list_each_match_as_its_own_row() {
    let mut harness = Harness::launched(
        Path::new(ROOT),
        Path::new(ROOT),
        &["src/cat.rs", "src/dog.rs"],
    );
    harness.git.grep_matches.lock().unwrap().extend([
        found("src/cat.rs", 3, "cat says meow"),
        found("src/cat.rs", 10, "cat purrs"),
    ]);
    open_find(&mut harness);

    type_text(&mut harness, "cat");

    assert!(harness.find("src/cat.rs:3").is_some());
    assert!(harness.find("meow").is_some());
    assert!(harness.find("src/cat.rs:10").is_some());
    assert!(harness.find("purrs").is_some());
}

#[test]
fn filter_limits_the_paths_sent_to_grep() {
    let mut harness = Harness::launched(
        Path::new(ROOT),
        Path::new(ROOT),
        &["src/app.rs", "README.md"],
    );
    open_find(&mut harness);
    type_text(&mut harness, "foo");
    harness.press(KeyCode::Tab); // query -> filter

    type_text(&mut harness, "*.rs");

    let calls = harness.git.calls();
    let grep_call = calls
        .iter()
        .rev()
        .find(|c| c.starts_with("grep "))
        .expect("a grep call was made");
    assert!(grep_call.contains("src/app.rs"));
    assert!(!grep_call.contains("README.md"));
}

/// `count` zero-padded, fixed-width labels (`row-0001`, ...) that never
/// collide as substrings of one another, so screen assertions are exact.
fn numbered_rows(count: usize) -> Vec<String> {
    (1..=count).map(|i| format!("row-{i:04}")).collect()
}

#[test]
fn clicking_a_match_opens_the_file_scrolled_to_its_line() {
    let dir = TempDir::new("find-click");
    let rows = numbered_rows(200);
    std::fs::write(dir.0.join("notes.txt"), rows.join("\n") + "\n").unwrap();
    let mut harness = Harness::launched(&dir.0, &dir.0, &["notes.txt"]);
    harness.git.grep_matches.lock().unwrap().push(found(
        "notes.txt",
        150,
        &format!("target {}", rows[149]),
    ));
    open_find(&mut harness);

    type_text(&mut harness, "target");
    // The sidebar is too narrow to show the whole row, so click on the
    // `path:line` prefix, which always fits and is unique on screen.
    let result_position = harness.at("notes.txt:150");
    harness.click(result_position);

    assert!(harness.line(2).contains("notes.txt"), "{}", harness.line(2));
    let marked = find_from(&harness, &rows[149], PREVIEW_LEFT_EDGE)
        .expect("the target line must be visible in the preview");
    assert!(
        find_from(&harness, &rows[0], PREVIEW_LEFT_EDGE).is_none(),
        "the file's first line must have scrolled out of view: {}",
        harness.screen()
    );

    // The matched row's background is distinct from its neighbor's: the
    // preview has no other reason for adjacent rows to differ.
    let buffer = harness.buffer();
    let marked_bg = buffer[marked].bg;
    let neighbor_bg = buffer[(marked.0, marked.1 + 1)].bg;
    assert_ne!(
        marked_bg, neighbor_bg,
        "the matched line should be highlighted"
    );
}

#[test]
fn down_then_enter_opens_the_highlighted_match() {
    let dir = TempDir::new("find-enter");
    std::fs::write(dir.0.join("notes.txt"), "alpha\nbeta needle\ngamma\n").unwrap();
    let mut harness = Harness::launched(&dir.0, &dir.0, &["notes.txt"]);
    harness
        .git
        .grep_matches
        .lock()
        .unwrap()
        .push(found("notes.txt", 2, "beta needle"));
    open_find(&mut harness);

    type_text(&mut harness, "needle");
    assert!(
        !harness.line(2).contains("notes.txt"),
        "nothing is open before Enter: {}",
        harness.line(2)
    );

    harness.press(KeyCode::Down);
    assert_eq!(harness.app.focus, Focus::FindResults);
    harness.press(KeyCode::Enter);

    assert!(
        harness.line(2).contains("notes.txt"),
        "Enter on the highlighted result must open it: {}",
        harness.line(2)
    );
}

#[test]
fn a_stale_held_search_result_is_dropped_after_further_typing() {
    let mut harness = Harness::launched(Path::new(ROOT), Path::new(ROOT), &["a.rs"]);
    harness
        .git
        .grep_matches
        .lock()
        .unwrap()
        .push(found("a.rs", 1, "only result"));
    open_find(&mut harness);
    harness.hold_files = true;

    type_text(&mut harness, "onl"); // first search job queued, query "onl"
    type_text(&mut harness, "y"); // "only": second, newer search job queued

    let stale = harness.run_files();
    harness.send(stale);
    assert!(
        harness.find("only result").is_none(),
        "the stale result must not be applied"
    );
    assert!(
        harness.find("Searching").is_some(),
        "still waiting for the newer search"
    );

    let fresh = harness.run_files();
    harness.send(fresh);
    assert!(harness.find("only result").is_some());
}

#[test]
fn a_grep_failure_shows_a_toast() {
    let mut harness = Harness::launched(Path::new(ROOT), Path::new(ROOT), &["a.rs"]);
    *harness.git.grep_error.lock().unwrap() = Some(failure("boom"));
    open_find(&mut harness);

    type_text(&mut harness, "abc");

    assert!(harness.find("Could not search files").is_some());
    assert!(harness.find("boom").is_some());
}
