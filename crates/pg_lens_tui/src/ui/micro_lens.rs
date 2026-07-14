//! Micro Lens: per-session activity table (mock data in Fase 1).

use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::{Modifier, Style},
    widgets::{Block, Row, Table},
};

use crate::app::App;

pub fn draw(app: &mut App, frame: &mut Frame, area: Rect) {
    let header = Row::new([
        "PID", "DB", "User", "Client", "State", "Wait", "Duration", "Query",
    ])
    .style(Style::new().bold());

    let rows = app.snapshot.activity.iter().map(|row| {
        Row::new([
            row.pid.to_string(),
            row.database.clone(),
            row.username.clone(),
            row.client.clone(),
            row.state.clone(),
            row.wait_event.clone().unwrap_or_default(),
            format_duration(row.duration_secs),
            row.query.clone(),
        ])
    });

    let widths = [
        Constraint::Length(6),
        Constraint::Length(10),
        Constraint::Length(12),
        Constraint::Length(12),
        Constraint::Length(11),
        Constraint::Length(22),
        Constraint::Length(8),
        Constraint::Min(20),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::bordered().title("Activity"))
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_symbol("\u{25b6} ");

    frame.render_stateful_widget(table, area, &mut app.table_state);
}

/// `0.4s`, `12s`, `6m24s` — good enough until Fase 4's format.rs.
fn format_duration(secs: f64) -> String {
    if secs < 1.0 {
        format!("{secs:.1}s")
    } else if secs < 60.0 {
        format!("{secs:.0}s")
    } else {
        let mins = (secs / 60.0).floor();
        let rest = secs - mins * 60.0;
        format!("{mins:.0}m{rest:02.0}s")
    }
}

#[cfg(test)]
mod tests {
    use super::format_duration;

    #[test]
    fn duration_formats_are_compact() {
        assert_eq!(format_duration(0.43), "0.4s");
        assert_eq!(format_duration(12.7), "13s");
        assert_eq!(format_duration(384.2), "6m24s");
    }
}
