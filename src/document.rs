//! Diff and preview documents: gutter text plus source lines with highlight lookups.

use std::io::{ErrorKind, Read};
use std::path::Path;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::diff::{self, Kind};
use crate::highlight::{Source, detect};
use crate::theme;

pub const MAX_PREVIEW_BYTES: u64 = 1024 * 1024;

pub struct DocRow {
    pub gutter: String,
    pub text: String,
    gutter_style: Style,
    background: Option<Color>,
    /// The (side, line) this row's colors come from.
    source: Option<(usize, usize)>,
}

impl DocRow {
    /// Background spanning the full row width, if any.
    pub fn background(&self) -> Option<Color> {
        self.background
    }
}

pub struct Document {
    pub rows: Vec<DocRow>,
    pub changes: Vec<usize>,
    pub wrap_indent: usize,
    sides: Vec<Source>,
}

impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Document({} rows)", self.rows.len())
    }
}

impl Document {
    /// A document with no rows.
    pub fn empty() -> Self {
        Self {
            rows: Vec::new(),
            changes: Vec::new(),
            wrap_indent: 0,
            sides: Vec::new(),
        }
    }

    /// A document holding one plain message row.
    pub fn message(text: &str) -> Self {
        Self {
            rows: vec![DocRow {
                gutter: String::new(),
                text: text.to_string(),
                gutter_style: Style::new(),
                background: None,
                source: None,
            }],
            changes: Vec::new(),
            wrap_indent: 0,
            sides: Vec::new(),
        }
    }

    pub fn first_change(&self) -> Option<usize> {
        self.changes.first().copied()
    }

    /// Render one row, highlighting only that row's source line.
    pub fn line(&self, index: usize) -> Line<'static> {
        let row = &self.rows[index];
        let base = row
            .background
            .map_or(Style::new(), |bg| Style::new().bg(bg));
        let mut spans = Vec::new();
        if !row.gutter.is_empty() {
            spans.push(Span::styled(
                row.gutter.clone(),
                base.patch(row.gutter_style),
            ));
        }
        let runs = row
            .source
            .map(|(side, line)| self.sides[side].highlight_line(line))
            .unwrap_or_default();
        let mut offset = 0;
        for (range, color) in runs {
            if range.start > offset {
                spans.push(Span::styled(
                    row.text[offset..range.start].to_string(),
                    base,
                ));
            }
            spans.push(Span::styled(
                row.text[range.clone()].to_string(),
                base.fg(color),
            ));
            offset = range.end;
        }
        if offset < row.text.len() {
            spans.push(Span::styled(row.text[offset..].to_string(), base));
        }
        Line::from(spans)
    }
}

/// Build the document for a file's patch text.
pub fn diff_document(path: &str, patch: &str) -> Document {
    let rows = diff::parse(patch);
    let new_lines: Vec<&str> = rows
        .iter()
        .filter(|r| r.new_no.is_some())
        .map(|r| r.text.as_str())
        .collect();
    let old_lines: Vec<&str> = rows
        .iter()
        .filter(|r| r.old_no.is_some())
        .map(|r| r.text.as_str())
        .collect();
    let first_line = new_lines
        .first()
        .or(old_lines.first())
        .copied()
        .unwrap_or("");
    let lang = detect(path, first_line);
    let sides = vec![Source::new(lang, &new_lines), Source::new(lang, &old_lines)];

    let largest = rows
        .iter()
        .flat_map(|r| [r.old_no, r.new_no])
        .flatten()
        .max()
        .unwrap_or(0);
    let width = largest.to_string().len().max(4);
    let number = |n: Option<u32>| n.map(|n| n.to_string()).unwrap_or_default();
    let (mut new_index, mut old_index) = (0, 0);
    let doc_rows = rows
        .iter()
        .map(|row| {
            let (marker, background) = match row.kind {
                Kind::Add => ('+', Some(theme::ADDITION_BACKGROUND)),
                Kind::Remove => ('-', Some(theme::REMOVAL_BACKGROUND)),
                Kind::Context => (' ', None),
            };
            let source = if row.new_no.is_some() {
                Some((0, new_index))
            } else {
                Some((1, old_index))
            };
            new_index += usize::from(row.new_no.is_some());
            old_index += usize::from(row.old_no.is_some());
            DocRow {
                gutter: format!(
                    "{marker} {:>width$} {:>width$} ",
                    number(row.old_no),
                    number(row.new_no)
                ),
                text: row.text.clone(),
                gutter_style: Style::new(),
                background,
                source,
            }
        })
        .collect();
    Document {
        rows: doc_rows,
        changes: diff::change_indices(&rows),
        wrap_indent: 0,
        sides,
    }
}

/// Read and prepare a bounded UTF-8 text file preview.
pub fn load_preview(path: &Path) -> Document {
    const TOO_LARGE: &str = "File is too large to preview (maximum 1 MiB).";
    let read = || -> std::io::Result<Result<Vec<u8>, &'static str>> {
        let metadata = path.symlink_metadata()?;
        if !metadata.file_type().is_file() {
            return Ok(Err("Only regular files can be previewed."));
        }
        if metadata.len() > MAX_PREVIEW_BYTES {
            return Ok(Err(TOO_LARGE));
        }
        let mut data = Vec::new();
        std::fs::File::open(path)?
            .take(MAX_PREVIEW_BYTES + 1)
            .read_to_end(&mut data)?;
        Ok(Ok(data))
    };
    let data = match read() {
        Ok(Ok(data)) => data,
        Ok(Err(message)) => return Document::message(message),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Document::message("File is no longer available.");
        }
        Err(_) => return Document::message("File could not be read."),
    };
    if data.len() as u64 > MAX_PREVIEW_BYTES {
        return Document::message(TOO_LARGE);
    }
    if data.contains(&0) {
        return Document::message("Binary files cannot be previewed.");
    }
    let Ok(source) = String::from_utf8(data) else {
        return Document::message("File is not valid UTF-8.");
    };

    let source = source.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = source.split('\n').collect();
    let name = path.to_string_lossy();
    let side = Source::new(detect(&name, lines[0]), &lines);
    let width = lines.len().to_string().len();
    let rows = lines
        .iter()
        .enumerate()
        .map(|(i, text)| DocRow {
            gutter: format!(" {:>width$} ", i + 1),
            text: text.to_string(),
            gutter_style: Style::new().add_modifier(Modifier::DIM),
            background: None,
            source: Some((0, i)),
        })
        .collect();
    Document {
        rows,
        changes: Vec::new(),
        wrap_indent: width + 2,
        sides: vec![side],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUTTER_WIDTH: usize = 12;

    const PATCH: &str = "\
diff --git a/module.py b/module.py
index 1111111..2222222 100644
--- a/module.py
+++ b/module.py
@@ -1,3 +1,4 @@
 import os
-x = 1
+x = 2
+
 def f():
";

    /// Wrap hunk lines in file headers so they parse as a unified patch.
    fn hunk(body: &str) -> String {
        let old = body.lines().filter(|l| !l.starts_with('+')).count();
        let new = body.lines().filter(|l| !l.starts_with('-')).count();
        format!("--- a/f\n+++ b/f\n@@ -1,{old} +1,{new} @@\n{body}")
    }

    fn lines(doc: &Document) -> Vec<Line<'static>> {
        (0..doc.rows.len()).map(|i| doc.line(i)).collect()
    }

    fn plain(line: &Line) -> String {
        line.to_string()
    }

    fn plains(doc: &Document) -> Vec<String> {
        lines(doc).iter().map(plain).collect()
    }

    /// The effective style of the character at `offset`.
    fn style_at(line: &Line, offset: usize) -> Style {
        let mut start = 0;
        for span in &line.spans {
            let len = span.content.chars().count();
            if offset < start + len {
                return line.style.patch(span.style);
            }
            start += len;
        }
        line.style
    }

    /// Distinct foreground colors across the source column.
    fn source_colors(line: &Line) -> Vec<Option<Color>> {
        let mut colors: Vec<Option<Color>> = (GUTTER_WIDTH..plain(line).chars().count())
            .map(|i| style_at(line, i).fg)
            .collect();
        colors.sort_by_key(|c| format!("{c:?}"));
        colors.dedup();
        colors
    }

    #[test]
    fn diff_document_prepares_visible_rows_and_change_positions() {
        let doc = diff_document("module.py", PATCH);

        assert_eq!(
            plains(&doc),
            [
                "     1    1 import os",
                "-    2      x = 1",
                "+         2 x = 2",
                "+         3 ",
                "     3    4 def f():",
            ]
        );
        assert_eq!(doc.changes, [1]);
        assert_eq!(doc.first_change(), Some(1));
        let [context, removed, added, blank_added, _] = lines(&doc).try_into().ok().unwrap();
        assert!(style_at(&removed, 0).bg.is_some());
        assert!(style_at(&added, 0).bg.is_some());
        assert_ne!(style_at(&removed, 0).bg, style_at(&added, 0).bg);
        assert_eq!(style_at(&blank_added, 0).bg, style_at(&added, 0).bg);
        assert_eq!(doc.rows[3].background(), doc.rows[2].background());
        assert_ne!(style_at(&context, 0).bg, style_at(&added, 0).bg);
        assert!(style_at(&context, GUTTER_WIDTH).fg.is_some());
    }

    #[test]
    fn diff_document_reports_no_changes_for_an_all_context_patch() {
        let doc = diff_document("notes.txt", &hunk(" context one\n context two\n"));

        assert_eq!(
            plains(&doc),
            ["     1    1 context one", "     2    2 context two"]
        );
        assert_eq!(doc.changes, Vec::<usize>::new());
        assert_eq!(doc.first_change(), None);
    }

    #[test]
    fn diff_rows_show_bracketed_source_literally() {
        let doc = diff_document("notes.txt", &hunk("-[old]\n+[new] [/bold]\n"));

        assert_eq!(
            plains(&doc),
            ["-    1      [old]", "+         1 [new] [/bold]"]
        );
    }

    #[test]
    fn diff_document_follows_the_patch_text() {
        let first = diff_document("module.py", &hunk("-a = 1\n+a = 2\n"));
        let second = diff_document("module.py", &hunk("-a = 1\n+a = 3\n"));

        assert!(plain(&first.line(1)).ends_with("a = 2"));
        assert!(plain(&second.line(1)).ends_with("a = 3"));
    }

    #[test]
    fn diff_document_colors_follow_the_entry_path() {
        let patch = hunk("-value\n+value = 1\n");

        let as_python = diff_document("value.py", &patch);
        let as_text = diff_document("value.txt", &patch);

        assert_eq!(plains(&as_python), plains(&as_text));
        assert!(source_colors(&as_python.line(1)).len() > 1);
        assert_eq!(source_colors(&as_text.line(1)).len(), 1);
    }

    #[test]
    fn diff_rows_color_source_by_filename_language() {
        for (path, source, colored_offset) in [
            ("example.py", "import os", 0),
            ("example.PY", "import os", 0),
            ("src/dir with spaces/example.js", "const x = 1;", 0),
            ("example.json", r#"{"key": 1}"#, 1),
            ("example.rs", "fn main() {}", 0),
        ] {
            let doc = diff_document(path, &hunk(&format!(" {source}\n")));

            let line = doc.line(0);
            assert_eq!(plain(&line), format!("     1    1 {source}"));
            assert!(
                style_at(&line, GUTTER_WIDTH + colored_offset).fg.is_some(),
                "{path}"
            );
        }
    }

    #[test]
    fn unknown_file_types_render_plain() {
        let doc = diff_document("notes.unknown", &hunk(" plain words\n"));

        let line = doc.line(0);
        assert_eq!(plain(&line), "     1    1 plain words");
        assert_eq!(source_colors(&line), [None]);
    }

    #[test]
    fn new_side_keeps_blank_lines_and_order_for_highlighting() {
        let doc = diff_document(
            "src/example.py",
            &hunk(" import os\n-removed = 1\n+\n def f(): pass\n"),
        );

        assert_eq!(
            plains(&doc),
            [
                "     1    1 import os",
                "-    2      removed = 1",
                "+         2 ",
                "     3    3 def f(): pass"
            ]
        );
        assert!(style_at(&doc.line(0), GUTTER_WIDTH).fg.is_some());
        assert!(style_at(&doc.line(3), GUTTER_WIDTH).fg.is_some());
    }

    #[test]
    fn all_removals_show_as_removed_rows_highlighted_from_the_old_side() {
        let doc = diff_document("removed.py", &hunk("-import os\n-x = 1\n"));

        assert_eq!(plains(&doc), ["-    1      import os", "-    2      x = 1"]);
        for line in lines(&doc) {
            let last = plain(&line).chars().count() - 1;
            assert!(style_at(&line, 0).bg.is_some());
            assert!(style_at(&line, last).bg.is_some());
        }
        assert!(style_at(&doc.line(0), GUTTER_WIDTH).fg.is_some());
    }

    #[test]
    fn diff_rows_use_plain_columns_and_full_width_change_backgrounds() {
        let doc = diff_document(
            "example.txt",
            &hunk(" context [not markup]\n-removed\n+added\n"),
        );
        let [context, removed, added] = lines(&doc).try_into().ok().unwrap();

        assert_eq!(plain(&context), "     1    1 context [not markup]");
        assert_eq!(plain(&removed), "-    2      removed");
        assert_eq!(plain(&added), "+         2 added");
        assert_eq!(style_at(&context, 0).bg, None);
        assert_eq!(doc.rows[0].background(), None);
        for (line, row) in [(&removed, &doc.rows[1]), (&added, &doc.rows[2])] {
            let last = plain(line).chars().count() - 1;
            assert!(style_at(line, 0).bg.is_some());
            assert_eq!(style_at(line, last).bg, style_at(line, 0).bg);
            assert_eq!(row.background(), style_at(line, 0).bg);
        }
        assert_ne!(style_at(&removed, 0).bg, style_at(&added, 0).bg);
    }

    #[test]
    fn diff_rows_preserve_empty_rows() {
        let doc = diff_document("empty.txt", &hunk(" \n"));

        assert_eq!(plains(&doc), ["     1    1 "]);
        assert_eq!(style_at(&doc.line(0), 0), Style::new());
    }

    #[test]
    fn syntax_colors_source_not_gutter_and_removed_rows_use_the_old_side() {
        let patch = "--- a/f\n+++ b/f\n@@ -20,2 +10,1 @@\n import os\n-import sys\n@@ -30,0 +30,1 @@\n+import re\n";
        let doc = diff_document("src/example.py", patch);
        let [context, removed, added] = lines(&doc).try_into().ok().unwrap();

        assert_eq!(plain(&context), "    20   10 import os");
        assert_eq!(plain(&removed), "-   21      import sys");
        assert_eq!(plain(&added), "+        30 import re");
        assert!(style_at(&context, GUTTER_WIDTH).fg.is_some());
        assert!(style_at(&added, GUTTER_WIDTH).fg.is_some());
        assert!(style_at(&added, GUTTER_WIDTH).bg.is_some());
        assert!(style_at(&removed, GUTTER_WIDTH).fg.is_some());
        for line in [&context, &removed, &added] {
            assert_eq!(style_at(line, 0).fg, None);
        }
    }

    #[test]
    fn removed_rows_are_highlighted_from_the_old_side() {
        // Only the old side wraps `x = y` in a string, so the removed row is string
        // text there, while the added `x = y` is plain code on the new side.
        let patch = hunk("-s = \"\"\"\n-x = y\n-\"\"\"\n+x = y\n");
        let doc = diff_document("example.py", &patch);
        assert_ne!(
            style_at(&doc.line(1), GUTTER_WIDTH).fg,
            style_at(&doc.line(3), GUTTER_WIDTH).fg
        );
    }

    #[test]
    fn gutter_widens_beyond_9999_lines() {
        let patch = "--- a/f\n+++ b/f\n@@ -12345 +12345 @@\n-old\n+new\n";
        let doc = diff_document("f.txt", patch);

        assert_eq!(plains(&doc), ["- 12345       old", "+       12345 new"]);
    }

    #[test]
    fn unsupported_entries_show_their_reason_as_one_plain_row() {
        let doc = Document::message("Binary files are not supported.");

        assert_eq!(plains(&doc), ["Binary files are not supported."]);
        assert_eq!(style_at(&doc.line(0), 0), Style::new());
    }

    /// A file in its own temporary directory, removed when dropped.
    struct TempFile(std::path::PathBuf);

    impl std::ops::Deref for TempFile {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
        }
    }

    fn temp_file(name: &str, contents: &[u8]) -> TempFile {
        let dir =
            std::env::temp_dir().join(format!("gitplume-document-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();
        TempFile(path)
    }

    #[test]
    fn preview_builds_numbered_highlighted_lines() {
        let doc = load_preview(&temp_file("example.py", b"answer = 42\n"));

        assert_eq!(plains(&doc), [" 1 answer = 42", " 2 "]);
        let line = doc.line(0);
        assert_eq!(line.style, Style::new());
        assert_eq!(line.spans[0].content, " 1 ");
        assert_eq!(
            line.spans[0].style,
            Style::new().add_modifier(Modifier::DIM)
        );
        assert_eq!(style_at(&line, 0).fg, None);
        assert!(source_colors_from(&line, 3).len() > 1);
        assert_eq!(doc.wrap_indent, 3);
    }

    fn source_colors_from(line: &Line, start: usize) -> Vec<String> {
        let mut colors: Vec<String> = (start..plain(line).chars().count())
            .map(|i| format!("{:?}", style_at(line, i).fg))
            .collect();
        colors.sort();
        colors.dedup();
        colors
    }

    #[test]
    fn preview_preserves_logical_lines() {
        for (i, (source, expected)) in [
            ("", &[" 1 "][..]),
            ("first", &[" 1 first"]),
            ("first\n", &[" 1 first", " 2 "]),
            ("first\n\n", &[" 1 first", " 2 ", " 3 "]),
            ("first\rsecond", &[" 1 first", " 2 second"]),
            ("first\r\nsecond", &[" 1 first", " 2 second"]),
        ]
        .into_iter()
        .enumerate()
        {
            let doc = load_preview(&temp_file(&format!("lines{i}.txt"), source.as_bytes()));
            assert_eq!(plains(&doc), expected, "{source:?}");
        }
    }

    #[test]
    fn preview_gutter_widens_with_the_line_count() {
        let doc = load_preview(&temp_file("long.txt", "x\n".repeat(10).as_bytes()));

        assert_eq!(plain(&doc.line(0)), "  1 x");
        assert_eq!(plain(&doc.line(10)), " 11 ");
        assert_eq!(doc.wrap_indent, 4);
    }

    #[test]
    fn preview_returns_friendly_messages() {
        let large = vec![b'x'; MAX_PREVIEW_BYTES as usize + 1];
        for (name, contents, message) in [
            (
                "binary.dat",
                &b"before\0after"[..],
                "Binary files cannot be previewed.",
            ),
            ("invalid.txt", b"\xff", "File is not valid UTF-8."),
            (
                "large.txt",
                &large,
                "File is too large to preview (maximum 1 MiB).",
            ),
        ] {
            let doc = load_preview(&temp_file(name, contents));
            assert_eq!(plains(&doc), [message]);
        }
    }

    #[test]
    fn preview_handles_disappeared_file() {
        let present = temp_file("present.txt", b"");
        let path = present.with_file_name("gone.txt");

        assert_eq!(
            plains(&load_preview(&path)),
            ["File is no longer available."]
        );
    }

    #[test]
    fn preview_handles_unreadable_file() {
        // A path below a regular file fails with "not a directory", whoever runs it.
        let file = temp_file("unreadable.txt", b"contents");
        let path = file.join("child.txt");

        assert_eq!(plains(&load_preview(&path)), ["File could not be read."]);
    }

    #[test]
    fn preview_rejects_non_regular_files() {
        let target = temp_file("target.txt", b"outside the selected path");
        let link = target.with_file_name("link.txt");
        std::os::unix::fs::symlink(&*target, &link).unwrap();

        assert_eq!(
            plains(&load_preview(&link)),
            ["Only regular files can be previewed."]
        );
    }
}
