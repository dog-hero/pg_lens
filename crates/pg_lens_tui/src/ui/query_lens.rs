//! Query Lens: normalized statement stats from `pg_stat_statements`,
//! filtered to the connected database (the extension is cluster-wide — the
//! footer says so).
//!
//! Row semantics:
//! - the Query cell is SQL-highlighted after a char-safe truncation (same
//!   tokenizer as the Micro Lens — `ui/sql.rs`);
//! - `Hit%` = shared_blks_hit / (hit + read); `—` when zero blocks were
//!   touched (never a made-up 0% or 100%);
//! - `Enter` opens a detail panel: full normalized query (highlighted,
//!   wrapped) plus every metric incl. the queryid; `Enter`/`Esc` close it;
//! - `Unavailable` (extension missing / older than 1.8) renders a friendly
//!   centered explainer with the `CREATE EXTENSION` + preload hints — a calm
//!   per-lens state, never an error banner;
//! - a failed collection keeps the last rows on screen under an inline
//!   error line (same pattern as the Schema Lens);
//! - the footer names the database, the row count and the slow collection's
//!   staleness (statements share the Schema Lens cadence; `R` refreshes
//!   both).

use pg_lens_core::{StatementsSnapshot, StatementsStatus};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::Line,
    widgets::{Block, Cell, Clear, Paragraph, Row, Table, Wrap},
};

use crate::app::App;
use crate::ui::{format, sql, style};

/// Fixed widths of every column except the flexible Query one, in order:
/// Calls, Total, Mean, Rows, Hit%.
const FIXED_WIDTHS: [u16; 5] = [8, 9, 9, 8, 6];
const COLUMN_SPACING: u16 = 1;
/// Highlight symbol "▶ " rendered left of the selected row.
const HIGHLIGHT_WIDTH: u16 = 2;

/// Shared-buffer cache hit ratio in percent; `None` when no shared blocks
/// were touched at all — rendered `—`, never a fabricated number.
fn hit_pct(hit: i64, read: i64) -> Option<f64> {
    let total = hit + read;
    if total > 0 {
        Some(hit as f64 * 100.0 / total as f64)
    } else {
        None
    }
}

fn hit_pct_cell(hit: i64, read: i64) -> String {
    hit_pct(hit, read).map_or_else(|| "\u{2014}".to_string(), |p| format!("{p:.1}%"))
}

pub fn draw(app: &mut App, frame: &mut Frame, area: Rect) {
    let Some(statements) = app.snapshot.statements.clone() else {
        // First slow collection still pending (it runs on connect, so this
        // is short-lived): a friendly placeholder, not an empty table.
        let placeholder = Paragraph::new(vec![
            Line::default(),
            Line::from(" collecting statement stats\u{2026} (first slow collection pending)")
                .dim(),
        ])
        .block(Block::bordered().title("Statements"));
        frame.render_widget(placeholder, area);
        return;
    };

    if let StatementsStatus::Unavailable(reason) = &statements.status {
        draw_unavailable(reason, frame, area);
        return;
    }

    let error_height = match statements.status {
        StatementsStatus::Error(_) => 1,
        _ => 0,
    };
    let [table_area, error_area, footer_area] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(error_height),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_table(app, &statements, frame, table_area);
    if let StatementsStatus::Error(msg) = &statements.status {
        let line = Line::from(format!(" statements: {msg} \u{2014} showing last collection"))
            .style(Style::new().fg(Color::White).bg(Color::Red).bold());
        frame.render_widget(Paragraph::new(line), error_area);
    }
    draw_footer(app, &statements, frame, footer_area);
    if app.detail_open {
        draw_detail(app, frame, area);
    }
}

/// The calm per-lens explainer for `StatementsStatus::Unavailable`: what is
/// missing and exactly what to run — centered, dim chrome, never red.
fn draw_unavailable(reason: &str, frame: &mut Frame, area: Rect) {
    let lines = vec![
        Line::default(),
        Line::from("pg_stat_statements not available")
            .style(style::accent_style())
            .alignment(Alignment::Center),
        Line::default(),
        Line::from(reason.to_string()).alignment(Alignment::Center),
        Line::default(),
        Line::from("to enable it:")
            .style(style::label_style())
            .alignment(Alignment::Center),
        sql::highlight_line("  CREATE EXTENSION pg_stat_statements;").alignment(Alignment::Center),
        Line::from("(needs shared_preload_libraries = 'pg_stat_statements' + restart)")
            .style(style::label_style())
            .alignment(Alignment::Center),
    ];
    let panel = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(Block::bordered().title("Statements"));
    frame.render_widget(panel, area);
}

fn draw_table(app: &mut App, statements: &StatementsSnapshot, frame: &mut Frame, area: Rect) {
    let header = Row::new(["Query", "Calls", "Total", "Mean", "Rows", "Hit%"])
        .style(Style::new().bold());

    let query_width = query_column_width(area.width);

    let rows = app
        .statements_row_order
        .iter()
        .filter_map(|&i| statements.statements.get(i))
        .map(|row| {
            // Truncate FIRST (char-safe), then tokenize — the ellipsis
            // lands in a default-styled span (same as the Micro Lens).
            let query_text = format::truncate_with_ellipsis(&row.query, query_width);
            Row::new(vec![
                Cell::from(sql::highlight_line(&query_text)),
                Cell::from(format::human_count(row.calls)),
                Cell::from(format::human_ms(row.total_exec_ms)),
                Cell::from(format::human_ms(row.mean_exec_ms)),
                Cell::from(format::human_count(row.rows)),
                Cell::from(hit_pct_cell(row.shared_blks_hit, row.shared_blks_read)),
            ])
        });

    let widths = [
        Constraint::Min(10),
        Constraint::Length(FIXED_WIDTHS[0]),
        Constraint::Length(FIXED_WIDTHS[1]),
        Constraint::Length(FIXED_WIDTHS[2]),
        Constraint::Length(FIXED_WIDTHS[3]),
        Constraint::Length(FIXED_WIDTHS[4]),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::bordered().title("Statements"))
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_symbol("\u{25b6} ");

    frame.render_stateful_widget(table, area, &mut app.statements_table_state);
}

/// How many characters the flexible Query column can hold at this terminal
/// width (same arithmetic as the Micro/Schema lens tables).
fn query_column_width(area_width: u16) -> usize {
    let fixed: u16 = FIXED_WIDTHS.iter().sum();
    let overhead = 2 /* block borders */ + HIGHLIGHT_WIDTH + fixed + 5 * COLUMN_SPACING;
    usize::from(area_width.saturating_sub(overhead))
}

/// `db: shop · 8 statements · collected 12s ago · current database only` —
/// which database (pg_stat_statements is cluster-wide; this lens filters),
/// how fresh the slow collection is, and the shared-refresh hint.
fn draw_footer(app: &App, statements: &StatementsSnapshot, frame: &mut Frame, area: Rect) {
    let staleness_secs = (pg_lens_core::history::epoch_ms_now()
        .saturating_sub(statements.collected_at_epoch_ms))
        / 1_000;
    let line = Line::from(format!(
        " db: {db} \u{b7} {n} statements \u{b7} collected {staleness_secs}s ago \u{b7} \
         current database only \u{b7} R: recollect",
        db = app.snapshot.vitals.database,
        n = statements.statements.len(),
    ))
    .dim();
    frame.render_widget(Paragraph::new(line), area);
}

/// Detail panel over the lower part of the lens: every metric (incl. the
/// queryid) plus the full normalized query, highlighted and wrapped.
fn draw_detail(app: &App, frame: &mut Frame, area: Rect) {
    let Some(row) = app.selected_statement() else {
        return;
    };

    let [_, panel_area] =
        Layout::vertical([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(area);

    let title = format!(
        "Statement \u{2014} queryid {} (Enter/Esc: close)",
        row.query_id.as_deref().unwrap_or("\u{2014}"),
    );

    let mut lines = vec![
        style::kv("user:  ", row.username.clone()),
        style::kv(
            "calls: ",
            format!(
                "{} \u{b7} rows {} ({} per call)",
                format::human_count(row.calls),
                format::human_count(row.rows),
                if row.calls > 0 {
                    format!("{:.1}", row.rows as f64 / row.calls as f64)
                } else {
                    "\u{2014}".to_string()
                },
            ),
        ),
        style::kv(
            "time:  ",
            format!(
                "total {} \u{b7} mean {}",
                format::human_ms(row.total_exec_ms),
                format::human_ms(row.mean_exec_ms),
            ),
        ),
        style::kv(
            "shared blocks: ",
            format!(
                "hit {} \u{b7} read {} \u{b7} hit% {}",
                format::human_count(row.shared_blks_hit),
                format::human_count(row.shared_blks_read),
                hit_pct_cell(row.shared_blks_hit, row.shared_blks_read),
            ),
        ),
        Line::from("query:").style(style::label_style()),
    ];
    lines.extend(sql::highlight_lines(&row.query));

    let panel = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(Block::bordered().title(title));
    frame.render_widget(Clear, panel_area);
    frame.render_widget(panel, panel_area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_pct_guards_the_zero_division() {
        assert_eq!(hit_pct(0, 0), None, "no blocks touched = no ratio");
        assert_eq!(hit_pct(90, 10), Some(90.0));
        assert_eq!(hit_pct(0, 10), Some(0.0));
        assert_eq!(hit_pct(10, 0), Some(100.0));
    }

    #[test]
    fn hit_pct_cell_renders_a_dash_or_a_percentage() {
        assert_eq!(hit_pct_cell(0, 0), "\u{2014}");
        assert_eq!(hit_pct_cell(997, 3), "99.7%");
        assert_eq!(hit_pct_cell(1, 2), "33.3%");
    }

    #[test]
    fn query_width_shrinks_with_the_terminal_and_never_underflows() {
        // 120 cols: 120 - (2 + 2 + 40 + 5) = 71 chars for the query.
        assert_eq!(query_column_width(120), 71);
        assert!(query_column_width(80) < query_column_width(120));
        assert_eq!(query_column_width(10), 0);
    }
}
