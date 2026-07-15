//! Admin-action confirmation modal (Micro Lens: `c` cancel / `K` terminate).
//!
//! A small centered bordered box drawn over whatever is on screen. The
//! terminate variant is red and spells out that the connection dies — the
//! cancel variant only stops the current query. Rendering only: the y/n/Esc
//! semantics live in `crate::app::handle_confirm_key`.

use pg_lens_core::AdminKind;
use ratatui::{
    Frame,
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
};

use crate::app::App;
use crate::ui::style;

const WIDTH: u16 = 62;
const HEIGHT: u16 = 5;

pub fn draw(app: &App, frame: &mut Frame) {
    let Some(confirm) = app.confirm.as_ref() else {
        return;
    };
    let area = centered(frame.area(), WIDTH, HEIGHT);

    let pid = confirm.command.pid();
    let target = format!("{}@{}", confirm.username, confirm.database);
    let (title, border, question, warning) = match confirm.command.kind() {
        AdminKind::Cancel => (
            " Cancel query ",
            style::accent_style(),
            format!("Cancel query on PID {pid} ({target})?"),
            None,
        ),
        AdminKind::Terminate => (
            " Terminate backend ",
            Style::new().fg(Color::Red).bold(),
            format!("Terminate backend PID {pid} ({target})?"),
            Some("The connection will be killed."),
        ),
    };

    let mut lines = vec![Line::from(question).centered().bold()];
    if let Some(warning) = warning {
        lines.push(
            Line::from(warning)
                .centered()
                .style(Style::new().fg(Color::Red).bold()),
        );
    }
    let [yk, yd] = style::hint("y", ": confirm");
    let [nk, nd] = style::hint("n/Esc", ": abort");
    lines.push(
        Line::from(vec![
            yk,
            yd,
            Span::styled(" \u{b7} ", style::label_style()),
            nk,
            nd,
        ])
        .centered(),
    );

    let panel = Paragraph::new(lines).block(
        Block::bordered()
            .title(title)
            .title_style(border)
            .border_style(border),
    );
    frame.render_widget(Clear, area);
    frame.render_widget(panel, area);
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
        let rect = centered(area, WIDTH, HEIGHT);
        assert!(rect.x > 0 && rect.y > 0);
        assert_eq!(rect.width, WIDTH);
        assert_eq!(rect.height, HEIGHT);

        // Tiny terminal: clamped, never panics or overflows.
        let tiny = Rect::new(0, 0, 30, 3);
        let rect = centered(tiny, WIDTH, HEIGHT);
        assert!(rect.width <= 30 && rect.height <= 3);
    }
}
