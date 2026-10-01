use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Widget};

use crate::app::{
    Action, App, Button, FileNode, Focus, MAX_FILE_JUMP_RESULTS, MAX_FIND_RESULTS, MIN_FIND_QUERY,
    Modal, Severity, Tab, Target, TreeRow,
};
use crate::code_view::{CodeView, Scrollbar, render_scrollbar, scrollbar_layout};
use crate::icons;
use crate::layout::{Group, Splitter};
use crate::model::{Commit, Side};
use crate::theme;
use unicode_width::UnicodeWidthStr;

const TOAST_WIDTH: u16 = 50;
const SHORTCUTS_WIDTH: u16 = 60;
const DISCARD_WIDTH: u16 = 60;
const JUMP_WIDTH: u16 = 70;

pub const SHORTCUTS: &str = "Mouse controls are supported throughout.

Navigation
  j / k       Move selection or scroll
  Enter       Open the selected item
  1 / 2 / 3   Changes / Files / Find tab
  n / p       Next / previous diff change
  t           Jump to a file (Files tab)
  Ctrl+F      Find in files

File actions
  Space       Check or uncheck a file
  s           Stage or unstage the focused file
  d           Discard the focused unstaged file

Application
  r           Refresh repository views
  w           Toggle line wrapping
  h           Show or close this help
  q           Quit";

pub fn render(app: &mut App, frame: &mut Frame) {
    app.hits.clear();
    let area = frame.area();
    let buf = frame.buffer_mut();
    buf.set_style(area, Style::new().fg(theme::TEXT).bg(theme::CANVAS));
    let [tabs, body, status] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);

    render_tabs(app, tabs, buf);
    match app.tab {
        Tab::Changes => render_changes(app, body, buf),
        Tab::Files => render_files(app, body, buf),
        Tab::Find => render_find(app, body, buf),
    }
    let text = match app.hint() {
        Some(hint) => hint.to_string(),
        None => format!("Branch: {}", app.branch),
    };
    Paragraph::new(text)
        .style(theme::title())
        .block(Block::new().padding(Padding::horizontal(1)))
        .render(status, buf);

    if let Some(modal) = app.modal.clone() {
        dim(area, buf);
        app.hits.push((area, Target::Backdrop));
        match modal {
            Modal::Shortcuts => render_shortcuts(app, area, buf),
            Modal::Discard { entries, confirm } => {
                let untracked = entries.iter().any(|entry| entry.status == '?');
                render_discard(app, area, entries.len(), untracked, confirm, buf);
            }
            Modal::FileJump { query, selected } => {
                render_file_jump(app, area, &query, selected, buf)
            }
        }
    }
    render_toasts(
        app,
        Rect {
            height: status.y,
            ..area
        },
        buf,
    );
}

fn render_tabs(app: &mut App, area: Rect, buf: &mut Buffer) {
    buf.set_style(area, Style::new().fg(theme::MUTED_TEXT).bg(theme::SURFACE));
    let mut x = area.x;
    let mut active = (area.x, area.x);
    for (tab, label) in [
        (Tab::Changes, " Changes "),
        (Tab::Files, " Files "),
        (Tab::Find, " Find "),
    ] {
        let style = if app.tab == tab {
            Style::new()
                .fg(theme::TEXT)
                .bg(theme::RAISED_SURFACE)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme::MUTED_TEXT).bg(theme::SURFACE)
        };
        let width = (label.len() as u16).min(area.right().saturating_sub(x));
        let rect = Rect::new(x, area.y, width, 1);
        Span::styled(label, style).render(rect, buf);
        app.hits
            .push((Rect { height: 2, ..rect }, Target::Tab(tab)));
        if app.tab == tab {
            active = (x, x + width);
        }
        x += width;
    }
    // The bar is dim outside the active tab, with half bars at its edges.
    if area.height < 2 {
        return;
    }
    let y = area.y + 1;
    let dim = Style::new().fg(theme::INACTIVE_SELECTION);
    for x in area.x..area.right() {
        let (symbol, style) = if (active.0..active.1).contains(&x) {
            ("━", Style::new().fg(theme::PRIMARY))
        } else if x + 1 == active.0 {
            ("╸", dim)
        } else if x == active.1 {
            ("╺", dim)
        } else {
            ("━", dim)
        };
        buf[(x, y)].set_symbol(symbol).set_style(style);
    }
}

/// Split `area` along one axis into the group's panes with one-cell splitters between.
fn split(group: &mut Group, area: Rect, vertical: bool) -> Vec<Rect> {
    let sizes = group.layout(if vertical { area.height } else { area.width });
    let mut constraints = Vec::new();
    for (index, size) in sizes.into_iter().enumerate() {
        if index > 0 {
            constraints.push(Constraint::Length(1));
        }
        constraints.push(Constraint::Length(size));
    }
    let layout = if vertical {
        Layout::vertical(constraints)
    } else {
        Layout::horizontal(constraints)
    };
    layout.split(area).to_vec()
}

fn split_columns(group: &mut Group, area: Rect) -> [Rect; 3] {
    split(group, area, false)
        .try_into()
        .expect("two panes and a splitter")
}

/// Draw a splitter: `│` down a vertical one, `─` across a horizontal one.
fn splitter(app: &mut App, splitter: Splitter, area: Rect, buf: &mut Buffer) {
    let active = app.hover == Some(Target::Splitter(splitter))
        || app.drag.is_some_and(|drag| drag.splitter == splitter);
    let (symbol, color) = (
        match splitter {
            Splitter::Section(_) => "─",
            Splitter::Sidebar | Splitter::Files => "│",
        },
        if active { theme::ACCENT } else { theme::BORDER },
    );
    // Section splitters sit inside the sidebar, on its surface.
    let background = match splitter {
        Splitter::Section(_) => theme::SURFACE,
        Splitter::Sidebar | Splitter::Files => theme::CANVAS,
    };
    for position in area.positions() {
        buf[position]
            .set_symbol(symbol)
            .set_fg(color)
            .set_bg(background);
    }
    app.hits.push((area, Target::Splitter(splitter)));
}

fn render_changes(app: &mut App, area: Rect, buf: &mut Buffer) {
    let [sidebar, divider, pane] = split_columns(&mut app.panes.changes, area);
    splitter(app, Splitter::Sidebar, divider, buf);
    let [staged, split1, unstaged, split2, commits] = split(&mut app.panes.sections, sidebar, true)
        .try_into()
        .expect("three sections and two splitters");
    splitter(app, Splitter::Section(0), split1, buf);
    splitter(app, Splitter::Section(1), split2, buf);

    status_section(app, staged, Side::Staged, buf);
    status_section(app, unstaged, Side::Unstaged, buf);
    let [title, list] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(commits);
    viewer_title(app, title, "Commits", &[], buf);
    let loading = app.tree_loading();
    bordered_list(loading.then(loading_line).into_iter().collect(), list, buf);
    app.hits.push((list, Target::Pane(Focus::Commits)));
    if !loading {
        commit_rows(app, list.inner(ratatui::layout::Margin::new(1, 1)), buf);
    }

    let [title, view] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(pane);
    viewer_title(
        app,
        title,
        &app.diff_title.clone(),
        &[Button::PreviousChange, Button::NextChange],
        buf,
    );
    if app.diff_loading {
        Paragraph::new(loading_line())
            .style(Style::new().fg(theme::TEXT).bg(theme::DIFF_BACKGROUND))
            .render(view, buf);
    } else {
        render_view(&mut app.diff_view, view, buf);
    }
    app.hits.push((view, Target::Pane(Focus::Diff)));
}

fn render_files(app: &mut App, area: Rect, buf: &mut Buffer) {
    let [tree, divider, pane] = split_columns(&mut app.panes.files, area);
    splitter(app, Splitter::Files, divider, buf);
    let loading = app.files_tree_loading;
    bordered_list(loading.then(loading_line).into_iter().collect(), tree, buf);
    app.hits.push((tree, Target::Pane(Focus::FilesTree)));
    if !loading {
        file_rows(app, tree.inner(ratatui::layout::Margin::new(1, 1)), buf);
    }
    render_preview(app, pane, buf);
}

/// The preview pane, shared by the Files and Find tabs.
fn render_preview(app: &mut App, pane: Rect, buf: &mut Buffer) {
    let [title, view] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(pane);
    viewer_title(app, title, &app.preview_title.clone(), &[], buf);
    if app.preview_loading {
        Paragraph::new(loading_line())
            .style(Style::new().fg(theme::TEXT).bg(theme::DIFF_BACKGROUND))
            .render(view, buf);
    } else {
        render_view(&mut app.preview_view, view, buf);
    }
    app.hits.push((view, Target::Pane(Focus::Preview)));
}

/// The Find tab: a Find box, a Filter box, and the scrollable match list,
/// sharing the Files tab's sidebar width.
fn render_find(app: &mut App, area: Rect, buf: &mut Buffer) {
    let [sidebar, divider, pane] = split_columns(&mut app.panes.files, area);
    splitter(app, Splitter::Files, divider, buf);

    let truncated = app.find.truncated;
    let [query_area, filter_area, results_area, note_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Fill(1),
        Constraint::Length(truncated as u16),
    ])
    .areas(sidebar);

    text_input(
        app,
        query_area,
        Focus::FindQuery,
        &app.find.query.clone(),
        "Find",
        buf,
    );
    text_input(
        app,
        filter_area,
        Focus::FindFilter,
        &app.find.filter.clone(),
        "Filter, e.g. *.rs, src/**",
        buf,
    );

    let message = if app.find.query.chars().count() < MIN_FIND_QUERY {
        // Keep this in sync with `MIN_FIND_QUERY`, currently 3.
        Some("Type at least 3 characters")
    } else if app.find.loading {
        Some("Searching…")
    } else if app.find.results.is_empty() {
        Some("No matches")
    } else {
        None
    };
    bordered_list(
        message
            .map(|m| Line::styled(m, Style::new().add_modifier(Modifier::DIM)))
            .into_iter()
            .collect(),
        results_area,
        buf,
    );
    app.hits
        .push((results_area, Target::Pane(Focus::FindResults)));
    if message.is_none() {
        find_rows(
            app,
            results_area.inner(ratatui::layout::Margin::new(1, 1)),
            buf,
        );
    }
    if truncated {
        Paragraph::new(format!("Showing first {MAX_FIND_RESULTS} matches"))
            .style(Style::new().fg(theme::MUTED_TEXT))
            .render(note_area, buf);
    }

    render_preview(app, pane, buf);
}

/// A bordered single-line text input: a block cursor while focused, a dim
/// placeholder while empty.
fn text_input(
    app: &mut App,
    area: Rect,
    focus: Focus,
    text: &str,
    placeholder: &str,
    buf: &mut Buffer,
) {
    let focused = app.focus == focus;
    let border_color = if focused {
        theme::ACCENT
    } else {
        theme::BORDER
    };
    Block::bordered()
        .border_style(Style::new().fg(border_color))
        .style(Style::new().fg(theme::TEXT).bg(theme::SURFACE))
        .render(area, buf);
    app.hits.push((area, Target::Pane(focus)));
    if area.height < 2 {
        return;
    }
    let line = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: 1,
    };
    if text.is_empty() {
        Span::styled(
            placeholder,
            Style::new()
                .fg(theme::MUTED_TEXT)
                .add_modifier(Modifier::DIM),
        )
        .render(line, buf);
    } else {
        Span::raw(text.to_string()).render(line, buf);
    }
    if focused && line.width > 0 {
        let x = line.x + (text.chars().count() as u16).min(line.width.saturating_sub(1));
        buf[(x, line.y)].set_style(Style::new().add_modifier(Modifier::REVERSED));
    }
}

/// Draw the visible rows of the find-in-files result list.
fn find_rows(app: &mut App, area: Rect, buf: &mut Buffer) {
    let lines: Vec<Line<'static>> = app
        .find
        .results
        .iter()
        .map(|found| {
            Line::from(vec![
                Span::styled(
                    format!("{}:{} ", found.path, found.line),
                    Style::new().fg(theme::MUTED_TEXT),
                ),
                Span::raw(found.text.clone()),
            ])
        })
        .collect();
    let cursor = app.find.cursor;
    let focused = app.focus == Focus::FindResults;
    let find = &mut app.find;
    let hits = tree_rows(
        lines,
        cursor,
        focused,
        &mut find.offset,
        &mut find.scroll_x,
        &mut find.followed,
        area,
        buf,
    );
    app.hits
        .extend(hits.into_iter().map(|(rect, row)| match row {
            Ok(row) => (rect, Target::FindResult(row)),
            Err(bar) => (rect, Target::Scrollbar(Focus::FindResults, bar)),
        }));
}

fn loading_line() -> Line<'static> {
    Line::styled("Loading…", Style::new().add_modifier(Modifier::DIM))
}

/// A status section: title bar with bulk actions, then the bordered list.
fn status_section(app: &mut App, area: Rect, side: Side, buf: &mut Buffer) {
    let (title, focus, bulk): (_, _, &[Button]) = match side {
        Side::Staged => ("Staged", Focus::Staged, &[Button::Bulk(Action::Unstage)]),
        Side::Unstaged => (
            "Unstaged",
            Focus::Unstaged,
            &[Button::Bulk(Action::Stage), Button::Bulk(Action::Discard)],
        ),
    };
    let [title_row, list] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
    let buttons = if app.list(side).any_checked() {
        bulk
    } else {
        &[]
    };
    viewer_title(app, title_row, title, buttons, buf);
    let focused = app.focus == focus;
    let loading = app.status_loading;
    bordered_list(loading.then(loading_line).into_iter().collect(), list, buf);
    app.hits.push((list, Target::Pane(focus)));
    if !loading {
        status_rows(
            app,
            side,
            focused,
            list.inner(ratatui::layout::Margin::new(1, 1)),
            buf,
        );
    }
}

/// Draw the visible rows of a status list with their states and row actions.
fn status_rows(app: &mut App, side: Side, focused: bool, area: Rect, buf: &mut Buffer) {
    let list = match side {
        Side::Staged => &mut app.staged,
        Side::Unstaged => &mut app.unstaged,
    };
    let count = list.entries.len();
    let (area, vbar, _) = scrollbar_layout(area, count, 0);
    let height = area.height as usize;
    follow(
        &mut list.offset,
        &mut list.followed,
        list.highlight,
        count,
        height,
    );
    let offset = list.offset;
    let mut hits = Vec::new();
    if let Some(bar) = vbar {
        render_scrollbar(buf, bar, true, count, height, offset, None);
        let focus = match side {
            Side::Staged => Focus::Staged,
            Side::Unstaged => Focus::Unstaged,
        };
        let scrollbar = Scrollbar::new(bar, true, count, height);
        hits.push((bar, Target::Scrollbar(focus, scrollbar)));
    }
    let hover = app.hover;
    let list = app.list(side);
    for (index, entry) in list.entries.iter().enumerate().skip(offset).take(height) {
        let rect = Rect {
            y: area.y + (index - offset) as u16,
            height: 1,
            ..area
        };
        let highlighted = list.highlight == Some(index);
        let hovered = matches!(
            hover,
            Some(Target::Row(s, i) | Target::Checkbox(s, i) | Target::Button(Button::Row(s, i, _)))
                if s == side && i == index
        );
        let style = if highlighted && focused {
            Style::new()
                .fg(theme::TEXT)
                .bg(theme::FOCUSED_SELECTION)
                .add_modifier(Modifier::BOLD)
        } else if highlighted {
            Style::new().fg(theme::TEXT).bg(theme::INACTIVE_SELECTION)
        } else if hovered {
            Style::new().fg(theme::TEXT).bg(theme::RAISED_SURFACE)
        } else {
            Style::new().fg(theme::TEXT).bg(theme::SURFACE)
        };
        let label = match &entry.unsupported_reason {
            Some(reason) => format!("[!] {} {} - {reason}", entry.status, entry.path),
            None => {
                let mark = if list.checked[index] { 'x' } else { ' ' };
                format!("[{mark}] {} {}", entry.status, entry.path)
            }
        };
        buf.set_style(rect, style);
        Span::styled(label, style).render(rect, buf);
        hits.push((rect, Target::Row(side, index)));
        if entry.unsupported_reason.is_some() {
            continue;
        }
        hits.push((
            Rect {
                width: 3.min(rect.width),
                ..rect
            },
            Target::Checkbox(side, index),
        ));
        if !(hovered || highlighted && focused) {
            continue;
        }
        let actions: &[Action] = match side {
            Side::Staged => &[Action::Unstage],
            Side::Unstaged => &[Action::Stage, Action::Discard],
        };
        let mut x = rect
            .right()
            .saturating_sub(3 * actions.len() as u16)
            .max(rect.x);
        for &action in actions {
            let button = Button::Row(side, index, action);
            let cell = Rect {
                x,
                width: 3.min(rect.right() - x),
                ..rect
            };
            let hovered_button = hover == Some(Target::Button(button));
            Span::styled(glyph(button), button_style(button, hovered_button, style))
                .render(cell, buf);
            hits.push((cell, Target::Button(button)));
            x += cell.width;
        }
    }
    app.hits.extend(hits);
}

/// A commit subject with gitmoji shortcodes expanded, then the short hash in dim text.
pub fn format_commit_label(commit: &Commit) -> Line<'static> {
    Line::from(vec![
        Span::raw(format!("{} ", expand_gitmoji(&commit.subject))),
        Span::styled(
            commit.short_hash.clone(),
            Style::new().add_modifier(Modifier::DIM),
        ),
    ])
}

/// Replace known `:shortcode:`s with their emoji; unknown ones stay as written.
fn expand_gitmoji(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find(':') {
        let after = &rest[start + 1..];
        let found = after
            .find(':')
            .and_then(|end| Some((end, emojis::get_by_shortcode(&after[..end])?)));
        match found {
            Some((end, emoji)) => {
                out.push_str(&rest[..start]);
                out.push_str(emoji.as_str());
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[..=start]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Draw the visible rows of the commit tree.
fn commit_rows(app: &mut App, area: Rect, buf: &mut Buffer) {
    let tree = &mut app.commits;
    let lines = tree
        .rows()
        .into_iter()
        .map(|row| match row {
            TreeRow::Commit(commit) => {
                let node = &tree.nodes[commit];
                let mut line = format_commit_label(&node.commit);
                let marker = if node.expanded { EXPANDED } else { COLLAPSED };
                line.spans.insert(0, Span::raw(marker));
                line
            }
            TreeRow::File(commit, file) => {
                let file = &tree.nodes[commit].files.as_ref().expect("loaded files")[file];
                Line::from(format!("    {} {}", file.status, file.path))
            }
            TreeRow::Empty(_) => Line::from("    (no changed files)"),
        })
        .collect();
    let cursor = tree.cursor;
    let focused = app.focus == Focus::Commits;
    let tree = &mut app.commits;
    let hits = tree_rows(
        lines,
        cursor,
        focused,
        &mut tree.offset,
        &mut tree.scroll_x,
        &mut tree.followed,
        area,
        buf,
    );
    app.hits
        .extend(hits.into_iter().map(|(rect, row)| match row {
            Ok(row) => (rect, Target::TreeRow(row)),
            Err(bar) => (rect, Target::Scrollbar(Focus::Commits, bar)),
        }));
}

/// Draw the visible rows of the Files tree.
fn file_rows(app: &mut App, area: Rect, buf: &mut Buffer) {
    let tree = &mut app.files;
    let rows = tree.rows();
    let guides = tree_guides(&tree.nodes);
    let lines = rows
        .iter()
        .map(|&index| {
            let node = &tree.nodes[index];
            let mut line = if node.dir {
                let mut line = icons::folder_label(&node.name, node.expanded);
                let marker = if node.expanded { EXPANDED } else { COLLAPSED };
                line.spans.insert(0, Span::raw(marker));
                line
            } else {
                icons::file_label(&node.name)
            };
            line.spans.insert(
                0,
                Span::styled(guides[index].clone(), Style::new().fg(theme::BORDER)),
            );
            line
        })
        .collect();
    let cursor = tree
        .cursor
        .and_then(|node| rows.iter().position(|&r| r == node));
    let focused = app.focus == Focus::FilesTree;
    let tree = &mut app.files;
    let hits = tree_rows(
        lines,
        cursor,
        focused,
        &mut tree.offset,
        &mut tree.scroll_x,
        &mut tree.followed,
        area,
        buf,
    );
    app.hits
        .extend(hits.into_iter().map(|(rect, row)| match row {
            Ok(row) => (rect, Target::FileRow(row)),
            Err(bar) => (rect, Target::Scrollbar(Focus::FilesTree, bar)),
        }));
}

/// Scroll `offset` so a `cursor` that moved since the last draw is visible,
/// then clamp it to `count` rows in a window of `height`.
fn follow(
    offset: &mut usize,
    followed: &mut Option<usize>,
    cursor: Option<usize>,
    count: usize,
    height: usize,
) {
    if cursor != *followed {
        if let Some(cursor) = cursor {
            *offset = (*offset)
                .min(cursor)
                .max((cursor + 1).saturating_sub(height));
        }
        *followed = cursor;
    }
    *offset = (*offset).min(count.saturating_sub(height));
}

/// Draw a tree's `lines` scrolled to (`scroll_x`, `offset`) with scrollbars as
/// needed, highlighting row `cursor` (bold only while `focused`); returns each
/// drawn row's rect and index, then each scrollbar's rect and geometry.
#[allow(clippy::too_many_arguments)]
fn tree_rows(
    lines: Vec<Line<'static>>,
    cursor: Option<usize>,
    focused: bool,
    offset: &mut usize,
    scroll_x: &mut usize,
    followed: &mut Option<usize>,
    area: Rect,
    buf: &mut Buffer,
) -> Vec<(Rect, Result<usize, Scrollbar>)> {
    let count = lines.len();
    let width = lines.iter().map(Line::width).max().unwrap_or(0);
    let (area, vbar, hbar) = scrollbar_layout(area, count, width);
    let (window_x, height) = (area.width as usize, area.height as usize);
    follow(offset, followed, cursor, count, height);
    *scroll_x = (*scroll_x).min(width.saturating_sub(window_x));
    let mut hits = Vec::new();
    for (index, line) in lines.into_iter().enumerate().skip(*offset).take(height) {
        let rect = Rect {
            y: area.y + (index - *offset) as u16,
            height: 1,
            ..area
        };
        let style = if cursor == Some(index) && focused {
            Style::new()
                .fg(theme::TEXT)
                .bg(theme::FOCUSED_SELECTION)
                .add_modifier(Modifier::BOLD)
        } else if cursor == Some(index) {
            Style::new().fg(theme::TEXT).bg(theme::INACTIVE_SELECTION)
        } else {
            Style::new().fg(theme::TEXT).bg(theme::SURFACE)
        };
        // Render the whole row off screen, then copy the visible window.
        let full = Rect::new(
            0,
            0,
            (width.max(*scroll_x + window_x)).min(u16::MAX as usize) as u16,
            1,
        );
        let mut row = Buffer::empty(full);
        line.style(style).render(full, &mut row);
        let start = (*scroll_x).min((full.width - rect.width) as usize) as u16;
        for dx in 0..rect.width {
            let mut cell = row[(start + dx, 0)].clone();
            // A wide glyph cut by either edge shows as a blank cell.
            // Ratatui resets a wide glyph's hidden cell, dropping its style,
            // so a left cut takes the glyph's cell instead.
            if dx == 0 && start > 0 && row[(start - 1, 0)].symbol().width() > 1 {
                cell = row[(start - 1, 0)].clone();
                cell.set_symbol(" ");
            } else if dx + 1 == rect.width && cell.symbol().width() > 1 {
                cell.set_symbol(" ");
            }
            buf[(rect.x + dx, rect.y)] = cell;
        }
        hits.push((rect, Ok(index)));
    }
    if let Some(bar) = vbar {
        render_scrollbar(buf, bar, true, count, height, *offset, None);
        hits.push((bar, Err(Scrollbar::new(bar, true, count, height))));
    }
    if let Some(bar) = hbar {
        render_scrollbar(
            buf,
            bar,
            false,
            width,
            window_x,
            *scroll_x,
            Some(theme::SURFACE),
        );
        hits.push((bar, Err(Scrollbar::new(bar, false, width, window_x))));
    }
    if let (Some(v), Some(h)) = (vbar, hbar) {
        buf[(v.x, h.y)].reset();
        buf[(v.x, h.y)].set_bg(theme::SCROLLBAR_BACKGROUND);
    }
    hits
}

const EXPANDED: &str = "▼ ";
const COLLAPSED: &str = "▶ ";

fn tree_guides(nodes: &[FileNode]) -> Vec<String> {
    // A node is last when no later sibling follows before its parent ends.
    let mut is_last = vec![false; nodes.len()];
    let mut later_sibling: Vec<bool> = Vec::new();
    for (index, node) in nodes.iter().enumerate().rev() {
        later_sibling.resize(node.depth + 1, false);
        is_last[index] = !later_sibling[node.depth];
        later_sibling[node.depth] = true;
    }
    let mut ancestors_last: Vec<bool> = Vec::new();
    nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            ancestors_last.truncate(node.depth);
            let mut guide: String = ancestors_last
                .iter()
                .skip(1)
                .map(|&last| if last { "   " } else { "│  " })
                .collect();
            if node.depth > 0 {
                guide.push_str(if is_last[index] { "└─ " } else { "├─ " });
            }
            ancestors_last.push(is_last[index]);
            guide
        })
        .collect()
}

/// The three-cell label of an icon button.
fn glyph(button: Button) -> &'static str {
    match button {
        Button::PreviousChange => " ↑ ",
        Button::NextChange => " ↓ ",
        Button::Row(_, _, action) | Button::Bulk(action) => match action {
            Action::Stage => " ↑ ",
            Action::Unstage => " ↓ ",
            Action::Discard => " ↶ ",
        },
    }
}

/// A button's style on `base`: raised and bold while hovered, danger for discard.
fn button_style(button: Button, hovered: bool, base: Style) -> Style {
    if !hovered {
        return base.fg(theme::TEXT);
    }
    let discard = matches!(
        button,
        Button::Row(_, _, Action::Discard) | Button::Bulk(Action::Discard)
    );
    Style::new()
        .fg(if discard { theme::DANGER } else { theme::TEXT })
        .bg(theme::RAISED_SURFACE)
        .add_modifier(Modifier::BOLD)
}

fn bordered_list(rows: Vec<Line<'static>>, area: Rect, buf: &mut Buffer) {
    Paragraph::new(rows)
        .style(Style::new().fg(theme::TEXT).bg(theme::SURFACE))
        .block(Block::bordered().border_style(Style::new().fg(theme::BORDER)))
        .render(area, buf);
}

/// A title row with `label` on the left and icon `buttons` right-aligned.
fn viewer_title(app: &mut App, area: Rect, label: &str, buttons: &[Button], buf: &mut Buffer) {
    buf.set_style(area, theme::title());
    let inner = area.inner(ratatui::layout::Margin::new(1, 0));
    let actions = (buttons.len() as u16 * 3).min(inner.width);
    let [text, mut rest] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(actions)]).areas(inner);
    Paragraph::new(label.to_string())
        .style(theme::title())
        .render(text, buf);
    for &button in buttons {
        let rect = Rect {
            width: 3.min(rest.width),
            ..rest
        };
        let style = if app.enabled(button) {
            button_style(
                button,
                app.hover == Some(Target::Button(button)),
                theme::title(),
            )
        } else {
            theme::title().add_modifier(Modifier::DIM)
        };
        Span::styled(glyph(button), style).render(rect, buf);
        app.hits.push((rect, Target::Button(button)));
        rest.x += rect.width;
        rest.width -= rect.width;
    }
}

fn render_view(view: &mut CodeView, area: Rect, buf: &mut Buffer) {
    view.render(area, buf);
    // CodeView leaves unstyled cells at the terminal default; give them the viewer colors.
    for position in area.positions() {
        let cell = &mut buf[position];
        if cell.fg == Color::Reset {
            cell.fg = theme::TEXT;
        }
        if cell.bg == Color::Reset {
            cell.bg = theme::DIFF_BACKGROUND;
        }
    }
}

/// Blend every cell in `area` 70% toward the canvas color.
fn dim(area: Rect, buf: &mut Buffer) {
    fn blend(color: Color, fallback: Color) -> Color {
        let (Color::Rgb(r, g, b), Color::Rgb(cr, cg, cb)) = (
            if color == Color::Reset {
                fallback
            } else {
                color
            },
            theme::CANVAS,
        ) else {
            return color;
        };
        let mix = |c: u8, k: u8| ((c as u16 * 3 + k as u16 * 7) / 10) as u8;
        Color::Rgb(mix(r, cr), mix(g, cg), mix(b, cb))
    }
    for position in area.positions() {
        let cell = &mut buf[position];
        cell.fg = blend(cell.fg, theme::TEXT);
        cell.bg = blend(cell.bg, theme::CANVAS);
    }
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

fn render_shortcuts(app: &mut App, area: Rect, buf: &mut Buffer) {
    let lines = SHORTCUTS.lines().count() as u16;
    // Border, padding, title, margins, and the Close button around the text.
    let height = lines + 10;
    let dialog = centered(area, SHORTCUTS_WIDTH.min(area.width * 9 / 10), height);
    Clear.render(dialog, buf);
    let block = Block::bordered()
        .border_style(Style::new().fg(theme::BORDER))
        .style(Style::new().fg(theme::TEXT).bg(theme::RAISED_SURFACE))
        .padding(Padding::new(2, 2, 1, 1));
    let inner = block.inner(dialog);
    block.render(dialog, buf);
    let [text, button] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(3)]).areas(inner);
    let mut content = vec![
        Line::styled(
            "Keyboard Shortcuts",
            Style::new().add_modifier(Modifier::BOLD),
        ),
        Line::default(),
    ];
    content.extend(SHORTCUTS.lines().map(Line::from));
    Paragraph::new(content).render(text, buf);
    let close = Rect {
        width: BUTTON_WIDTH.min(button.width),
        ..button
    };
    let hovered = app.hover == Some(Target::CloseShortcuts);
    dialog_button("Close", false, true, hovered, close, buf);
    app.hits.push((close, Target::CloseShortcuts));
}

fn render_discard(
    app: &mut App,
    area: Rect,
    count: usize,
    untracked: bool,
    confirm: bool,
    buf: &mut Buffer,
) {
    let noun = if count == 1 { "file" } else { "files" };
    let mut message = format!("Discard changes to {count} {noun}? This cannot be undone.");
    if untracked {
        message.push_str(" Untracked files will be permanently deleted.");
    }
    let width = DISCARD_WIDTH.min(area.width * 9 / 10);
    let text = wrap(&message, width.saturating_sub(6) as usize);
    // Border, padding, title, the message with its margins, and the buttons.
    let dialog = centered(area, width, text.len() as u16 + 10);
    Clear.render(dialog, buf);
    let block = Block::bordered()
        .border_style(Style::new().fg(theme::BORDER))
        .style(Style::new().fg(theme::TEXT).bg(theme::RAISED_SURFACE))
        .padding(Padding::new(2, 2, 1, 1));
    let inner = block.inner(dialog);
    block.render(dialog, buf);
    let [body, buttons] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(3)]).areas(inner);
    let mut lines = vec![
        Line::styled(
            "Discard Changes?",
            Style::new().add_modifier(Modifier::BOLD),
        ),
        Line::default(),
    ];
    lines.extend(text.into_iter().map(Line::from));
    Paragraph::new(lines).render(body, buf);
    // Right-aligned: Cancel, a gap, then Discard.
    let [_, cancel, _, discard] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(BUTTON_WIDTH),
        Constraint::Length(1),
        Constraint::Length(BUTTON_WIDTH),
    ])
    .areas(buttons);
    let hover = app.hover;
    let cancel_hovered = hover == Some(Target::CancelDiscard);
    let discard_hovered = hover == Some(Target::ConfirmDiscard);
    dialog_button("Cancel", false, !confirm, cancel_hovered, cancel, buf);
    dialog_button("Discard", true, confirm, discard_hovered, discard, buf);
    app.hits.push((cancel, Target::CancelDiscard));
    app.hits.push((discard, Target::ConfirmDiscard));
}

const BUTTON_WIDTH: u16 = 16;

fn dialog_button(
    label: &str,
    danger: bool,
    focused: bool,
    hovered: bool,
    area: Rect,
    buf: &mut Buffer,
) {
    let shade = |background: Color, amount: f32| match background {
        Color::Rgb(r, g, b) => {
            let mix = |c: u8| {
                let target = if amount > 0.0 { 255.0 } else { 0.0 };
                (c as f32 + (target - c as f32) * amount.abs()) as u8
            };
            Color::Rgb(mix(r), mix(g), mix(b))
        }
        other => other,
    };
    let (foreground, background) = match (danger, focused) {
        (true, false) => (theme::CANVAS, theme::DANGER),
        (true, true) => (theme::CANVAS, shade(theme::DANGER, 0.1)),
        (false, false) => (theme::TEXT, theme::SURFACE),
        (false, true) => (Color::Rgb(255, 255, 255), theme::PRIMARY),
    };
    // Half the focused danger step, so hover never mimics focus.
    let background = if hovered {
        shade(background, 0.05)
    } else {
        background
    };
    let style = Style::new().fg(foreground).bg(background);
    buf.set_style(area, style);
    let [top, middle, bottom] = Layout::vertical([Constraint::Length(1); 3]).areas(area);
    let edge = |symbol: &str, color: Color, row: Rect, buf: &mut Buffer| {
        if row.height == 0 {
            return;
        }
        for x in row.left()..row.right() {
            buf[(x, row.y)].set_symbol(symbol).set_fg(color);
        }
    };
    edge("▔", shade(background, 0.3), top, buf);
    edge("▁", shade(background, -0.3), bottom, buf);
    Line::styled(format!(" {label} "), style.add_modifier(Modifier::BOLD))
        .centered()
        .render(middle, buf);
}

/// The file jump dialog, three rows from the top: input, results, and a truncation note.
fn render_file_jump(
    app: &mut App,
    area: Rect,
    query: &str,
    selected: Option<usize>,
    buf: &mut Buffer,
) {
    let (matches, truncated) = app.jump_matches();
    let top = area.y + 3;
    let available = area.bottom().saturating_sub(top);
    let visible = (matches.len() as u16)
        .min(area.height.saturating_sub(7))
        .min(available.saturating_sub(3 + truncated as u16));
    let height = (visible + 3 + truncated as u16).min(available);
    let width = JUMP_WIDTH.min(area.width * 9 / 10);
    let dialog = Rect::new(area.x + (area.width - width) / 2, top, width, height);
    Clear.render(dialog, buf);
    buf.set_style(
        dialog,
        Style::new().fg(theme::TEXT).bg(theme::RAISED_SURFACE),
    );
    app.hits.push((dialog, Target::JumpDialog));
    let [input, results, note] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(visible),
        Constraint::Length(truncated as u16),
    ])
    .areas(dialog);
    buf.set_style(input, Style::new().fg(theme::TEXT).bg(theme::SURFACE));
    // Padding 1 1 0 1: the text sits on the second row, one cell in.
    let line = Rect {
        x: input.x + 1,
        y: input.y + 1,
        width: input.width.saturating_sub(2),
        height: 1,
    };
    let text = if query.is_empty() {
        Span::styled(
            "Jump to file",
            Style::new()
                .fg(theme::MUTED_TEXT)
                .add_modifier(Modifier::DIM),
        )
    } else {
        Span::raw(query.to_string())
    };
    text.render(line, buf);
    if selected.is_none() && input.height >= 2 && line.width > 0 {
        // The input has focus: show its block cursor after the text.
        let x = line.x + (query.chars().count() as u16).min(line.width.saturating_sub(1));
        buf[(x, line.y)].set_style(Style::new().add_modifier(Modifier::REVERSED));
    }
    buf.set_style(results, Style::new().fg(theme::TEXT).bg(theme::SURFACE));
    app.jump_rows = visible as usize;
    app.jump_offset = app
        .jump_offset
        .min(matches.len().saturating_sub(visible as usize));
    let offset = app.jump_offset;
    let (results, vbar, _) = scrollbar_layout(results, matches.len(), 0);
    if let Some(bar) = vbar {
        render_scrollbar(
            buf,
            bar,
            true,
            matches.len(),
            visible as usize,
            offset,
            None,
        );
        let scrollbar = Scrollbar::new(bar, true, matches.len(), visible as usize);
        app.hits.push((bar, Target::JumpScrollbar(scrollbar)));
    }
    for (row, path) in matches
        .iter()
        .enumerate()
        .skip(offset)
        .take(visible as usize)
    {
        let rect = Rect {
            y: results.y + (row - offset) as u16,
            height: 1,
            ..results
        };
        let style = if selected == Some(row) {
            Style::new().bg(theme::PRIMARY).add_modifier(Modifier::BOLD)
        } else {
            Style::new()
        };
        buf.set_style(rect, style);
        Span::styled(path.clone(), style).render(rect, buf);
        app.hits.push((rect, Target::JumpResult(row)));
    }
    Paragraph::new(format!("Showing first {MAX_FILE_JUMP_RESULTS} matches"))
        .style(Style::new().fg(theme::MUTED_TEXT))
        .render(note, buf);
}

/// Word-wrap `text` to `width` columns, splitting words longer than a line.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let mut word: Vec<char> = word.chars().collect();
            let used = line.chars().count();
            if used > 0 && used + 1 + word.len() > width {
                lines.push(std::mem::take(&mut line));
            }
            while word.len() > width {
                lines.push(word.drain(..width).collect());
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.extend(word);
        }
        lines.push(line);
    }
    lines
}

/// Stack toasts upward from the bottom-right of `area`, newest lowest.
fn render_toasts(app: &mut App, area: Rect, buf: &mut Buffer) {
    let width = TOAST_WIDTH.min(area.width);
    let mut bottom = area.bottom();
    for toast in app.toasts.iter().rev() {
        let color = match toast.severity {
            Severity::Error => theme::DANGER,
            Severity::Warning => theme::FOCUS,
            Severity::Information => theme::ACCENT,
        };
        let mut lines = Vec::new();
        if let Some(title) = &toast.title {
            lines.push(Line::styled(
                title.clone(),
                Style::new().fg(color).add_modifier(Modifier::BOLD),
            ));
        }
        lines.extend(
            wrap(&toast.body, width.saturating_sub(3) as usize)
                .into_iter()
                .map(Line::from),
        );
        let height = (lines.len() as u16).min(bottom.saturating_sub(area.y));
        if height == 0 {
            break;
        }
        let rect = Rect::new(area.right() - width, bottom - height, width, height);
        Clear.render(rect, buf);
        Paragraph::new(lines)
            .style(Style::new().fg(theme::TEXT).bg(theme::RAISED_SURFACE))
            .block(
                Block::new()
                    .borders(Borders::LEFT)
                    .border_type(BorderType::Thick)
                    .border_style(Style::new().fg(color))
                    .padding(Padding::horizontal(1)),
            )
            .render(rect, buf);
        app.hits.push((rect, Target::Toast(toast.id)));
        bottom = rect.y;
    }
}
