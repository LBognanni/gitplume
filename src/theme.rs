use ratatui::style::{Color, Modifier, Style};

pub const CANVAS: Color = Color::from_u32(0x0d1117);
pub const SURFACE: Color = Color::from_u32(0x161b22);
pub const RAISED_SURFACE: Color = Color::from_u32(0x21262d);
pub const INACTIVE_SELECTION: Color = Color::from_u32(0x30363d);
pub const TEXT: Color = Color::from_u32(0xf0f3f6);
pub const MUTED_TEXT: Color = Color::from_u32(0xb1bac4);
pub const BORDER: Color = Color::from_u32(0x6e7681);
pub const ACCENT: Color = Color::from_u32(0x58a6ff);
pub const FOCUS: Color = Color::from_u32(0xf2cc60);
pub const FOCUSED_SELECTION: Color = Color::from_u32(0x174ea6);
pub const ADDITION_BACKGROUND: Color = Color::from_u32(0x142b1d);
pub const REMOVAL_BACKGROUND: Color = Color::from_u32(0x351b20);
pub const DIFF_BACKGROUND: Color = Color::from_u32(0x272822);
pub const DANGER: Color = Color::from_u32(0xff7b72);
pub const ADDITION: Color = Color::from_u32(0x7ee787);
pub const PRIMARY: Color = Color::from_u32(0x0178d4);

/// Panel titles, viewer titles, and the status bar.
pub fn title() -> Style {
    Style::new()
        .fg(MUTED_TEXT)
        .bg(RAISED_SURFACE)
        .add_modifier(Modifier::BOLD)
}

pub const SCROLLBAR: Color = ACCENT;
pub const SCROLLBAR_BACKGROUND: Color = CANVAS;
