//! In-session database picker overlay (U2, `d` — any lens, no other overlay
//! open).
//!
//! Unlike the startup service picker (`ui/picker.rs`, a full-screen
//! pre-poller mode), this is a small centered overlay drawn OVER whatever is
//! already on screen — the same visual family as the admin confirm modal
//! (`ui/confirm.rs`): `Clear` + a bordered `Block`. PostgreSQL cannot switch
//! databases in-session, so Enter always means "ask the poller to
//! reconnect" (`crate::app::handle_db_picker_key`), never an in-place
//! update — the connecting splash/banner path takes it from there.

use ratatui::{
    Frame,
    layout::{Constraint, Flex, Layout, Rect},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
};

use crate::app::App;
use crate::ui::{format, style};

pub fn draw(app: &App, frame: &mut Frame) {
    let Some(picker) = &app.db_picker else {
        return;
    };
    let area = frame.area();

    let name_width = picker
        .entries
        .iter()
        .map(|e| e.name.chars().count())
        .max()
        .unwrap_or(0);
    let size_width = picker
        .entries
        .iter()
        .map(|e| size_text(e.size_bytes).chars().count())
        .max()
        .unwrap_or(0);

    let title = " select a database ";
    let current_suffix = " (current)";
    let hint = hint_line();
    let content_width = picker
        .entries
        .iter()
        .map(|e| {
            let current = usize::from(e.name == app.snapshot.vitals.database) * current_suffix.len();
            2 + name_width + 2 + size_width + current
        })
        .max()
        .unwrap_or(0)
        .max(title.chars().count())
        .max(hint.width()) as u16;
    let width = (content_width + 4).min(area.width.saturating_sub(4).max(20));

    let mut lines = entry_lines(app, picker, name_width, size_width);
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

fn size_text(size_bytes: Option<i64>) -> String {
    match size_bytes {
        Some(bytes) => format::human_bytes(bytes),
        None => "\u{2014}".to_string(),
    }
}

/// One line per database: `▸ name  size` for the selected row (accented
/// name), `  name  size` (dim value) otherwise, plus `(current)` on the
/// connected database regardless of selection.
fn entry_lines(
    app: &App,
    picker: &crate::app::DbPickerState,
    name_width: usize,
    size_width: usize,
) -> Vec<Line<'static>> {
    picker
        .entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let selected = i == picker.selected;
            let is_current = entry.name == app.snapshot.vitals.database;
            let marker = if selected { "\u{25b8} " } else { "  " };
            let name = format!("{marker}{name:<name_width$}", name = entry.name);
            let name_style = if selected {
                style::accent_style()
            } else {
                style::value_style()
            };
            let size = format!("{:>size_width$}", size_text(entry.size_bytes));
            let current = if is_current { " (current)" } else { "" };
            Line::from(vec![
                Span::raw(" "),
                Span::styled(name, name_style),
                Span::styled("  ", style::label_style()),
                Span::styled(size, style::label_style()),
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

/// A `width` x `height` rect centered in `area` (clamped to fit).
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
        let rect = centered(area, 40, 8);
        assert!(rect.x > 0 && rect.y > 0);
        assert_eq!(rect.width, 40);
        assert_eq!(rect.height, 8);

        let tiny = Rect::new(0, 0, 20, 4);
        let rect = centered(tiny, 40, 8);
        assert!(rect.width <= 20 && rect.height <= 4);
    }
}
