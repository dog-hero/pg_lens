//! Keyboard help overlay (`?`, v0.9) — a static, no-data modal listing
//! EVERY binding `app.rs::handle_key` (and its sub-keymaps) recognizes. It
//! is the single source of truth the README keybindings table is
//! reconciled against — keep it in sync with `handle_key` by hand; there is
//! no other mechanism enforcing it.
//!
//! Visual family: same `Clear` + bordered `Block`, centered-and-clamped
//! shape as `ui/confirm.rs`/`ui/db_picker.rs`, just wider and taller (a
//! two-column key/description reference, not a short prompt). On a small
//! terminal the box clamps to the frame and the tail of the list is
//! clipped rather than panicking — see `centered`.

use ratatui::{
    Frame,
    layout::{Constraint, Flex, Layout, Rect},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
};

use crate::app::App;
use crate::ui::style;

const WIDTH: u16 = 66;

/// One row of the reference: `None` starts a dim section header, `Some`
/// renders a `key   description` line (key in accent, description dim).
enum Row {
    Section(&'static str),
    Bind(&'static str, &'static str),
}

/// The complete, ordered binding list — grouped exactly as `app.rs::handle_key`
/// and its sub-keymaps implement them. Nothing here is invented: every row
/// maps to a real `KeyCode::` arm.
const ROWS: &[Row] = &[
    Row::Section("Navigation"),
    Row::Bind("Tab", "cycle lenses (Macro/Micro/Replication/Schema/Indexes/Query)"),
    Row::Bind("j / \u{2193}", "move selection down"),
    Row::Bind("k / \u{2191}", "move selection up"),
    Row::Bind("Enter", "open/close the selected row's detail panel"),
    Row::Section("Sub-views & overlays"),
    Row::Bind("/", "filter activity (Micro Lens only)"),
    Row::Bind("w", "full waits panel (Micro Lens only)"),
    Row::Bind("I", "idle connection census (Micro Lens only)"),
    Row::Bind("v", "Vacuum sub-view (Schema Lens only)"),
    Row::Bind("d", "database picker (any lens)"),
    Row::Bind("!", "open a psql shell on the same connection (any lens)"),
    Row::Bind("?", "this help"),
    Row::Section("Data & refresh"),
    Row::Bind("R", "force schema/query-stats refresh (any lens)"),
    Row::Bind("s", "cycle sort (Micro/Schema Tables/Query Lens)"),
    Row::Bind("+ / =", "increase poll interval"),
    Row::Bind("-", "decrease poll interval"),
    Row::Bind("Space", "pause / resume the display"),
    Row::Section("Admin (Micro Lens, selected row)"),
    Row::Bind("c", "cancel the query (asks to confirm)"),
    Row::Bind("K", "terminate the backend (asks to confirm)"),
    Row::Bind("y / n", "confirm / abort — while that modal is open"),
    Row::Section("Quit"),
    Row::Bind("q", "quit immediately"),
    Row::Bind("Ctrl+C", "quit immediately (works everywhere)"),
    Row::Bind("Esc", "close the open overlay/panel, or arm a 2s quit \u{2014}"),
    Row::Bind("", "a second Esc within that window quits"),
];

pub fn draw(app: &App, frame: &mut Frame) {
    if !app.help_open {
        return;
    }
    let area = frame.area();

    let key_width = ROWS
        .iter()
        .filter_map(|r| match r {
            Row::Bind(k, _) => Some(k.chars().count()),
            Row::Section(_) => None,
        })
        .max()
        .unwrap_or(0);

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(ROWS.len() + 2);
    for row in ROWS {
        match row {
            Row::Section(title) => {
                if !lines.is_empty() {
                    lines.push(Line::default());
                }
                lines.push(Line::from(Span::styled(*title, style::accent_style())));
            }
            Row::Bind(key, desc) => {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(format!("{key:<key_width$}"), style::accent_style()),
                    Span::raw("  "),
                    Span::styled(*desc, style::label_style()),
                ]));
            }
        }
    }
    lines.push(Line::from(Span::styled("Esc / ?: close", style::label_style())).centered());

    let width = WIDTH.min(area.width.saturating_sub(2).max(20));
    let height = (lines.len() as u16 + 2).min(area.height.saturating_sub(2).max(5));

    let rect = centered(area, width, height);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(" keyboard help ")
                .border_style(style::accent_style()),
        ),
        rect,
    );
}

/// A `width` x `height` rect centered in `area` (clamped to fit) — identical
/// helper to `ui/confirm.rs`/`ui/db_picker.rs`.
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
        let rect = centered(area, WIDTH, 20);
        assert!(rect.x > 0 && rect.y > 0);
        assert_eq!(rect.width, WIDTH);

        // Tiny terminal: clamped, never panics or overflows.
        let tiny = Rect::new(0, 0, 20, 4);
        let rect = centered(tiny, WIDTH, 20);
        assert!(rect.width <= 20 && rect.height <= 4);
    }
}
