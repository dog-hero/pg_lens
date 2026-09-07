//! Records Lens (v0.18.0): recorded incident sessions and snapshot bookmarks
//! on disk.
//!
//! Provides an interactive view to inspect, filter, sort, replay in-app, copy path,
//! or delete recorded sessions (`.jsonl`) and snapshots (`.json`).

use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Cell, Paragraph, Row, Table},
};

use crate::app::App;
use crate::ui::format;

pub fn draw(app: &mut App, frame: &mut Frame, area: Rect) {
    if app.records_row_order.is_empty() {
        draw_empty(app, frame, area);
    } else {
        draw_table(app, frame, area);
    }
}

fn status_badge(entry: &pg_lens_core::recording::RecordingEntry) -> Span<'static> {
    if entry.is_active {
        Span::styled(
            "\u{25cf} REC",
            Style::new().fg(Color::Green).add_modifier(Modifier::BOLD),
        )
    } else {
        match entry.kind {
            pg_lens_core::recording::RecordingKind::Recording => Span::styled(
                "REC",
                Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
            pg_lens_core::recording::RecordingKind::Bookmark => Span::styled(
                "BOOKMARK",
                Style::new().fg(Color::Magenta).add_modifier(Modifier::BOLD),
            ),
        }
    }
}

fn draw_table(app: &mut App, frame: &mut Frame, area: Rect) {
    let filter_active = !app.records_filter.is_empty();
    let sort_label = app.records_sort_mode.label();

    let title = if app.records_filter_editing {
        format!(" Records Lens [filter: {}_\u{2588}] ", app.records_filter)
    } else if filter_active {
        format!(
            " Records Lens [filter: \"{}\" \u{2014} {} matching] (sort: {}) ",
            app.records_filter,
            app.records_row_order.len(),
            sort_label,
        )
    } else {
        format!(
            " Records Lens ({} items) (sort: {}) ",
            app.records_row_order.len(),
            sort_label,
        )
    };

    let block = Block::bordered()
        .title(title)
        .border_style(Style::new().fg(Color::Cyan));

    let header = Row::new(vec![
        Cell::from(Span::styled("STATUS/KIND", Style::new().bold().underlined())),
        Cell::from(Span::styled("TARGET", Style::new().bold().underlined())),
        Cell::from(Span::styled("FILENAME", Style::new().bold().underlined())),
        Cell::from(Span::styled("SIZE", Style::new().bold().underlined())),
        Cell::from(Span::styled("FRAMES", Style::new().bold().underlined())),
        Cell::from(Span::styled("STARTED", Style::new().bold().underlined())),
        Cell::from(Span::styled("ENDED", Style::new().bold().underlined())),
        Cell::from(Span::styled("DURATION", Style::new().bold().underlined())),
    ])
    .style(Style::new().fg(Color::White))
    .bottom_margin(1);

    let rows: Vec<Row> = app
        .records_row_order
        .iter()
        .filter_map(|&idx| app.records.get(idx))
        .map(|entry| {
            let status_cell = Cell::from(status_badge(entry));
            let target_cell = Cell::from(Span::styled(
                entry.target.clone(),
                Style::new().fg(Color::White).bold(),
            ));
            let filename_style = if entry.is_active {
                Style::new().fg(Color::Green).bold()
            } else {
                Style::new().fg(Color::Yellow)
            };
            let filename_cell = Cell::from(Span::styled(entry.filename.clone(), filename_style));
            let size_cell = Cell::from(Span::styled(
                format::human_bytes(entry.size_bytes as i64),
                Style::new().dim(),
            ));
            let frames_str = entry
                .frame_count
                .map(|c| c.to_string())
                .unwrap_or_else(|| "\u{2014}".to_string());
            let frames_cell = Cell::from(Span::styled(frames_str, Style::new().dim()));

            let started_str = entry
                .started_at
                .as_deref()
                .unwrap_or("\u{2014}");
            let started_cell = Cell::from(Span::styled(started_str.to_string(), Style::new().dim()));

            let ended_str = if entry.is_active {
                "recording...".to_string()
            } else {
                entry
                    .ended_at
                    .as_deref()
                    .unwrap_or("\u{2014}")
                    .to_string()
            };
            let ended_cell = Cell::from(Span::styled(
                ended_str,
                if entry.is_active {
                    Style::new().fg(Color::Green)
                } else {
                    Style::new().dim()
                },
            ));

            let duration_str = if let Some(dur) = entry.duration_secs {
                pg_lens_core::recording::format_duration_secs(dur)
            } else if entry.is_active {
                "live...".to_string()
            } else {
                "\u{2014}".to_string()
            };
            let duration_cell = Cell::from(Span::styled(
                duration_str,
                if entry.is_active {
                    Style::new().fg(Color::Green)
                } else {
                    Style::new().fg(Color::Magenta)
                },
            ));

            Row::new(vec![
                status_cell,
                target_cell,
                filename_cell,
                size_cell,
                frames_cell,
                started_cell,
                ended_cell,
                duration_cell,
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(13),
        Constraint::Length(16),
        Constraint::Min(25),
        Constraint::Length(10),
        Constraint::Length(8),
        Constraint::Length(21),
        Constraint::Length(21),
        Constraint::Length(12),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(
            Style::new()
                .bg(Color::Rgb(35, 45, 60))
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("\u{25b6} ");

    frame.render_stateful_widget(table, area, &mut app.records_table_state);
}

fn draw_empty(app: &App, frame: &mut Frame, area: Rect) {
    let title = if app.records_filter_editing {
        format!(" Records Lens [filter: {}_\u{2588}] ", app.records_filter)
    } else if !app.records_filter.is_empty() {
        format!(" Records Lens [filter: \"{}\"] ", app.records_filter)
    } else {
        " Records Lens ".to_string()
    };

    let block = Block::bordered()
        .title(title)
        .border_style(Style::new().dim());

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if !app.records_filter.is_empty() {
        let msg = Line::from(vec![
            Span::styled(" No recordings or bookmarks match filter \"", Style::new().dim()),
            Span::styled(app.records_filter.clone(), Style::new().fg(Color::Yellow)),
            Span::styled("\" (press \\ to clear)", Style::new().dim()),
        ]);
        frame.render_widget(Paragraph::new(msg), inner);
        return;
    }

    let rec_dir = pg_lens_core::recording::recordings_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "~/.local/share/pg_lens/recordings".to_string());
    let exp_dir = pg_lens_core::recording::exports_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "~/.local/share/pg_lens/exports".to_string());

    let lines = vec![
        Line::default(),
        Line::from(Span::styled(
            " No recorded sessions or snapshot bookmarks found",
            Style::new().fg(Color::White).bold(),
        )),
        Line::default(),
        Line::from(vec![
            Span::styled("   \u{2022} Press ", Style::new().dim()),
            Span::styled("R", Style::new().fg(Color::Cyan).bold()),
            Span::styled(
                " to toggle continuous incident recording (.jsonl)",
                Style::new().dim(),
            ),
        ]),
        Line::from(vec![
            Span::styled("   \u{2022} Press ", Style::new().dim()),
            Span::styled("E", Style::new().fg(Color::Cyan).bold()),
            Span::styled(
                " to export an immediate snapshot bookmark (.json)",
                Style::new().dim(),
            ),
        ]),
        Line::from(vec![
            Span::styled("   \u{2022} Press ", Style::new().dim()),
            Span::styled("b/B", Style::new().fg(Color::Cyan).bold()),
            Span::styled(
                " to rescan disk for newly added files",
                Style::new().dim(),
            ),
        ]),
        Line::default(),
        Line::from(Span::styled(
            format!(" Recordings directory: {rec_dir}"),
            Style::new().dim(),
        )),
        Line::from(Span::styled(
            format!(" Exports directory:    {exp_dir}"),
            Style::new().dim(),
        )),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}

