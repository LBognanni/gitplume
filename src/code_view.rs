//! Virtual viewer state, rendering, scrolling, wrapping, selection.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use unicode_width::UnicodeWidthChar;

use crate::document::Document;
use crate::theme;

const TAB: usize = 8;
const WHEEL: usize = 3;
const VBAR: u16 = 2;

/// A (source row, character offset) position in the displayed row text.
type Point = (usize, usize);

/// A highlighted source row: (row, styled characters, row background style).
type Materialized = (usize, Vec<(char, Style)>, Style);

/// Map a scrollbar track position to a centered document position.
pub fn scrollbar_click_target(
    y: usize,
    height: usize,
    virtual_size: usize,
    window: usize,
) -> usize {
    let max = virtual_size.saturating_sub(window);
    if height == 0 || y == 0 {
        return 0;
    }
    if y >= height - 1 {
        return max;
    }
    let target = (y as f64 + 0.5) / height as f64 * virtual_size as f64 - window as f64 / 2.0;
    (target.max(0.0).round() as usize).min(max)
}

/// Thumb (start, length) on a track.
fn thumb(track: usize, virtual_size: usize, window: usize, scroll: usize) -> (usize, usize) {
    let len = (track * window)
        .checked_div(virtual_size)
        .unwrap_or(track)
        .clamp(1, track.max(1));
    let max = virtual_size.saturating_sub(window);
    let start = (scroll * (track - len.min(track)))
        .checked_div(max)
        .unwrap_or(0);
    (start, len)
}

/// A drawn scrollbar's geometry: `window` of `size` on a `track` starting at
/// screen coordinate `origin` along the bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scrollbar {
    pub vertical: bool,
    pub origin: u16,
    pub track: usize,
    pub size: usize,
    pub window: usize,
}

/// The outcome of pressing a scrollbar.
pub enum Press {
    /// The thumb was grabbed this many cells below its start.
    Grab(usize),
    /// The track was clicked: scroll to this position.
    Jump(usize),
}

impl Scrollbar {
    pub fn new(bar: Rect, vertical: bool, size: usize, window: usize) -> Self {
        let (origin, track) = if vertical {
            (bar.y, bar.height)
        } else {
            (bar.x, bar.width)
        };
        Self {
            vertical,
            origin,
            track: track as usize,
            size,
            window,
        }
    }

    fn along(&self, pos: Position) -> usize {
        let at = if self.vertical { pos.y } else { pos.x };
        at.saturating_sub(self.origin) as usize
    }

    /// Press at `pos` while scrolled to `scroll`.
    pub fn press(&self, pos: Position, scroll: usize) -> Press {
        let along = self.along(pos);
        let (start, len) = thumb(self.track, self.size, self.window, scroll);
        if (start..start + len).contains(&along) {
            Press::Grab(along - start)
        } else {
            Press::Jump(scrollbar_click_target(
                along,
                self.track,
                self.size,
                self.window,
            ))
        }
    }

    /// The scroll position for a thumb grabbed at `grab` dragged to `pos`.
    pub fn drag(&self, pos: Position, grab: usize) -> usize {
        let (_, len) = thumb(self.track, self.size, self.window, 0);
        let start = self.along(pos).saturating_sub(grab);
        let max = self.size.saturating_sub(self.window);
        (start * max)
            .checked_div(self.track.saturating_sub(len))
            .unwrap_or(0)
            .min(max)
    }
}

/// Whether `rows` rows up to `width` cells wide need (vertical, horizontal)
/// scrollbars in a `w` x `h` area, counting the space each bar takes; the
/// vertical bar is `bar` columns wide.
fn needs_bars(w: usize, h: usize, rows: usize, width: usize, bar: usize) -> (bool, bool) {
    let mut vertical = rows > h;
    let horizontal = width > w.saturating_sub(bar * usize::from(vertical));
    if horizontal && !vertical {
        vertical = rows > h.saturating_sub(1);
    }
    let horizontal = width > w.saturating_sub(bar * usize::from(vertical));
    (vertical, horizontal)
}

/// Split `area` into content and the (vertical, horizontal) scrollbars needed
/// to show `rows` rows up to `width` cells wide, with a one-column vertical bar.
pub fn scrollbar_layout(
    area: Rect,
    rows: usize,
    width: usize,
) -> (Rect, Option<Rect>, Option<Rect>) {
    let (vertical, horizontal) =
        needs_bars(area.width as usize, area.height as usize, rows, width, 1);
    let content_w = area.width.saturating_sub(u16::from(vertical));
    let content_h = area.height.saturating_sub(u16::from(horizontal));
    (
        Rect::new(area.x, area.y, content_w, content_h),
        vertical.then(|| Rect::new(area.x + content_w, area.y, 1, content_h)),
        horizontal.then(|| Rect::new(area.x, area.y + content_h, content_w, 1)),
    )
}

/// Draw a scrollbar track in `bar` for a `window` onto `size` scrolled to `scroll`.
pub fn render_scrollbar(
    buf: &mut Buffer,
    bar: Rect,
    vertical: bool,
    size: usize,
    window: usize,
    scroll: usize,
    half_on: Option<Color>,
) {
    let track = if vertical { bar.height } else { bar.width } as usize;
    let (start, len) = thumb(track, size, window, scroll);
    for (x, y) in bar.positions().map(|p| (p.x, p.y)) {
        let i = if vertical { y - bar.y } else { x - bar.x } as usize;
        let color = if (start..start + len).contains(&i) {
            theme::SCROLLBAR
        } else {
            theme::SCROLLBAR_BACKGROUND
        };
        buf[(x, y)].reset();
        match half_on {
            // A thinner bar: the lower half block over the pane surface.
            Some(pane) => {
                buf[(x, y)]
                    .set_symbol("▄")
                    .set_style(Style::new().fg(color).bg(pane));
            }
            None => {
                buf[(x, y)].set_style(Style::new().bg(color));
            }
        }
    }
}

fn char_width(c: char) -> usize {
    c.width().unwrap_or(0)
}

/// Expand tabs to 8-column stops, continuing from cell column `col`.
fn expand(text: &str, col: &mut usize, mut push: impl FnMut(char)) {
    for c in text.chars() {
        if c == '\t' {
            let spaces = TAB - *col % TAB;
            for _ in 0..spaces {
                push(' ');
            }
            *col += spaces;
        } else {
            push(c);
            *col += char_width(c);
        }
    }
}

/// The character offset at the given cell offset.
fn char_offset(chars: &[char], cells: usize) -> usize {
    let mut col = 0;
    for (i, &c) in chars.iter().enumerate() {
        let w = char_width(c);
        if col + w > cells {
            return i;
        }
        col += w;
    }
    chars.len()
}

/// Word-wrap `body` to `width` cells; returns the start offset of each line.
fn wrap(body: &[char], width: usize) -> Vec<usize> {
    let mut starts = vec![0];
    let mut pos = 0;
    let mut i = 0;
    while i < body.len() {
        // A word is leading spaces, non-spaces, then trailing spaces.
        let start = i;
        while i < body.len() && body[i] == ' ' {
            i += 1;
        }
        while i < body.len() && body[i] != ' ' {
            i += 1;
        }
        let trimmed_end = i;
        while i < body.len() && body[i] == ' ' {
            i += 1;
        }
        let cells =
            |r: std::ops::Range<usize>| body[r].iter().map(|&c| char_width(c)).sum::<usize>();
        let trimmed = cells(start..trimmed_end);
        if pos + trimmed > width {
            if trimmed > width {
                if pos > 0 {
                    starts.push(start);
                    pos = 0;
                }
                for (k, &c) in body[start..i].iter().enumerate() {
                    let w = char_width(c);
                    if pos > 0 && pos + w > width {
                        starts.push(start + k);
                        pos = 0;
                    }
                    pos += w;
                }
                continue;
            }
            starts.push(start);
            pos = 0;
        }
        pos += cells(start..i);
    }
    starts
}

/// One rendered row: a slice of a source row's displayed text.
#[derive(Clone, Copy)]
struct Visual {
    row: usize,
    /// Character offset where this row's text starts in the source row.
    start: usize,
    end: usize,
    /// Continuation rows render the wrap indent before `start..end`;
    /// first rows render `0..end`.
    continuation: bool,
}

enum Drag {
    None,
    Thumb { vertical: bool, grab: usize },
    Select(Point),
}

pub struct CodeView {
    doc: Document,
    /// Tab-expanded displayed text (gutter plus source) per source row.
    texts: Vec<Vec<char>>,
    max_width: usize,
    visual: Vec<Visual>,
    first_visual: Vec<usize>,
    /// Width the wrapped index was built for.
    index_width: Option<usize>,
    /// Area (width, height) the wrapped index was built for.
    wrap_area: (usize, usize),
    wrapped: bool,
    scroll_x: usize,
    scroll_y: usize,
    pending_row: Option<usize>,
    area: Rect,
    content: Rect,
    vbar: Option<Rect>,
    hbar: Option<Rect>,
    selection: Option<(Point, Point)>,
    drag: Drag,
    materialized: usize,
}

impl Default for CodeView {
    fn default() -> Self {
        Self::new()
    }
}

impl CodeView {
    pub fn new() -> Self {
        let mut view = Self {
            doc: Document::message(""),
            texts: Vec::new(),
            max_width: 0,
            visual: Vec::new(),
            first_visual: Vec::new(),
            index_width: None,
            wrap_area: (0, 0),
            wrapped: false,
            scroll_x: 0,
            scroll_y: 0,
            pending_row: None,
            area: Rect::default(),
            content: Rect::default(),
            vbar: None,
            hbar: None,
            selection: None,
            drag: Drag::None,
            materialized: 0,
        };
        view.doc.rows.clear();
        view
    }

    /// Document rows materialized during the last draw.
    pub fn materialized(&self) -> usize {
        self.materialized
    }

    pub fn document(&self) -> &Document {
        &self.doc
    }

    /// Replace the document, clear the selection, and reset scroll to (0, 0).
    pub fn set_document(&mut self, doc: Document) {
        self.texts = doc
            .rows
            .iter()
            .map(|row| {
                let mut chars = Vec::new();
                let mut col = 0;
                expand(&row.gutter, &mut col, |c| chars.push(c));
                expand(&row.text, &mut col, |c| chars.push(c));
                chars
            })
            .collect();
        self.max_width = self
            .texts
            .iter()
            .map(|t| t.iter().map(|&c| char_width(c)).sum())
            .max()
            .unwrap_or(0);
        self.doc = doc;
        self.selection = None;
        self.drag = Drag::None;
        self.index_width = None;
        self.scroll_x = 0;
        self.scroll_y = 0;
        self.relayout(self.area);
    }

    pub fn wrapped(&self) -> bool {
        self.wrapped
    }

    /// Toggle wrapping, keeping relative vertical progress.
    pub fn set_wrapped(&mut self, wrapped: bool) {
        if wrapped == self.wrapped {
            return;
        }
        let (_, max_y) = self.max_scroll();
        let progress = if max_y > 0 {
            self.scroll_y as f64 / max_y as f64
        } else {
            0.0
        };
        self.wrapped = wrapped;
        self.index_width = None;
        self.relayout(self.area);
        let (_, max_y) = self.max_scroll();
        self.scroll_to(0, (progress * max_y as f64).round() as usize);
    }

    pub fn scroll_offset(&self) -> (usize, usize) {
        (self.scroll_x, self.scroll_y)
    }

    /// Virtual (width, height) of the rendered rows.
    pub fn virtual_size(&self) -> (usize, usize) {
        let width = if self.wrapped {
            if self.visual.is_empty() {
                0
            } else {
                self.content.width as usize
            }
        } else {
            self.max_width
        };
        (width, self.visual.len())
    }

    /// The text area of the last draw, excluding scrollbars.
    pub fn viewport(&self) -> Rect {
        self.content
    }

    pub fn max_scroll(&self) -> (usize, usize) {
        let (w, h) = self.virtual_size();
        (
            w.saturating_sub(self.content.width as usize),
            h.saturating_sub(self.content.height as usize),
        )
    }

    /// Scroll to a position, clamped to the document bounds once drawn.
    pub fn scroll_to(&mut self, x: usize, y: usize) {
        self.scroll_x = x;
        self.scroll_y = y;
        self.clamp();
    }

    /// Put a source row at the top; resolved on the next draw if not drawn yet.
    pub fn scroll_to_row(&mut self, row: usize) {
        if self.area.is_empty() {
            self.pending_row = Some(row);
        } else {
            self.scroll_to(self.scroll_x, self.source_to_visual_row(row));
        }
    }

    /// The first visual row of a source row.
    pub fn source_to_visual_row(&self, row: usize) -> usize {
        match self.first_visual.len() {
            0 => 0,
            n => self.first_visual[row.min(n - 1)],
        }
    }

    fn clamp(&mut self) {
        if self.area.is_empty() {
            return;
        }
        let (max_x, max_y) = self.max_scroll();
        self.scroll_x = self.scroll_x.min(max_x);
        self.scroll_y = self.scroll_y.min(max_y);
    }

    /// Build the visual-row index, anchoring the top row across width changes.
    fn build(&self, width: Option<usize>) -> (Vec<Visual>, Vec<usize>) {
        let mut visual = Vec::new();
        let mut first_visual = Vec::new();
        for (row, text) in self.texts.iter().enumerate() {
            first_visual.push(visual.len());
            let Some(width) = width else {
                visual.push(Visual {
                    row,
                    start: 0,
                    end: text.len(),
                    continuation: false,
                });
                continue;
            };
            let indent = self.doc.wrap_indent.min(width - 1);
            let prefix = char_offset(text, indent);
            let starts = wrap(&text[prefix..], width - indent);
            for (i, &start) in starts.iter().enumerate() {
                let end = starts.get(i + 1).map_or(text.len(), |&s| prefix + s);
                visual.push(Visual {
                    row,
                    start: prefix + start,
                    end,
                    continuation: i > 0,
                });
            }
        }
        (visual, first_visual)
    }

    /// Rebuild the wrapped index for a `w` x `h` area once per size change,
    /// keeping the source position at the top of the viewport.
    fn rewrap(&mut self, w: usize, h: usize) {
        if self.index_width.is_some() && self.wrap_area == (w, h) {
            return;
        }
        let anchor = (self.index_width.is_some() && !self.visual.is_empty()).then(|| {
            let v = self.visual[self.scroll_y.min(self.visual.len() - 1)];
            (v.row, v.start)
        });
        let mut width = w;
        let (mut visual, mut first_visual) = self.build(Some(width));
        if visual.len() > h && w > VBAR as usize {
            width = w - VBAR as usize;
            (visual, first_visual) = self.build(Some(width));
        }
        self.visual = visual;
        self.first_visual = first_visual;
        self.index_width = Some(width);
        self.wrap_area = (w, h);
        if let Some((row, start)) = anchor {
            let first = self.first_visual[row];
            let last = self
                .first_visual
                .get(row + 1)
                .map_or(self.visual.len(), |&n| n);
            let offset = self.visual[first..last]
                .iter()
                .filter(|v| v.start <= start)
                .count();
            self.scroll_y = first + offset.saturating_sub(1);
        }
    }

    /// Lay out text and scrollbars inside `area` and rebuild the index if needed.
    fn relayout(&mut self, area: Rect) {
        self.area = area;
        if area.is_empty() {
            self.content = Rect::default();
            self.vbar = None;
            self.hbar = None;
            return;
        }
        let (w, h) = (area.width as usize, area.height as usize);
        let (vertical, horizontal) = if self.wrapped {
            self.rewrap(w, h);
            (self.index_width < Some(w), false)
        } else {
            if self.index_width.is_none() {
                (self.visual, self.first_visual) = self.build(None);
                self.index_width = Some(0);
            }
            needs_bars(w, h, self.visual.len(), self.max_width, VBAR as usize)
        };
        let content_w = area.width.saturating_sub(VBAR * u16::from(vertical));
        let content_h = area.height.saturating_sub(u16::from(horizontal));
        self.content = Rect::new(area.x, area.y, content_w, content_h);
        self.vbar = vertical.then(|| {
            Rect::new(
                area.x + content_w,
                area.y,
                area.width - content_w,
                content_h,
            )
        });
        self.hbar = horizontal.then(|| Rect::new(area.x, area.y + content_h, content_w, 1));
        if self.wrapped {
            self.scroll_x = 0;
        }
        self.clamp();
    }

    /// Scroll with a key; returns whether the key was handled.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        let (x, y) = (self.scroll_x, self.scroll_y);
        let page = self.content.height as usize;
        let (x, y) = match key.code {
            KeyCode::Up | KeyCode::Char('k') => (x, y.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => (x, y + 1),
            KeyCode::Left => (x.saturating_sub(1), y),
            KeyCode::Right => (x + 1, y),
            KeyCode::PageUp => (x, y.saturating_sub(page)),
            KeyCode::PageDown => (x, y + page),
            KeyCode::Home => (x, 0),
            KeyCode::End => (x, usize::MAX),
            _ => return false,
        };
        self.scroll_to(x, y);
        true
    }

    /// Handle a mouse event; returns text to copy to the clipboard, if any.
    pub fn handle_mouse(&mut self, event: MouseEvent) -> Option<String> {
        let pos = Position::new(event.column, event.row);
        let (x, y) = (self.scroll_x, self.scroll_y);
        let shift = event.modifiers.contains(KeyModifiers::SHIFT);
        match event.kind {
            MouseEventKind::ScrollDown if shift => self.scroll_to(x + WHEEL, y),
            MouseEventKind::ScrollUp if shift => self.scroll_to(x.saturating_sub(WHEEL), y),
            MouseEventKind::ScrollDown => self.scroll_to(x, y + WHEEL),
            MouseEventKind::ScrollUp => self.scroll_to(x, y.saturating_sub(WHEEL)),
            MouseEventKind::ScrollRight => self.scroll_to(x + WHEEL, y),
            MouseEventKind::ScrollLeft => self.scroll_to(x.saturating_sub(WHEEL), y),
            MouseEventKind::Down(MouseButton::Left) => {
                let on_bar = |rect: Option<Rect>| rect.is_some_and(|r| r.contains(pos));
                let hit = if on_bar(self.vbar) {
                    self.bar(true)
                } else if on_bar(self.hbar) {
                    self.bar(false)
                } else {
                    None
                };
                if let Some(bar) = hit {
                    match bar.press(pos, self.axis_scroll(bar.vertical)) {
                        Press::Grab(grab) => {
                            self.drag = Drag::Thumb {
                                vertical: bar.vertical,
                                grab,
                            }
                        }
                        Press::Jump(value) => self.set_axis(bar.vertical, value),
                    }
                } else if self.content.contains(pos) {
                    self.selection = None;
                    self.drag = Drag::Select(self.point_at(pos));
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => match self.drag {
                Drag::Thumb { vertical, grab } => {
                    if let Some(bar) = self.bar(vertical) {
                        self.set_axis(vertical, bar.drag(pos, grab));
                    }
                }
                Drag::Select(anchor) => {
                    let point = self.point_at(pos);
                    self.selection = Some((anchor.min(point), anchor.max(point)));
                }
                Drag::None => {}
            },
            MouseEventKind::Up(MouseButton::Left) => {
                let selecting = matches!(self.drag, Drag::Select(_));
                self.drag = Drag::None;
                if selecting {
                    return self.selected_text().filter(|text| !text.is_empty());
                }
            }
            _ => {}
        }
        None
    }

    /// The drawn scrollbar for one axis, if any.
    fn bar(&self, vertical: bool) -> Option<Scrollbar> {
        let (w, h) = self.virtual_size();
        if vertical {
            let bar = self.vbar?;
            Some(Scrollbar::new(bar, true, h, self.content.height as usize))
        } else {
            let bar = self.hbar?;
            Some(Scrollbar::new(bar, false, w, self.content.width as usize))
        }
    }

    fn axis_scroll(&self, vertical: bool) -> usize {
        if vertical {
            self.scroll_y
        } else {
            self.scroll_x
        }
    }

    fn set_axis(&mut self, vertical: bool, value: usize) {
        if vertical {
            self.scroll_to(self.scroll_x, value);
        } else {
            self.scroll_to(value, self.scroll_y);
        }
    }

    /// Map a screen position to a selection point; past the end maps beyond the last row.
    fn point_at(&self, pos: Position) -> Point {
        let row = self.scroll_y + pos.y.saturating_sub(self.content.y) as usize;
        let Some(v) = self.visual.get(row) else {
            return (self.texts.len(), 0);
        };
        let cell = self.scroll_x + pos.x.saturating_sub(self.content.x) as usize;
        let text = &self.texts[v.row];
        if v.continuation {
            let indent = self.indent();
            let index = char_offset(&text[v.start..v.end], cell.saturating_sub(indent));
            (v.row, v.start + index)
        } else {
            (v.row, char_offset(&text[..v.end], cell))
        }
    }

    fn indent(&self) -> usize {
        self.doc
            .wrap_indent
            .min((self.content.width as usize).saturating_sub(1))
    }

    /// The selected displayed text; wrapped rows copy as their source text.
    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection?;
        let n = self.texts.len();
        if start.0 >= n {
            return Some(String::new());
        }
        let end = if end.0 >= n {
            (n - 1, self.texts[n - 1].len())
        } else {
            end
        };
        let slice = |row: usize, from: usize, to: usize| -> String {
            let text = &self.texts[row];
            text[from.min(text.len())..to.min(text.len()).max(from.min(text.len()))]
                .iter()
                .collect()
        };
        if start.0 == end.0 {
            return Some(slice(start.0, start.1, end.1));
        }
        let mut lines = vec![slice(start.0, start.1, usize::MAX)];
        lines.extend((start.0 + 1..end.0).map(|row| slice(row, 0, usize::MAX)));
        lines.push(slice(end.0, 0, end.1));
        Some(lines.join("\n"))
    }

    /// Selected character range of a source row.
    fn selected_range(&self, row: usize) -> Option<(usize, usize)> {
        let (start, end) = self.selection?;
        if row < start.0 || row > end.0 {
            return None;
        }
        let from = if row == start.0 { start.1 } else { 0 };
        let to = if row == end.0 { end.1 } else { usize::MAX };
        Some((from, to))
    }

    /// Draw the visible rows and scrollbars into `area`.
    pub fn render(&mut self, area: Rect, buf: &mut Buffer) {
        self.relayout(area);
        if area.is_empty() {
            return;
        }
        if let Some(row) = self.pending_row.take() {
            self.scroll_to(self.scroll_x, self.source_to_visual_row(row));
        }
        self.materialized = 0;
        let content = self.content;
        let indent = self.indent();
        let blank_gutter_width = if self.wrapped {
            indent.saturating_sub(1)
        } else {
            self.doc
                .wrap_indent
                .saturating_sub(1)
                .saturating_sub(self.scroll_x)
                .min(content.width as usize)
        } as u16;
        let mut cached: Option<Materialized> = None;
        for y in 0..content.height {
            let screen_y = content.y + y;
            for x in content.x..content.right() {
                buf[(x, screen_y)].reset();
            }
            let Some(&v) = self.visual.get(self.scroll_y + y as usize) else {
                buf.set_style(
                    Rect::new(content.x, screen_y, blank_gutter_width, 1),
                    theme::gutter(),
                );
                continue;
            };
            if cached.as_ref().is_none_or(|(row, ..)| *row != v.row) {
                cached = Some(self.materialize(v.row));
            }
            let (_, chars, base) = cached.as_ref().unwrap();
            // Display cells paired with their source character offsets.
            let mut cells: Vec<(char, Style, Option<usize>)> = Vec::new();
            if v.continuation {
                let gutter = self.doc.rows[v.row].gutter_style();
                cells.extend((0..indent).map(|x| {
                    let style = if x + 1 == indent { *base } else { gutter };
                    (' ', style, None)
                }));
                cells.extend((v.start..v.end).map(|i| (chars[i].0, chars[i].1, Some(i))));
            } else {
                cells.extend((0..v.end).map(|i| (chars[i].0, chars[i].1, Some(i))));
            }
            let selected = self.selected_range(v.row);
            let width = content.width as usize;
            let mut col = 0;
            for (c, mut style, offset) in cells {
                let w = char_width(c);
                if w == 0 {
                    continue;
                }
                if let (Some((from, to)), Some(offset)) = (selected, offset)
                    && (from..to).contains(&offset)
                {
                    style = style.bg(theme::FOCUSED_SELECTION);
                }
                let (left, right) = (col, col + w);
                col = right;
                if right <= self.scroll_x {
                    continue;
                }
                if left >= self.scroll_x + width {
                    break;
                }
                let x = content.x + left.saturating_sub(self.scroll_x) as u16;
                if left < self.scroll_x || right > self.scroll_x + width {
                    // A wide character cut by an edge shows as blank cells.
                    let visible = right.min(self.scroll_x + width) - left.max(self.scroll_x);
                    for dx in 0..visible as u16 {
                        buf[(x + dx, screen_y)].set_symbol(" ").set_style(style);
                    }
                } else {
                    buf.set_stringn(x, screen_y, c.to_string(), w, style);
                }
            }
            let filled = col.saturating_sub(self.scroll_x).min(width) as u16;
            for x in content.x + filled..content.right() {
                buf[(x, screen_y)].set_style(*base);
            }
        }
        if let Some(bar) = self.vbar {
            self.render_bar(buf, bar, true);
        }
        if let Some(bar) = self.hbar {
            self.render_bar(buf, bar, false);
        }
        if let (Some(v), Some(h)) = (self.vbar, self.hbar) {
            for x in v.x..v.right() {
                buf[(x, h.y)].reset();
                buf[(x, h.y)].set_style(Style::new().bg(theme::SCROLLBAR_BACKGROUND));
            }
        }
    }

    /// Highlight one source row into tab-expanded styled characters.
    fn materialize(&mut self, row: usize) -> Materialized {
        self.materialized += 1;
        let base = self.doc.rows[row]
            .background()
            .map_or(Style::new(), |bg| Style::new().bg(bg));
        let mut chars = Vec::new();
        let mut col = 0;
        for span in self.doc.line(row).spans {
            expand(&span.content, &mut col, |c| chars.push((c, span.style)));
        }
        (row, chars, base)
    }

    fn render_bar(&self, buf: &mut Buffer, bar: Rect, vertical: bool) {
        if let Some(b) = self.bar(vertical) {
            let scroll = self.axis_scroll(vertical);
            render_scrollbar(buf, bar, vertical, b.size, b.window, scroll, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use unicode_width::UnicodeWidthStr;

    use crate::document::diff_document;

    fn doc(lines: &[&str]) -> Document {
        let mut doc = Document::message("");
        doc.rows = lines
            .iter()
            .map(|line| Document::message(line).rows.pop().unwrap())
            .collect();
        doc
    }

    fn numbered(count: usize, suffix: &str) -> Document {
        let lines: Vec<String> = (0..count).map(|row| format!("{row:03}{suffix}")).collect();
        doc(&lines.iter().map(String::as_str).collect::<Vec<_>>())
    }

    fn draw(view: &mut CodeView, width: u16, height: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| view.render(frame.area(), frame.buffer_mut()))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    /// Visible text of a row, skipping the cells covered by wide characters.
    fn line(buffer: &Buffer, y: u16) -> String {
        let mut text = String::new();
        let mut x = 0;
        while x < buffer.area.width {
            let symbol = buffer[(x, y)].symbol();
            text.push_str(symbol);
            x += symbol.width().max(1) as u16;
        }
        text
    }

    fn lines(buffer: &Buffer, width: u16, height: u16) -> Vec<String> {
        (0..height)
            .map(|y| line(buffer, y).chars().take(width as usize).collect())
            .collect()
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    /// Drag-select from one cell to another and return the copied text.
    fn select(view: &mut CodeView, from: (u16, u16), to: (u16, u16)) -> Option<String> {
        view.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            from.0,
            from.1,
        ));
        view.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), to.0, to.1));
        view.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), to.0, to.1))
    }

    #[test]
    fn set_document_displays_content_and_extent() {
        let mut view = CodeView::new();
        view.set_document(doc(&["x\t界", "longest", "third"]));
        let buffer = draw(&mut view, 6, 2);

        assert_eq!(view.virtual_size(), (10, 3));
        let viewport = view.viewport();
        assert_eq!(
            view.max_scroll(),
            (10 - viewport.width as usize, 3 - viewport.height as usize)
        );
        assert!(line(&buffer, 0).starts_with("x   "));
        assert_eq!(view.scroll_offset(), (0, 0));
    }

    #[test]
    fn render_crops_and_preserves_source_and_row_styles() {
        let patch = "@@ -0,0 +1 @@\n+def f():\n";
        let mut view = CodeView::new();
        view.set_document(diff_document("module.py", patch));
        let gutter_text = view.document().rows[0].gutter.clone();
        let gutter = gutter_text.chars().count() as u16;
        let row_bg = view.document().rows[0].background().unwrap();
        let buffer = draw(&mut view, gutter + 3, 2);

        assert_eq!(line(&buffer, 0), format!("{gutter_text}def"));
        let keyword = buffer[(gutter, 0)].fg;
        assert_ne!(
            keyword,
            buffer[(1, 0)].fg,
            "keyword cells differ from gutter cells"
        );
        assert!((0..gutter - 1).all(|x| buffer[(x, 0)].bg != row_bg));
        assert!((gutter - 1..gutter + 3).all(|x| buffer[(x, 0)].bg == row_bg));

        draw(&mut view, 4, 2);
        view.scroll_to(gutter as usize - 1, 0);
        let buffer = draw(&mut view, 4, 2);
        assert_eq!(line(&buffer, 0), " def");
        assert_eq!(buffer[(1, 0)].fg, keyword);
        assert!((0..4).all(|x| buffer[(x, 0)].bg == row_bg));
    }

    #[test]
    fn rows_below_the_document_are_blank() {
        let mut view = CodeView::new();
        view.set_document(doc(&["abc"]));
        let buffer = draw(&mut view, 5, 2);
        assert_eq!(line(&buffer, 1), "     ");
    }

    #[test]
    fn gutter_background_continues_below_short_files_without_change_markers() {
        for wrapped in [false, true] {
            let mut view = CodeView::new();
            view.set_document(diff_document(
                "example.txt",
                "@@ -1 +1,2 @@\n context\n+added\n",
            ));
            let gutter = view.document().rows[0].gutter.len() as u16;
            view.set_wrapped(wrapped);
            let buffer = draw(&mut view, gutter + 10, 5);
            for y in 2..5 {
                assert_eq!(line(&buffer, y).trim(), "");
                for x in 0..gutter - 1 {
                    assert_eq!(buffer[(x, y)].bg, buffer[(0, 0)].bg);
                    assert_ne!(buffer[(x, y)].bg, buffer[(0, 1)].bg);
                }
                assert_ne!(buffer[(0, y)].bg, buffer[(gutter, y)].bg);
                assert_eq!(buffer[(gutter - 1, y)].bg, buffer[(gutter, y)].bg);
            }
        }
    }

    #[test]
    fn gutter_below_short_files_tracks_horizontal_scrolling() {
        let mut view = CodeView::new();
        let patch = format!("@@ -1 +1 @@\n {}\n", "x".repeat(80));
        view.set_document(diff_document("example.txt", &patch));
        let gutter = view.document().rows[0].gutter.len() as u16;
        draw(&mut view, gutter + 10, 5);
        view.scroll_to(4, 0);
        let buffer = draw(&mut view, gutter + 10, 5);
        for x in 0..gutter - 5 {
            assert_eq!(buffer[(x, 2)].bg, buffer[(0, 0)].bg);
        }
        assert_ne!(buffer[(gutter - 5, 2)].bg, buffer[(0, 0)].bg);

        view.scroll_to(gutter as usize, 0);
        let buffer = draw(&mut view, gutter + 10, 5);
        assert!((0..gutter + 10).all(|x| buffer[(x, 2)].bg == buffer[(gutter, 2)].bg));
    }

    #[test]
    fn resize_preserves_virtual_size_and_updates_cropping() {
        let mut view = CodeView::new();
        let lines: Vec<String> = (0..4).map(|row| format!("{row}ABCDEFGHIJK")).collect();
        view.set_document(doc(&lines.iter().map(String::as_str).collect::<Vec<_>>()));
        let buffer = draw(&mut view, 5, 2);
        assert_eq!(view.virtual_size(), (12, 4));
        assert_eq!(&line(&buffer, 0)[..3], "0AB");

        let buffer = draw(&mut view, 8, 3);
        assert_eq!(view.virtual_size(), (12, 4));
        // Six text columns, then the two-column vertical scrollbar.
        assert_eq!(line(&buffer, 0), "0ABCDE  ");

        view.scroll_to(3, 1);
        let buffer = draw(&mut view, 8, 3);
        assert_eq!(view.scroll_offset(), (3, 1));
        assert_eq!(&line(&buffer, 0)[..6], "CDEFGH");
    }

    #[test]
    fn empty_document_has_zero_virtual_dimensions() {
        let mut view = CodeView::new();
        view.set_document(doc(&[]));
        let buffer = draw(&mut view, 8, 3);
        assert_eq!(view.virtual_size(), (0, 0));
        assert_eq!(view.scroll_offset(), (0, 0));
        assert_eq!(line(&buffer, 0), " ".repeat(8));
    }

    #[test]
    fn wide_unicode_uses_cell_width_and_crops_by_cells() {
        let mut view = CodeView::new();
        view.set_document(doc(&["a界b"]));
        let buffer = draw(&mut view, 4, 1);
        assert_eq!(view.virtual_size(), (4, 1));
        assert_eq!(line(&buffer, 0), "a界b");

        let buffer = draw(&mut view, 3, 2);
        assert_eq!(line(&buffer, 0), "a界");

        view.scroll_to(1, 0);
        let buffer = draw(&mut view, 2, 2);
        assert_eq!(view.scroll_offset(), (1, 0));
        assert_eq!(line(&buffer, 0), "界");

        view.scroll_to(2, 0);
        let buffer = draw(&mut view, 2, 2);
        assert_eq!(
            line(&buffer, 0),
            " b",
            "a split wide character is never drawn"
        );
        // The first visible cell belongs to the wide character (offset 1).
        assert_eq!(select(&mut view, (0, 0), (2, 0)).as_deref(), Some("界b"));
    }

    #[test]
    fn repaint_work_is_bounded_by_viewport_not_document_length() {
        let materialized = |count: usize| {
            let mut view = CodeView::new();
            view.set_document(numbered(count, ""));
            let buffer = draw(&mut view, 20, 4);
            assert!(line(&buffer, 0).starts_with("000"));
            view.materialized()
        };
        let small = materialized(100);
        let large = materialized(100_000);
        assert!(0 < small && small <= 4);
        assert_eq!(large, small);

        let mut view = CodeView::new();
        view.set_document(numbered(100_000, &" word".repeat(20)));
        view.set_wrapped(true);
        draw(&mut view, 20, 4);
        assert!(view.materialized() <= 4);
    }

    #[test]
    fn document_replacement_renders_new_same_sized_content() {
        let mut view = CodeView::new();
        let xs = "x".repeat(40);
        view.set_document(doc(&vec![xs.as_str(); 100]));
        draw(&mut view, 8, 3);
        view.scroll_to(20, 50);

        let ys = "y".repeat(40);
        view.set_document(doc(&vec![ys.as_str(); 100]));
        let buffer = draw(&mut view, 8, 3);
        assert_eq!(view.virtual_size(), (40, 100));
        assert!(lines(&buffer, 6, 2).iter().all(|row| row == &"y".repeat(6)));
    }

    #[test]
    fn equal_document_replacement_resets_viewport() {
        let mut view = CodeView::new();
        let suffix = "x".repeat(40);
        view.set_document(numbered(100, &suffix));
        draw(&mut view, 8, 3);
        view.scroll_to(20, 50);
        let buffer = draw(&mut view, 8, 3);
        assert!(lines(&buffer, 6, 2).iter().all(|row| row == &"x".repeat(6)));

        view.set_document(numbered(100, &suffix));
        let buffer = draw(&mut view, 8, 3);
        assert_eq!(view.scroll_offset(), (0, 0));
        assert_eq!(view.max_scroll().1, 100 - view.viewport().height as usize);
        assert_eq!(lines(&buffer, 6, 2), ["000xxx", "001xxx"]);
    }

    #[test]
    fn keyboard_and_scrollbar_navigation_is_bounded() {
        let mut view = CodeView::new();
        view.set_document(numbered(30, &format!(" {}", "x".repeat(40))));
        draw(&mut view, 8, 4);

        view.handle_key(key(KeyCode::Down));
        view.handle_key(key(KeyCode::Right));
        assert_eq!(view.scroll_offset(), (1, 1));
        view.handle_key(key(KeyCode::Char('j')));
        view.handle_key(key(KeyCode::Char('k')));
        view.handle_key(key(KeyCode::Left));
        assert_eq!(view.scroll_offset(), (0, 1));

        let page = view.viewport().height as usize;
        view.handle_key(key(KeyCode::PageDown));
        assert_eq!(view.scroll_offset().1, 1 + page);

        let (max_x, max_y) = view.max_scroll();
        view.scroll_to(0, max_y - 1);
        for _ in 0..2 {
            view.handle_key(key(KeyCode::PageDown));
            assert_eq!(view.scroll_offset().1, max_y);
        }
        view.scroll_to(0, 1);
        for _ in 0..2 {
            view.handle_key(key(KeyCode::PageUp));
            assert_eq!(view.scroll_offset().1, 0);
        }
        view.scroll_to(max_x - 1, 0);
        for _ in 0..2 {
            view.handle_key(key(KeyCode::Right));
            assert_eq!(view.scroll_offset(), (max_x, 0));
        }
        view.handle_key(key(KeyCode::End));
        assert_eq!(view.scroll_offset().1, max_y);
        view.handle_key(key(KeyCode::Home));
        assert_eq!(view.scroll_offset().1, 0);

        // The vertical scrollbar is the last column above the horizontal one.
        let bar = view.viewport().right();
        let last = view.viewport().bottom() - 1;
        view.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), bar, last));
        assert_eq!(view.scroll_offset().1, max_y);
        view.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), bar, 0));
        assert_eq!(view.scroll_offset().1, 0);
    }

    #[test]
    fn vertical_scrollbar_is_two_columns_wide_and_both_respond() {
        let mut view = CodeView::new();
        view.set_document(numbered(30, ""));
        let buffer = draw(&mut view, 10, 4);
        assert_eq!(view.viewport().width, 8);
        let thumb = buffer[(8, 0)].bg;
        assert_eq!(buffer[(9, 0)].bg, thumb);
        assert_ne!(buffer[(8, 3)].bg, thumb, "the track below the thumb");
        for column in [8, 9] {
            view.scroll_to(0, 0);
            view.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), column, 3));
            assert_eq!(
                view.scroll_offset().1,
                view.max_scroll().1,
                "column {column}"
            );
            view.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), column, 3));
            // Drag the thumb from the bottom back to the top.
            view.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), column, 3));
            view.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), column, 0));
            view.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), column, 0));
            assert_eq!(view.scroll_offset().1, 0, "column {column}");
        }
    }

    #[test]
    fn mouse_wheel_scrolls_three_rows_or_columns() {
        let mut view = CodeView::new();
        view.set_document(numbered(30, &"x".repeat(40)));
        draw(&mut view, 8, 4);
        view.handle_mouse(mouse(MouseEventKind::ScrollDown, 0, 0));
        assert_eq!(view.scroll_offset(), (0, 3));
        let mut shifted = mouse(MouseEventKind::ScrollDown, 0, 0);
        shifted.modifiers = KeyModifiers::SHIFT;
        view.handle_mouse(shifted);
        assert_eq!(view.scroll_offset(), (3, 3));
        view.handle_mouse(mouse(MouseEventKind::ScrollLeft, 0, 0));
        view.handle_mouse(mouse(MouseEventKind::ScrollUp, 0, 0));
        assert_eq!(view.scroll_offset(), (0, 0));
    }

    #[test]
    fn dragging_the_thumb_scrolls_proportionally() {
        let mut view = CodeView::new();
        view.set_document(numbered(100, ""));
        draw(&mut view, 8, 10);
        let bar = view.viewport().right();
        view.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), bar, 0));
        assert_eq!(
            view.scroll_offset().1,
            0,
            "pressing the thumb does not jump"
        );
        view.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), bar, 5));
        let middle = view.scroll_offset().1;
        assert!(30 < middle && middle < 70, "{middle}");
        view.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), bar, 9));
        assert_eq!(view.scroll_offset().1, view.max_scroll().1);
        view.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), bar, 9));
    }

    #[test]
    fn scrollbar_target_handles_empty_and_short_documents() {
        assert_eq!(scrollbar_click_target(0, 0, 0, 0), 0);
        assert_eq!(scrollbar_click_target(0, 10, 2, 5), 0);
        assert_eq!(scrollbar_click_target(5, 10, 2, 5), 0);
        assert_eq!(scrollbar_click_target(5, 10, 100, 10), 50);
    }

    #[test]
    fn page_keys_move_the_viewport_immediately() {
        let mut view = CodeView::new();
        view.set_document(numbered(30, ""));
        draw(&mut view, 8, 4);
        let page = view.viewport().height as usize;

        view.handle_key(key(KeyCode::PageDown));
        assert_eq!(view.scroll_offset().1, page);
        let buffer = draw(&mut view, 8, 4);
        assert_eq!(&line(&buffer, 0)[..3], format!("{page:03}"));

        view.handle_key(key(KeyCode::PageUp));
        assert_eq!(view.scroll_offset().1, 0);
    }

    #[test]
    fn wrapping_maps_source_rows_and_preserves_progress() {
        let mut view = CodeView::new();
        let lines: Vec<String> = (0..20)
            .map(|row| format!("{row} {}", "x".repeat(20)))
            .collect();
        view.set_document(doc(&lines.iter().map(String::as_str).collect::<Vec<_>>()));
        draw(&mut view, 10, 4);
        let (_, max_y) = view.max_scroll();
        view.scroll_to(5, max_y / 2);
        let progress = view.scroll_offset().1 as f64 / max_y as f64;

        view.set_wrapped(true);
        let buffer = draw(&mut view, 10, 4);
        assert_eq!(view.scroll_offset().0, 0);
        assert!(view.source_to_visual_row(3) > 3);
        let (max_x, max_y) = view.max_scroll();
        assert_eq!(max_x, 0);
        let wrapped = view.scroll_offset().1 as f64 / max_y as f64;
        assert!((wrapped - progress).abs() <= 1.0 / max_y as f64);
        // Only the vertical scrollbar remains: the bottom row shows text.
        assert!(line(&buffer, 3).trim_end().chars().any(|c| c != ' '));
    }

    #[test]
    fn deferred_scroll_to_row_resolves_on_first_draw() {
        let mut view = CodeView::new();
        view.set_document(doc(&[
            "short",
            &"x".repeat(50),
            "target",
            "tail",
            "end",
            "more",
        ]));
        view.set_wrapped(true);
        view.scroll_to_row(2);
        let buffer = draw(&mut view, 10, 3);
        assert_eq!(line(&buffer, 0).trim_end(), "target");
    }

    #[test]
    fn selection_uses_displayed_expanded_text_and_replacement_clears_it() {
        let mut view = CodeView::new();
        view.set_document(doc(&[" 1 a\tb", " 2 second"]));
        draw(&mut view, 20, 4);
        assert_eq!(
            select(&mut view, (0, 0), (9, 0)).as_deref(),
            Some(" 1 a    b")
        );
        assert!(view.selected_text().is_some());

        view.set_document(doc(&["replacement"]));
        assert_eq!(view.selected_text(), None);
    }

    #[test]
    fn selection_handles_a_trailing_blank_line() {
        let mut view = CodeView::new();
        view.set_document(doc(&["first", ""]));
        draw(&mut view, 20, 4);
        assert_eq!(
            select(&mut view, (0, 0), (0, 1)).as_deref(),
            Some("first\n")
        );
        assert_eq!(
            select(&mut view, (0, 0), (0, 3)).as_deref(),
            Some("first\n")
        );
        assert_eq!(select(&mut view, (0, 1), (0, 1)), None);
    }

    #[test]
    fn selection_background_preserves_document_foreground() {
        let patch = "@@ -1 +1 @@\n def f():\n";
        let mut view = CodeView::new();
        view.set_document(diff_document("module.py", patch));
        let gutter = view.document().rows[0].gutter.chars().count() as u16;
        let before = draw(&mut view, 30, 4);
        select(&mut view, (gutter, 0), (gutter + 3, 0));
        let after = draw(&mut view, 30, 4);

        let (plain, selected) = (&before[(gutter, 0)], &after[(gutter, 0)]);
        assert_ne!(selected.fg, Color::Reset);
        assert_eq!(selected.fg, plain.fg);
        assert_ne!(selected.bg, plain.bg);
        assert_eq!(after[(gutter + 4, 0)].bg, before[(gutter + 4, 0)].bg);
    }

    #[test]
    fn wrapped_selection_uses_source_text_without_visual_newlines() {
        let mut view = CodeView::new();
        view.set_document(doc(&["abc   def", "second"]));
        view.set_wrapped(true);
        let buffer = draw(&mut view, 7, 4);

        assert!(view.virtual_size().1 > 2);
        let continuation = view.source_to_visual_row(0) as u16 + 1;
        assert!(line(&buffer, continuation).starts_with("def"));
        let second = view.source_to_visual_row(1) as u16;
        assert_eq!(
            select(&mut view, (0, 0), (3, second)).as_deref(),
            Some("abc   def\nsec")
        );
        // The continuation's first cell is source offset 6.
        assert_eq!(
            select(&mut view, (0, continuation), (3, continuation)).as_deref(),
            Some("def")
        );
    }

    #[test]
    fn wrapped_document_indents_continuations_without_a_gutter_only_row() {
        let mut view = CodeView::new();
        let mut document = doc(&[" 1 abcdefghijklmno"]);
        document.wrap_indent = 3;
        view.set_document(document);
        view.set_wrapped(true);
        let buffer = draw(&mut view, 9, 5);

        let rows: Vec<String> = (0..view.virtual_size().1 as u16)
            .map(|y| line(&buffer, y).trim_end().to_string())
            .collect();
        assert!(rows.len() > 1);
        assert!(rows[0].starts_with(" 1 abc"));
        assert!(rows[1..].iter().all(|row| row.starts_with("   ")));
        assert!(rows.iter().all(|row| !row.trim().is_empty()));
    }

    #[test]
    fn wrapped_diff_keeps_a_styled_gutter_and_copies_only_source_on_continuations() {
        for (marker, old, new) in [
            ('+', "0,0", "1123"),
            ('-', "1123", "0,0"),
            (' ', "1123", "1123"),
        ] {
            let patch = format!("@@ -{old} +{new} @@\n{marker}abcdefghijklmnopqr\n");
            let mut view = CodeView::new();
            view.set_document(diff_document("example.txt", &patch));
            let gutter = view.document().rows[0].gutter.len() as u16;
            let prefix = view.document().rows[0].gutter.clone();
            view.set_wrapped(true);
            let buffer = draw(&mut view, gutter + 6, 6);

            assert_eq!(line(&buffer, 0), format!("{prefix}abcdef"));
            assert_eq!(
                line(&buffer, 1),
                format!("{}ghijkl", " ".repeat(gutter as usize))
            );
            assert_eq!(
                line(&buffer, 2),
                format!("{}mnopqr", " ".repeat(gutter as usize))
            );
            for y in 0..3 {
                for x in 0..gutter - 1 {
                    assert_eq!(buffer[(x, y)].bg, buffer[(0, 0)].bg);
                    assert_eq!(buffer[(x, y)].fg, buffer[(0, 0)].fg);
                }
                assert_ne!(buffer[(0, y)].bg, buffer[(gutter, y)].bg);
                assert_ne!(buffer[(0, y)].fg, buffer[(gutter, y)].fg);
                assert_eq!(buffer[(gutter - 2, y)].symbol(), " ");
                assert_eq!(buffer[(gutter - 2, y)].bg, buffer[(0, y)].bg);
                assert_eq!(buffer[(gutter - 1, y)].symbol(), " ");
                assert_eq!(buffer[(gutter - 1, y)].bg, buffer[(gutter, y)].bg);
            }
            assert_eq!(
                select(&mut view, (gutter, 0), (gutter + 6, 2)).as_deref(),
                Some("abcdefghijklmnopqr")
            );
            assert_eq!(
                select(&mut view, (0, 1), (gutter + 6, 1)).as_deref(),
                Some("ghijkl")
            );
        }
    }

    #[test]
    fn wrapped_diff_gutter_widens_with_numbers_and_shrinks_in_tiny_panes() {
        let patch = "@@ -12345 +12345 @@\n-界界界界界界\n+short\n";
        let mut view = CodeView::new();
        view.set_document(diff_document("example.txt", patch));
        let gutter = view.document().rows[0].gutter.len() as u16;
        view.set_wrapped(true);
        let buffer = draw(&mut view, gutter + 6, 6);
        assert!(line(&buffer, 0).contains("12345"));
        assert_eq!(
            line(&buffer, 1),
            format!("{}界界界", " ".repeat(gutter as usize))
        );
        for width in 1..=gutter {
            draw(&mut view, width, 6);
        }
        let buffer = draw(&mut view, gutter + 12, 6);
        assert!(line(&buffer, 0).ends_with("界界界界界界"));
        assert!(line(&buffer, 1).contains("short"));
    }

    #[test]
    fn wrapped_resize_preserves_visible_source_and_selection() {
        let mut view = CodeView::new();
        let (xs, ys) = ("x".repeat(50), "y".repeat(50));
        view.set_document(doc(&["short", &xs, "target", &ys]));
        view.set_wrapped(true);
        draw(&mut view, 16, 4);
        view.scroll_to(0, view.source_to_visual_row(1) + 2);
        let buffer = draw(&mut view, 16, 4);
        let target = (0..4)
            .find(|&y| line(&buffer, y).starts_with("target"))
            .unwrap();
        assert_eq!(
            select(&mut view, (0, target), (3, target)).as_deref(),
            Some("tar")
        );
        let top_before = line(&buffer, 0).trim().to_string();
        assert!(top_before.chars().all(|c| c == 'x'));

        let buffer = draw(&mut view, 10, 4);
        let top_after = line(&buffer, 0).trim().to_string();
        assert!(!top_after.is_empty() && top_after.chars().all(|c| c == 'x'));
        assert!(top_after.len() < top_before.len());
        assert_eq!(view.selected_text().as_deref(), Some("tar"));
    }

    #[test]
    fn wrapped_redraw_at_the_same_size_keeps_the_scroll_position() {
        let mut view = CodeView::new();
        let xs = "x".repeat(50);
        let rows: Vec<&str> = std::iter::repeat_n(xs.as_str(), 10).collect();
        view.set_document(doc(&rows));
        view.set_wrapped(true);
        draw(&mut view, 16, 4);
        let mid = view.source_to_visual_row(3) + 2;
        view.scroll_to(0, mid);
        let before = draw(&mut view, 16, 4);
        assert!(
            view.viewport().width < 16,
            "the vertical scrollbar is visible"
        );
        for _ in 0..3 {
            let after = draw(&mut view, 16, 4);
            assert_eq!(view.scroll_offset().1, mid);
            assert_eq!(lines(&after, 16, 4), lines(&before, 16, 4));
        }
    }

    #[test]
    fn wrapped_resize_keeps_the_top_source_offset() {
        let mut view = CodeView::new();
        let text: String = (0..60).map(|i| char::from(b'a' + (i % 26) as u8)).collect();
        let rows: Vec<&str> = std::iter::repeat_n(text.as_str(), 10).collect();
        view.set_document(doc(&rows));
        view.set_wrapped(true);
        draw(&mut view, 16, 4);
        view.scroll_to(0, view.source_to_visual_row(2) + 2);
        let buffer = draw(&mut view, 16, 4);
        let top = line(&buffer, 0);
        let offset = text.find(top.trim_end()).unwrap();

        let buffer = draw(&mut view, 11, 4);
        let top = line(&buffer, 0);
        let shown = top.trim_end();
        let start = text.find(shown).unwrap();
        assert!(start <= offset && offset < start + shown.len());
    }

    #[test]
    fn empty_areas_render_nothing_and_do_not_panic() {
        let mut view = CodeView::new();
        view.set_document(numbered(30, &"x".repeat(40)));
        let mut buffer = Buffer::empty(Rect::new(0, 0, 10, 10));
        view.render(Rect::new(2, 2, 0, 5), &mut buffer);
        view.render(Rect::new(2, 2, 5, 0), &mut buffer);
        view.set_wrapped(true);
        view.render(Rect::new(2, 2, 0, 5), &mut buffer);
        assert_eq!(buffer, Buffer::empty(Rect::new(0, 0, 10, 10)));
    }

    #[test]
    fn wide_character_cut_at_the_right_edge_is_blank() {
        let mut view = CodeView::new();
        view.set_document(doc(&["ab界"]));
        let buffer = draw(&mut view, 3, 2);
        assert_eq!(line(&buffer, 0), "ab ");
    }
}
