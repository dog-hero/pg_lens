//! Startup service picker, shown INSTEAD of the splash/dashboard while
//! `App::picker` is `Some` — i.e. before any poller exists.
//!
//! When pg_lens starts with no connection hints at all (no `--dsn`, no
//! `--service`, none of PGHOST/PGSERVICE/PG_LENS_SERVICE/PG_LENS_DSN) but a
//! valid services file with at least one entry, `main.rs` opens the TUI in
//! picker mode: the splash wordmark on top, a centered `select a service`
//! panel listing every `[services.<name>]` entry as `name  —  user@host`
//! (exactly what the file says — env/default fallbacks NOT applied), plus a
//! final `localhost  —  (default)` entry mapping to the plain no-service
//! resolution. Enter hands the choice back to `main.rs` (which resolves,
//! spawns the poller, and lets the connection splash take over);
//! `q`/`Esc` quit cleanly.
//!
//! Display safety: `PickerEntry` carries name/host/user only — a `password`
//! or `password_cmd` string can never reach this screen (see the
//! `settings::list_services`-based construction in `main.rs`).

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout},
    text::{Line, Span},
    widgets::{Block, Paragraph},
};

use crate::app::{App, PickerState};
use crate::ui::{splash, style};

/// `name  —  detail`, the em-dash separator of the panel rows.
const SEP: &str = "  \u{2014}  ";

pub fn draw(app: &App, frame: &mut Frame) {
    let Some(picker) = &app.picker else {
        return; // not in picker mode: nothing to draw (callers gate on it)
    };
    let area = frame.area();

    // Names are left-aligned in a common column so the details line up.
    let name_width = picker
        .entries
        .iter()
        .map(|e| e.name.chars().count())
        .max()
        .unwrap_or(0);

    let title = " select a service ";
    let content_width = picker
        .entries
        .iter()
        .map(|e| 2 + name_width + SEP.chars().count() + e.detail.chars().count())
        .max()
        .unwrap_or(0)
        .max(title.chars().count()) as u16;
    // Borders (2) + one space of horizontal padding each side (2).
    let panel_width = (content_width + 4).min(area.width.saturating_sub(4).max(20));
    let panel_height = picker.entries.len() as u16 + 2;

    // Wordmark (2) + blank + panel + hint line, vertically centered.
    let [_, content_area, _] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(3 + panel_height + 1),
        Constraint::Min(0),
    ])
    .areas(area);
    let [wordmark_area, _, panel_row, hint_area] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(panel_height),
        Constraint::Length(1),
    ])
    .areas(content_area);
    let [_, panel_area, _] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(panel_width),
        Constraint::Min(0),
    ])
    .areas(panel_row);

    let [wordmark, subtitle] = splash::wordmark_lines();
    frame.render_widget(
        Paragraph::new(vec![wordmark, subtitle]).alignment(Alignment::Center),
        wordmark_area,
    );

    frame.render_widget(
        Paragraph::new(entry_lines(picker, name_width))
            .block(Block::bordered().title(title).border_style(style::label_style())),
        panel_area,
    );

    // Keybinding hints, splash-statusbar style: keys accented, text dim.
    let sep = Span::styled(" \u{b7} ", style::label_style());
    let mut hint = Vec::new();
    for (i, (key, desc)) in [
        ("j/k", ": move"),
        ("Enter", ": connect"),
        ("q/Esc", ": quit"),
    ]
    .into_iter()
    .enumerate()
    {
        if i > 0 {
            hint.push(sep.clone());
        }
        let [k, d] = style::hint(key, desc);
        hint.push(k);
        hint.push(d);
    }
    frame.render_widget(
        Paragraph::new(Line::from(hint)).alignment(Alignment::Center),
        hint_area,
    );
}

/// One line per entry: `▸ name  —  detail` for the selected row (accented),
/// `  name  —  detail` (dim detail) otherwise.
fn entry_lines(picker: &PickerState, name_width: usize) -> Vec<Line<'static>> {
    picker
        .entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let selected = i == picker.selected;
            let marker = if selected { "\u{25b8} " } else { "  " };
            let name = format!("{marker}{name:<name_width$}", name = entry.name);
            let name_style = if selected {
                style::accent_style()
            } else {
                style::value_style()
            };
            Line::from(vec![
                Span::raw(" "), // horizontal padding inside the border
                Span::styled(name, name_style),
                Span::styled(SEP, style::label_style()),
                Span::styled(entry.detail.clone(), style::label_style()),
            ])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Action, PickerEntry, update};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend};

    fn picker_app() -> App {
        let mut app = App::new();
        app.picker = Some(PickerState::new(vec![
            PickerEntry {
                name: "prod".into(),
                detail: "svc_ro@db.prod.internal".into(),
                service: Some("prod".into()),
            },
            PickerEntry {
                name: "staging".into(),
                detail: "postgres@db.staging.internal".into(),
                service: Some("staging".into()),
            },
            PickerEntry {
                name: "localhost".into(),
                detail: "(default)".into(),
                service: None,
            },
        ]));
        app
    }

    fn render(app: &mut App) -> String {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| crate::ui::draw(app, frame))
            .expect("draw");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    /// The row of the rendered screen containing `needle`.
    fn row_of(screen: &str, cols: usize, needle: &str) -> String {
        screen
            .chars()
            .collect::<Vec<_>>()
            .chunks(cols)
            .map(|row| row.iter().collect::<String>())
            .find(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("row containing {needle:?} in:\n{screen}"))
    }

    #[test]
    fn picker_renders_wordmark_panel_entries_and_default() {
        let mut app = picker_app();
        let screen = render(&mut app);
        assert!(screen.contains("p g _ l e n s"), "wordmark: {screen}");
        assert!(screen.contains("select a service"));
        assert!(screen.contains("prod"));
        assert!(screen.contains("svc_ro@db.prod.internal"));
        assert!(screen.contains("staging"));
        assert!(screen.contains("localhost"));
        assert!(screen.contains("(default)"), "default entry: {screen}");
        assert!(screen.contains("Enter: connect"));
        // No dashboard/splash chrome underneath.
        assert!(!screen.contains("Macro Lens"));
        assert!(!screen.contains("connecting to"));
    }

    #[test]
    fn picker_marks_the_selected_entry_and_follows_j() {
        let mut app = picker_app();
        let screen = render(&mut app);
        assert!(
            row_of(&screen, 100, "prod").contains('\u{25b8}'),
            "first entry starts selected: {screen}"
        );
        assert!(!row_of(&screen, 100, "staging").contains('\u{25b8}'));

        update(
            &mut app,
            Action::Key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)),
        );
        let screen = render(&mut app);
        assert!(row_of(&screen, 100, "staging").contains('\u{25b8}'));
        assert!(!row_of(&screen, 100, "svc_ro@db.prod.internal").contains('\u{25b8}'));
    }

    #[test]
    fn picker_fits_a_small_terminal() {
        let mut app = picker_app();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| crate::ui::draw(&mut app, frame))
            .expect("draw");
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(screen.contains("select a service"));
        assert!(screen.contains("localhost"));
    }
}
