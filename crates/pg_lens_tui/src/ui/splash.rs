//! Full-screen connection splash, shown INSTEAD of the dashboard while no
//! real data has ever arrived (see [`App::show_splash`]).
//!
//! Before the first Ok snapshot the dashboard would render empty/zero
//! chrome — it looks broken. This screen renders instead: a centered
//! wordmark, a braille spinner driven by `App::tick_count` (advanced on
//! `Action::Tick`, 250ms), the `connecting to user@host …` line, and — when
//! the poller reports an error while still pre-first-data — a bordered,
//! word-wrapped error box with a "retrying automatically" hint (the poller
//! reconnects with backoff on its own; `q`/`Esc` quit as always).
//!
//! Once the first Ok snapshot lands the dashboard takes over permanently:
//! later disconnects keep the classic banner-over-last-data behavior.
//!
//! [`App::show_splash`]: crate::app::App::show_splash

use pg_lens_core::PollerStatus;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Wrap},
};

use crate::app::App;
use crate::ui::style;

/// Braille spinner frames, advanced once per `Action::Tick` (250ms).
pub const SPINNER_FRAMES: [&str; 10] = [
    "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}",
    "\u{2834}", "\u{2826}", "\u{2827}", "\u{2807}", "\u{280f}",
];

/// The spinner frame for the current tick (pure: `tick_count → glyph`).
pub fn spinner_frame(tick_count: u64) -> &'static str {
    SPINNER_FRAMES[(tick_count % SPINNER_FRAMES.len() as u64) as usize]
}

pub fn draw(app: &App, frame: &mut Frame) {
    let area = frame.area();
    let error = match &app.snapshot.status {
        PollerStatus::Error(msg) => Some(msg.as_str()),
        _ => None,
    };

    // Wordmark(3) + blank + spinner line + status line.
    const HEADER_H: u16 = 6;
    let box_width = area.width.saturating_sub(8).clamp(20, 72);
    // Error box height: enough rows to word-wrap the message (capped), plus
    // borders, plus a blank spacer and the dim hint line underneath.
    let error_h = error.map_or(0, |msg| {
        let inner = usize::from(box_width.saturating_sub(2)).max(1);
        let rows = msg.chars().count().div_ceil(inner).clamp(1, 8) as u16;
        1 + (rows + 2) + 1
    });

    let [_, content_area, _] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(HEADER_H + error_h),
        Constraint::Min(0),
    ])
    .areas(area);
    let [header_area, error_area] =
        Layout::vertical([Constraint::Length(HEADER_H), Constraint::Length(error_h)])
            .areas(content_area);

    let status_line = match &app.snapshot.status {
        PollerStatus::Error(_) => Line::from("connection failed \u{2014} retrying with backoff")
            .style(Style::new().fg(Color::Red)),
        _ => Line::from("waiting for the first snapshot").style(style::label_style()),
    };
    let header = Paragraph::new(vec![
        // Small hand-made wordmark: a lens over the name. Tasteful, 2 lines.
        Line::from(Span::styled("\u{25cd}  p g _ l e n s", style::accent_style())),
        Line::from("live PostgreSQL observability").style(style::label_style()),
        Line::default(),
        Line::from(vec![
            Span::styled(spinner_frame(app.tick_count), style::accent_style()),
            Span::raw(" connecting to "),
            Span::styled(app.host.clone(), style::value_style()),
            Span::raw(" \u{2026}"),
        ]),
        status_line,
    ])
    .alignment(Alignment::Center);
    frame.render_widget(header, header_area);

    if let Some(msg) = error {
        draw_error_box(msg, frame, error_area, box_width);
    }
}

/// Centered bordered box with the word-wrapped poller error (password_cmd
/// stderr can be long — it must wrap, never overflow), hint line below.
fn draw_error_box(msg: &str, frame: &mut Frame, area: Rect, box_width: u16) {
    let [_, box_area, hint_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);
    let [_, centered, _] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(box_width),
        Constraint::Min(0),
    ])
    .areas(box_area);

    let error_box = Paragraph::new(msg.to_string())
        .wrap(Wrap { trim: false })
        .block(
            Block::bordered()
                .title(" connection error ")
                .border_style(Style::new().fg(Color::Red)),
        );
    frame.render_widget(error_box, centered);

    let hint = Paragraph::new(
        Line::from("retrying automatically \u{b7} q/Esc: quit").style(style::label_style()),
    )
    .alignment(Alignment::Center);
    frame.render_widget(hint, hint_area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_cycles_through_all_frames_and_wraps() {
        let seen: Vec<&str> = (0..10).map(spinner_frame).collect();
        assert_eq!(seen.len(), 10);
        assert_eq!(spinner_frame(0), spinner_frame(10), "wraps at 10");
        assert_ne!(spinner_frame(0), spinner_frame(1), "advances per tick");
        // All ten frames are distinct glyphs.
        let mut unique = seen.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), 10);
    }
}
