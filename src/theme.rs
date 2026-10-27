use ratatui::style::Color;

pub const RAISED: Color = Color::Rgb(0x1b, 0x22, 0x27);
pub const LINE: Color = Color::Rgb(0x29, 0x30, 0x36);
pub const INK: Color = Color::Rgb(0xf2, 0xf4, 0xf3);
pub const MUTED: Color = Color::Rgb(0xa6, 0xb0, 0xb3);
pub const SILVER: Color = Color::Rgb(0xc4, 0xcd, 0xd0);
pub const ACCENT: Color = Color::Rgb(0x6f, 0x92, 0xff);
pub const ACCENT_SOFT: Color = Color::Rgb(0xc7, 0xd4, 0xff);

/// Makes archived or native text safe to render without altering its stored source.
pub fn sanitize_terminal_text(text: &str) -> String {
    text.chars()
        .map(|ch| match ch {
            '\n' | '\t' => ch,
            ch if ch.is_control() => '�',
            ch => ch,
        })
        .collect()
}
