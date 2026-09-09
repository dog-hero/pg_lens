//! In-session server / cluster picker overlay (`C` — any lens, no other overlay open).
//!
//! Allows dynamic runtime switching between servers / clusters defined in
//! `services.toml`. Enter tells the poller to disconnect from the current
//! server, reset stats and ring buffers, and connect to the target server.

use ratatui::{
    Frame,
    layout::{Constraint, Flex, Layout, Rect},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
};

use crate::app::App;
use crate::ui::style;

pub fn draw(app: &App, frame: &mut Frame) {
    let Some(picker) = &app.server_picker else {
        return;
    };
    let area = frame.area();

    let name_width = picker
        .entries
        .iter()
        .map(|e| e.name.chars().count())
        .max()
        .unwrap_or(0);
    let target_width = picker
        .entries
        .iter()
        .map(|e| format_target(e).chars().count())
        .max()
        .unwrap_or(0);

    let title = " select a server / cluster ";
    let current_suffix = " (current)";
    let hint = hint_line();
    let content_width = picker
        .entries
        .iter()
        .map(|e| {
            let is_current = app.snapshot.vitals.server_name.as_deref() == Some(&e.name);
            let current = usize::from(is_current) * current_suffix.len();
            2 + name_width + 2 + target_width + current
        })
        .max()
        .unwrap_or(0)
        .max(title.chars().count())
        .max(hint.width()) as u16;
    let width = (content_width + 6).min(area.width.saturating_sub(4).max(24));

    let mut lines = entry_lines(app, picker, name_width, target_width);
    lines.push(Line::default());
    lines.push(hint);
    let height = (lines.len() as u16 + 2).min(area.height.saturating_sub(2).max(5));

    let rect = centered(area, width, height);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title(title).border_style(style::accent_style())),
        rect,
    );
}

fn format_target(entry: &pg_lens_core::settings::ServiceSummary) -> String {
    let host = entry.host.as_deref().unwrap_or("localhost");
    let port = entry.port.unwrap_or(5432);
    let user = entry.user.as_deref().unwrap_or("-");
    let db = entry.dbname.as_deref().unwrap_or("-");
    format!("{user}@{host}:{port}/{db}")
}

fn entry_lines(
    app: &App,
    picker: &crate::app::ServerPickerState,
    name_width: usize,
    target_width: usize,
) -> Vec<Line<'static>> {
    picker
        .entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let selected = i == picker.selected;
            let is_current = app.snapshot.vitals.server_name.as_deref() == Some(&entry.name);
            let marker = if selected { "\u{25b8} " } else { "  " };
            let name = format!("{marker}{name:<name_width$}", name = entry.name);
            let name_style = if selected {
                style::accent_style()
            } else {
                style::value_style()
            };
            let target = format!("{:>target_width$}", format_target(entry));
            let current = if is_current { " (current)" } else { "" };
            Line::from(vec![
                Span::raw(" "),
                Span::styled(name, name_style),
                Span::styled("  ", style::label_style()),
                Span::styled(target, style::label_style()),
                Span::styled(current, style::label_style()),
            ])
        })
        .collect()
}

fn hint_line() -> Line<'static> {
    let sep = Span::styled(" \u{b7} ", style::label_style());
    let mut spans = Vec::new();
    for (i, (key, desc)) in [("j/k", ": move"), ("Enter", ": connect"), ("Esc", ": close")]
        .into_iter()
        .enumerate()
    {
        if i > 0 {
            spans.push(sep.clone());
        }
        let [k, d] = style::hint(key, desc);
        spans.push(k);
        spans.push(d);
    }
    Line::from(spans).centered()
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let [h] = Layout::horizontal([Constraint::Length(width.min(area.width))])
        .flex(Flex::Center)
        .areas(area);
    let [rect] = Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Center)
        .areas(h);
    rect
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centered_rect_is_inside_and_clamped() {
        let area = Rect::new(0, 0, 120, 36);
        let rect = centered(area, 50, 10);
        assert!(rect.x > 0 && rect.y > 0);
        assert_eq!(rect.width, 50);
        assert_eq!(rect.height, 10);

        let tiny = Rect::new(0, 0, 20, 4);
        let rect = centered(tiny, 50, 10);
        assert!(rect.width <= 20 && rect.height <= 4);
    }
}

