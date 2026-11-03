mod common;

use std::path::Path;

use common::{Harness, TempDir, failure};
use crossterm::event::{KeyCode, KeyModifiers, MouseEventKind};
use gitplume::app::{Event, Focus, Tab};
use gitplume::model::GrepMatch;
use gitplume::watcher::{Invalidation, Watch};

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

fn changed(paths: &[&str]) -> Invalidation {
    Invalidation {
        status: true,
        changed_paths: paths.iter().map(|p| p.to_string()).collect(),
        ..Invalidation::default()
    }
}

fn watch(harness: &mut Harness, invalidation: Invalidation) {
    harness.send(Event::Watch(Watch::Changed(invalidation)));
}

#[test]
fn background_search_keeps_selection_and_scroll_without_hiding_results() {
    let mut harness = Harness::launched(Path::new(ROOT), Path::new(ROOT), &["0.rs", "a.rs"]);
    harness
        .git
        .grep_matches
        .lock()
        .unwrap()
        .extend((1..=100).map(|line| found("a.rs", line, &format!("needle {line}"))));
    open_find(&mut harness);
    type_text(&mut harness, "needle");
    harness.click(harness.at("a.rs:2 "));
    let selected = harness.at("a.rs:2 ");
    let selected_bg = harness.buffer()[selected].bg;
    assert_ne!(selected_bg, harness.buffer()[harness.at("a.rs:1 ")].bg);

    // Hold the refresh so the in-flight state is also observable.
    harness.hold_files = true;
    watch(&mut harness, changed(&["other.rs"]));
    let files = harness.run_files();
    harness.send(files);
    assert!(harness.find("Searching").is_none());
    assert_eq!(harness.buffer()[selected].bg, selected_bg);
    let grep = harness.run_files();
    harness.send(grep);
    assert_eq!(harness.buffer()[selected].bg, selected_bg);

    // Scrolling away from the selection must survive even if its index changes.
    harness.hold_files = false;
    for _ in 0..4 {
        harness.scroll(MouseEventKind::ScrollDown, selected);
    }
    let top = harness.line(selected.1 - 1);
    harness
        .git
        .grep_matches
        .lock()
        .unwrap()
        .insert(0, found("0.rs", 1, "new needle"));
    watch(&mut harness, changed(&["a.rs"]));
    assert!(harness.find("a.rs:2 ").is_none());
    assert_ne!(
        harness.line(selected.1 - 1),
        top,
        "the inserted row changes visible content, not scroll position"
    );
    assert!(harness.find("a.rs:12 ").is_some(), "{}", harness.screen());

    // Enter still opens the selected match, not its old numeric index.
    harness.press(KeyCode::Enter);
    assert!(harness.line(2).contains("a.rs"));
    harness.press(KeyCode::Down);
    let next = harness.at("a.rs:3 ");
    assert_eq!(harness.buffer()[next].bg, selected_bg);
}

#[test]
fn background_search_follows_a_surviving_match_and_clears_a_removed_selection() {
    let mut harness = Harness::launched(Path::new(ROOT), Path::new(ROOT), &["a.rs"]);
    *harness.git.grep_matches.lock().unwrap() = vec![
        found("a.rs", 10, "needle first"),
        found("a.rs", 20, "needle selected"),
    ];
    open_find(&mut harness);
    type_text(&mut harness, "needle");
    harness.click(harness.at("a.rs:20"));
    let bg = harness.buffer()[harness.at("a.rs:20")].bg;
    *harness.git.grep_matches.lock().unwrap() = vec![
        found("a.rs", 1, "needle inserted"),
        found("a.rs", 10, "needle first"),
        found("a.rs", 20, "needle edited"),
    ];
    watch(&mut harness, changed(&["a.rs"]));
    assert_eq!(harness.buffer()[harness.at("a.rs:20")].bg, bg);
    assert_ne!(harness.buffer()[harness.at("a.rs:10")].bg, bg);

    harness.git.grep_matches.lock().unwrap().pop();
    watch(&mut harness, changed(&["a.rs"]));
    assert!(harness.find("a.rs:20").is_none());
    for label in ["a.rs:1 ", "a.rs:10"] {
        assert_ne!(harness.buffer()[harness.at(label)].bg, bg);
    }
}

#[test]
fn editing_query_or_filter_resets_result_selection_and_scroll() {
    for edit_filter in [false, true] {
        let mut harness = Harness::launched(Path::new(ROOT), Path::new(ROOT), &["a.rs"]);
        harness
            .git
            .grep_matches
            .lock()
            .unwrap()
            .extend((1..=100).map(|line| found("a.rs", line, "needles")));
        open_find(&mut harness);
        type_text(&mut harness, "needle");
        harness.press(KeyCode::Down);
        for _ in 0..35 {
            harness.press(KeyCode::Down);
        }
        let bg = harness.buffer()[harness.at("a.rs:36 ")].bg;
        assert!(harness.find("a.rs:1 ").is_none());
        harness.press(KeyCode::BackTab); // results -> filter
        if !edit_filter {
            harness.press(KeyCode::BackTab); // filter -> query
        }
        type_text(&mut harness, if edit_filter { "*.rs" } else { "s" });
        assert!(harness.find("a.rs:1 ").is_some());
        harness.press(KeyCode::Tab);
        if !edit_filter {
            harness.press(KeyCode::Tab);
        }
        assert_eq!(harness.app.focus, Focus::FindResults);
        assert_ne!(harness.buffer()[harness.at("a.rs:1 ")].bg, bg);
        harness.press(KeyCode::Down);
        assert_eq!(harness.buffer()[harness.at("a.rs:1 ")].bg, bg);
    }
}

#[test]
fn a_filesystem_change_refreshes_a_stale_match_while_find_is_active() {
    let mut harness = Harness::launched(Path::new(ROOT), Path::new(ROOT), &["a.rs"]);
    harness
        .git
        .grep_matches
        .lock()
        .unwrap()
        .push(found("a.rs", 5, "needle here"));
    open_find(&mut harness);
    type_text(&mut harness, "needle");
    assert!(harness.find("a.rs:5").is_some());

    // Lines were added above the match, so it now sits on a later line.
    *harness.git.grep_matches.lock().unwrap() = vec![found("a.rs", 9, "needle here")];
    watch(&mut harness, changed(&["a.rs"]));

    assert!(harness.find("a.rs:9").is_some(), "{}", harness.screen());
    assert!(harness.find("a.rs:5").is_none(), "{}", harness.screen());
}

#[test]
fn a_deleted_match_disappears_after_a_filesystem_change() {
    let mut harness = Harness::launched(Path::new(ROOT), Path::new(ROOT), &["a.rs"]);
    harness
        .git
        .grep_matches
        .lock()
        .unwrap()
        .push(found("a.rs", 5, "needle here"));
    open_find(&mut harness);
    type_text(&mut harness, "needle");
    assert!(harness.find("a.rs:5").is_some());

    // The match (or the whole file) is gone; grep now reports nothing.
    harness.git.grep_matches.lock().unwrap().clear();
    watch(&mut harness, changed(&["a.rs"]));

    assert!(harness.find("a.rs:5").is_none(), "{}", harness.screen());
    assert!(harness.find("No matches").is_some());
}

#[test]
fn a_change_elsewhere_is_only_applied_after_switching_to_find() {
    let mut harness = Harness::launched(Path::new(ROOT), Path::new(ROOT), &["a.rs"]);
    harness
        .git
        .grep_matches
        .lock()
        .unwrap()
        .push(found("a.rs", 5, "needle here"));
    open_find(&mut harness);
    type_text(&mut harness, "needle");
    assert!(harness.find("a.rs:5").is_some());
    // Move focus off the query box, where `1`/`3` type instead of switching.
    harness.press(KeyCode::Down);
    assert_eq!(harness.app.focus, Focus::FindResults);

    harness.press(KeyCode::Char('1')); // Changes tab
    let grep_calls = |h: &Harness| {
        h.git
            .calls()
            .iter()
            .filter(|c| c.starts_with("grep"))
            .count()
    };
    let calls_before = grep_calls(&harness);

    *harness.git.grep_matches.lock().unwrap() = vec![found("a.rs", 9, "needle here")];
    watch(&mut harness, changed(&["a.rs"]));

    assert_eq!(
        grep_calls(&harness),
        calls_before,
        "no search runs while Find is not active"
    );

    harness.press(KeyCode::Char('3')); // back to Find
    assert!(harness.find("a.rs:9").is_some(), "{}", harness.screen());
}
