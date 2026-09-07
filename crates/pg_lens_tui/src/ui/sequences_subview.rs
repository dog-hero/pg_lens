//! Sequences sub-view (v0.19, `S` in Schema Lens): sequence headroom
//! exhaustion watch. Identifies integer/smallint columns nearing overflow limit.

use pg_lens_core::{SchemaSnapshot, SequenceRow, SequenceSeverity};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Paragraph, Row, Table},
};

use crate::app::App;
use crate::ui::{format, style};

fn severity_marker_and_style(sev: SequenceSeverity) -> (&'static str, Style) {
    match sev {
        SequenceSeverity::Critical => ("!!", Style::new().fg(Color::Red).bold()),
        SequenceSeverity::Warning => ("! ", Style::new().fg(Color::Yellow).bold()),
        SequenceSeverity::Normal => ("  ", Style::new()),
    }
}

fn format_progress_bar(pct: f64, bar_width: usize) -> String {
    let filled = ((pct / 100.0) * bar_width as f64).round() as usize;
    let filled = filled.min(bar_width);
    let empty = bar_width.saturating_sub(filled);
    format!(
        "[{}{}] {:>5.1}%",
        "\u{2588}".repeat(filled),
        "\u{2591}".repeat(empty),
        pct
    )
}

pub fn draw_sequences_view(
    app: &mut App,
    schema: &SchemaSnapshot,
    frame: &mut Frame,
    area: Rect,
) {
    let [table_area, summary_area, footer_area] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_sequences_table(app, schema, frame, table_area);
    draw_summary(schema, frame, summary_area);
    draw_footer(app, schema, frame, footer_area);
}

fn draw_sequences_table(
    app: &mut App,
    schema: &SchemaSnapshot,
    frame: &mut Frame,
    area: Rect,
) {
    let header = Row::new([
        "!", "Schema", "Sequence", "Table", "Column", "Type", "Last Value", "Max Value", "Used %", "Remaining",
    ])
    .style(Style::new().bold());

    let title = if !app.schema_filter.is_empty() {
        format!("Sequences — Headroom Exhaustion Watch [filter: \"{}\" \u{2014} \\ to clear]", app.schema_filter)
    } else {
        "Sequences — Headroom Exhaustion Watch (S/Esc: return to Tables)".to_string()
    };

    let rows = app
        .sequences_row_order
        .iter()
        .filter_map(|&i| schema.sequences.get(i))
        .map(|seq: &SequenceRow| {
            let (marker, sev_style) = severity_marker_and_style(seq.severity);
            let bar_str = format_progress_bar(seq.percent_used, 8);
            let last_val_str = seq
                .last_value
                .map(format::human_count)
                .unwrap_or_else(|| "-".to_string());
            let max_val_str = format::human_count(seq.max_value);
            let remaining_str = format::human_count(seq.remaining_count);

            Row::new([
                marker.to_string(),
                seq.schema.clone(),
                seq.sequence_name.clone(),
                if seq.table_name.is_empty() {
                    "-".to_string()
                } else {
                    seq.table_name.clone()
                },
                if seq.column_name.is_empty() {
                    "-".to_string()
                } else {
                    seq.column_name.clone()
                },
                seq.data_type.clone(),
                last_val_str,
                max_val_str,
                bar_str,
                remaining_str,
            ])
            .style(sev_style)
        });

    let widths = [
        Constraint::Length(2),
        Constraint::Length(12),
        Constraint::Min(16),
        Constraint::Length(16),
        Constraint::Length(14),
        Constraint::Length(9),
        Constraint::Length(11),
        Constraint::Length(11),
        Constraint::Length(17),
        Constraint::Length(11),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::bordered().title(title))
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_symbol("\u{25b6} ");

    frame.render_stateful_widget(table, area, &mut app.sequences_table_state);
}

fn draw_summary(schema: &SchemaSnapshot, frame: &mut Frame, area: Rect) {
    let total = schema.sequences.len();
    let critical = schema
        .sequences
        .iter()
        .filter(|s| s.severity == SequenceSeverity::Critical)
        .count();
    let warning = schema
        .sequences
        .iter()
        .filter(|s| s.severity == SequenceSeverity::Warning)
        .count();

    let mut spans = vec![
        Span::raw(" "),
        Span::styled(format!("{total} sequences"), Style::new().bold()),
        Span::styled(" \u{b7} ", style::label_style()),
    ];

    if critical > 0 {
        spans.push(Span::styled(
            format!("{critical} CRITICAL (\u{2265} 90%)"),
            Style::new().fg(Color::Red).bold(),
        ));
        spans.push(Span::styled(" \u{b7} ", style::label_style()));
    }

    if warning > 0 {
        spans.push(Span::styled(
            format!("{warning} warning (\u{2265} 75%)"),
            Style::new().fg(Color::Yellow).bold(),
        ));
        spans.push(Span::styled(" \u{b7} ", style::label_style()));
    }

    spans.push(Span::styled(
        "sorted by percent used (headroom exhaustion)",
        style::label_style(),
    ));

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_footer(app: &App, schema: &SchemaSnapshot, frame: &mut Frame, area: Rect) {
    let staleness_secs =
        (pg_lens_core::history::epoch_ms_now().saturating_sub(schema.collected_at_epoch_ms))
            / 1_000;
    let line = Line::from(format!(
        " db: {db} \u{b7} collected {staleness_secs}s ago \u{b7} S/Esc: return to Tables \u{b7} /: filter \u{b7} y: copy sequence",
        db = app.snapshot.vitals.database,
    ))
    .dim();
    frame.render_widget(Paragraph::new(line), area);
}

