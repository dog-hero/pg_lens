//! Schema Lens: per-table stats + ESTIMATED bloat of the connected database
//! (Fase S3). U1 promoted the index advisor out of this lens into its own
//! `ui/index_lens.rs` tab — this lens is Tables + the vacuum section again.
//!
//! Row semantics (mirroring the Micro Lens's textual-marker precedent, so
//! PTY captures prove severity without colors):
//! - severity column `!`: `!!` = red tier (estimated bloat% over 50 AND
//!   bloat over 10 MB), `!` = yellow tier (over 30% AND over 1 MB), blank
//!   otherwise; the row style mirrors it (red wins over yellow);
//! - `is_na` rows (ioguix: estimate not applicable) render dim with `~?` in
//!   both bloat cells — never a made-up number. A table with no matching
//!   bloat row also shows `~?` (estimate missing), undimmed;
//! - `Last AV` prefers `last_autovacuum`, falls back to `last_vacuum`,
//!   else `—`;
//! - `Enter` opens a detail panel (full vacuum/analyze stats + the table's
//!   index bloat, joined via `BloatRow::table`); `Enter`/`Esc` close it;
//! - the footer names the database (the lens is per-database), the row
//!   count, the slow collection's staleness, and says ESTIMATED — the
//!   plan forbids presenting the estimate as a measurement.

use std::time::{SystemTime, UNIX_EPOCH};

use pg_lens_core::{BloatRow, SchemaSnapshot, SchemaStatus};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, Row, Table, Wrap},
};

use crate::app::{App, find_table_bloat};
use crate::ui::{format, style, vacuum};

/// Fixed widths of every column except the flexible Table one, in order:
/// severity, Size, Live, Dead, Bloat%, Bloat, Last AV, Seq/Idx.
const SEVERITY_WIDTH: u16 = 2;
const FIXED_WIDTHS: [u16; 7] = [9, 6, 6, 7, 9, 10, 11];
const COLUMN_SPACING: u16 = 1;
/// Highlight symbol "▶ " rendered left of the selected row.
const HIGHLIGHT_WIDTH: u16 = 2;
/// `~?`: estimate not applicable (is_na) or missing for this table.
const NO_ESTIMATE: &str = "~?";

/// Bloat severity tiers of the plan (S0 decision 3). Red wins over yellow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Severity {
    /// > 50% estimated bloat AND > 10 MB wasted.
    Red,
    /// > 30% estimated bloat AND > 1 MB wasted (but not red).
    Yellow,
    /// Estimate exists but is unreliable (`is_na`) — render dim, no number.
    NotApplicable,
    None,
}

fn severity(bloat: Option<&BloatRow>) -> Severity {
    let Some(bloat) = bloat else {
        return Severity::None;
    };
    if bloat.is_na {
        return Severity::NotApplicable;
    }
    let (Some(pct), Some(bytes)) = (bloat.bloat_pct, bloat.bloat_bytes) else {
        return Severity::None;
    };
    if pct > 50.0 && bytes > 10 << 20 {
        Severity::Red
    } else if pct > 30.0 && bytes > 1 << 20 {
        Severity::Yellow
    } else {
        Severity::None
    }
}

/// Unix epoch seconds "now" — computed once per frame, passed down so the
/// formatting helpers stay pure.
fn now_epoch_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn draw(app: &mut App, frame: &mut Frame, area: Rect) {
    let Some(schema) = app.snapshot.schema.clone() else {
        // First slow collection still pending (it runs on connect, so this
        // is short-lived): a friendly placeholder, not an empty table.
        let placeholder = Paragraph::new(vec![
            Line::default(),
            Line::from(" collecting schema stats\u{2026} (first slow collection pending)").dim(),
        ])
        .block(Block::bordered().title("Tables"));
        frame.render_widget(placeholder, area);
        return;
    };

    let error_height = match schema.status {
        SchemaStatus::Ok => 0,
        SchemaStatus::Error(_) => 1,
    };
    let [table_area, vacuum_area, error_area, footer_area] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(vacuum_section_height()),
        Constraint::Length(error_height),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_table(app, &schema, frame, table_area);
    draw_vacuum(app, &schema, frame, vacuum_area);
    if let SchemaStatus::Error(msg) = &schema.status {
        let line = Line::from(format!(" schema: {msg} \u{2014} showing last collection"))
            .style(Style::new().fg(Color::White).bg(Color::Red).bold());
        frame.render_widget(Paragraph::new(line), error_area);
    }
    draw_footer(app, &schema, frame, footer_area);
    if app.detail_open {
        draw_detail(app, &schema, frame, area);
    }
}

/// How many worst-table rows the vacuum section shows — fixed so the panel
/// never jitters in height as the collection's row count varies.
const VACUUM_TABLE_ROWS: usize = 3;

/// Fixed height of the "Vacuum / wraparound" section: 2 border rows, one
/// cluster headline, up to [`VACUUM_TABLE_ROWS`] worst-table rows, and one
/// in-flight-progress row.
fn vacuum_section_height() -> u16 {
    2 + 1 + VACUUM_TABLE_ROWS as u16 + 1
}

/// Dead-tuple ratio as a percentage; `0.0` on an empty table (never NaN).
fn dead_pct(dead: i64, live: i64) -> f64 {
    let total = dead + live;
    if total > 0 {
        dead as f64 / total as f64 * 100.0
    } else {
        0.0
    }
}

/// F2's "Vacuum / wraparound" section: the cluster-wide XID wraparound
/// headline (severity-colored), the worst per-table ages with their
/// dead-tuple ratio, and any in-flight `pg_stat_progress_vacuum` row —
/// rendered as a calm "no vacuum running" when the (usual) case is empty.
fn draw_vacuum(app: &App, schema: &SchemaSnapshot, frame: &mut Frame, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();

    match &schema.vacuum_cluster_age {
        Some(age) => {
            let sev = vacuum::age_severity(age.max_age_xids);
            lines.push(Line::from(vec![
                Span::styled(format!("{} ", sev.marker()), Style::new().fg(sev.color())),
                Span::styled("wraparound: ", style::label_style()),
                Span::styled(
                    format!("{} xids", format::human_count(age.max_age_xids)),
                    Style::new().fg(sev.color()).bold(),
                ),
                Span::styled(
                    format!(" (worst db: {})", age.worst_database),
                    style::label_style(),
                ),
            ]));
        }
        None => lines.push(Line::from(" wraparound: collecting\u{2026}").dim()),
    }

    if schema.vacuum_tables.is_empty() {
        lines.push(Line::from("  (no per-table XID ages collected yet)").dim());
    } else {
        for table in schema.vacuum_tables.iter().take(VACUUM_TABLE_ROWS) {
            let sev = vacuum::age_severity(table.age_xids);
            let pct = dead_pct(table.n_dead_tup, table.n_live_tup);
            lines.push(Line::from(vec![
                Span::styled(format!("{} ", sev.marker()), Style::new().fg(sev.color())),
                Span::raw(format!("{}.{}  ", table.schema, table.name)),
                Span::styled(
                    format!("{} xids", format::human_count(table.age_xids)),
                    Style::new().fg(sev.color()),
                ),
                Span::styled(format!("  \u{b7} {pct:.1}% dead"), style::label_style()),
            ]));
        }
    }
    // Pad to the fixed row count so the panel height stays constant.
    while lines.len() < 1 + VACUUM_TABLE_ROWS {
        lines.push(Line::default());
    }

    match app.snapshot.vacuum_progress.as_deref() {
        Some([]) => lines.push(Line::from("  no vacuum running").dim()),
        Some(rows) => {
            // Usually one; if several run concurrently, the first is enough
            // for this compact strip (the full list would need its own view).
            if let Some(row) = rows.first() {
                let pct = if row.heap_blks_total > 0 {
                    100.0 * row.heap_blks_scanned as f64 / row.heap_blks_total as f64
                } else {
                    0.0
                };
                lines.push(Line::from(format!(
                    "  vacuuming {} \u{2014} {} ({pct:.0}%)",
                    row.relation, row.phase
                )));
            } else {
                lines.push(Line::from("  no vacuum running").dim());
            }
        }
        None => lines.push(Line::from("  vacuum progress: unavailable").dim()),
    }

    let panel =
        Paragraph::new(lines).block(Block::bordered().title("Vacuum / wraparound"));
    frame.render_widget(panel, area);
}

fn draw_table(app: &mut App, schema: &SchemaSnapshot, frame: &mut Frame, area: Rect) {
    let header = Row::new([
        "!", "Table", "Size", "Live", "Dead", "Bloat%", "Bloat", "Last AV", "Seq/Idx",
    ])
    .style(Style::new().bold());

    let table_width = table_column_width(area.width);
    let now = now_epoch_secs();

    let rows = app
        .schema_row_order
        .iter()
        .filter_map(|&i| schema.tables.get(i))
        .map(|table| {
            let bloat = find_table_bloat(schema, table);
            let tier = severity(bloat);
            let marker = match tier {
                Severity::Red => "!!",
                Severity::Yellow => "!",
                Severity::NotApplicable | Severity::None => "",
            };
            let style = match tier {
                Severity::Red => Style::new().fg(Color::Red).bold(),
                Severity::Yellow => Style::new().fg(Color::Yellow),
                Severity::NotApplicable => Style::new().dim(),
                Severity::None => Style::new(),
            };
            let (bloat_pct, bloat_bytes) = match bloat {
                Some(b) if !b.is_na => (
                    b.bloat_pct
                        .map_or_else(|| NO_ESTIMATE.to_string(), |p| format!("{p:.1}%")),
                    b.bloat_bytes
                        .map_or_else(|| NO_ESTIMATE.to_string(), format::human_bytes),
                ),
                // is_na or no bloat row at all: no made-up numbers.
                _ => (NO_ESTIMATE.to_string(), NO_ESTIMATE.to_string()),
            };
            let last_av = format::human_ago(
                table
                    .last_autovacuum_epoch_secs
                    .or(table.last_vacuum_epoch_secs),
                now,
            );
            let seq_idx = format!(
                "{}/{}",
                format::human_count(table.seq_scan),
                table
                    .idx_scan
                    .map_or_else(|| "\u{2014}".to_string(), format::human_count),
            );
            Row::new([
                marker.to_string(),
                format::truncate_with_ellipsis(
                    &format!("{}.{}", table.schema, table.name),
                    table_width,
                ),
                format::human_bytes(table.total_bytes),
                format::human_count(table.n_live_tup),
                format::human_count(table.n_dead_tup),
                bloat_pct,
                bloat_bytes,
                last_av,
                seq_idx,
            ])
            .style(style)
        });

    let widths = [
        Constraint::Length(SEVERITY_WIDTH),
        Constraint::Min(8),
        Constraint::Length(FIXED_WIDTHS[0]),
        Constraint::Length(FIXED_WIDTHS[1]),
        Constraint::Length(FIXED_WIDTHS[2]),
        Constraint::Length(FIXED_WIDTHS[3]),
        Constraint::Length(FIXED_WIDTHS[4]),
        Constraint::Length(FIXED_WIDTHS[5]),
        Constraint::Length(FIXED_WIDTHS[6]),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::bordered().title("Tables"))
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_symbol("\u{25b6} ");

    frame.render_stateful_widget(table, area, &mut app.schema_table_state);
}

/// How many characters the flexible Table column can hold at this terminal
/// width (same arithmetic as the Micro Lens's query column).
fn table_column_width(area_width: u16) -> usize {
    let fixed: u16 = FIXED_WIDTHS.iter().sum::<u16>() + SEVERITY_WIDTH;
    let overhead = 2 /* block borders */ + HIGHLIGHT_WIDTH + fixed + 8 * COLUMN_SPACING;
    usize::from(area_width.saturating_sub(overhead))
}

/// `db: shop · 4 tables · collected 12s ago · ESTIMATED bloat` — which
/// database (the lens is per-database), how fresh the slow collection is,
/// and either the mandatory bloat-estimate label or how to get one: an
/// `idx_scan = 0`-style claim means nothing if counters were zeroed five
/// minutes ago (PRD pillar 6).
fn draw_footer(app: &App, schema: &SchemaSnapshot, frame: &mut Frame, area: Rect) {
    let staleness_secs =
        (pg_lens_core::history::epoch_ms_now().saturating_sub(schema.collected_at_epoch_ms))
            / 1_000;
    // Bloat is on-demand (its queries are slow): the auto cadence refreshes
    // only the table stats, so the footer says how to get bloat, or that the
    // shown estimate is on-demand.
    let bloat_note = if schema.table_bloat.is_empty() && schema.index_bloat.is_empty() {
        "R: estimate bloat (slow, on-demand)"
    } else {
        "ESTIMATED bloat (needs fresh ANALYZE) \u{b7} R: re-estimate"
    };
    let line = Line::from(format!(
        " db: {db} \u{b7} {n} tables \u{b7} collected {staleness_secs}s ago \u{b7} {bloat_note}",
        db = app.snapshot.vitals.database,
        n = schema.tables.len(),
    ))
    .dim();
    frame.render_widget(Paragraph::new(line), area);
}

/// Detail panel over the lower part of the lens: full vacuum/analyze stats,
/// size breakdown, and the table's btree indexes with their estimated bloat
/// (matched through `BloatRow::table`).
fn draw_detail(app: &App, schema: &SchemaSnapshot, frame: &mut Frame, area: Rect) {
    let Some(table) = app.selected_table() else {
        return;
    };
    let now = now_epoch_secs();

    let [_, panel_area] =
        Layout::vertical([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(area);

    let title = format!(
        "Table \u{2014} {}.{} (Enter/Esc: close)",
        table.schema, table.name
    );

    // Dim key, bold value (style::kv) — consistent with the Macro vitals.
    let mut lines = vec![
        style::kv(
            "size: ",
            format!(
                "total {} \u{b7} table {} \u{b7} indexes {}",
                format::human_bytes(table.total_bytes),
                format::human_bytes(table.table_bytes),
                format::human_bytes(table.index_bytes),
            ),
        ),
        style::kv(
            "tuples: ",
            format!(
                "live {} \u{b7} dead {} \u{b7} mod since analyze {} \u{b7} ins since vacuum {}",
                format::human_count(table.n_live_tup),
                format::human_count(table.n_dead_tup),
                format::human_count(table.n_mod_since_analyze),
                format::human_count(table.n_ins_since_vacuum),
            ),
        ),
        style::kv(
            "vacuum:  ",
            format!(
                "manual {} (x{}) \u{b7} auto {} (x{})",
                format::human_ago(table.last_vacuum_epoch_secs, now),
                table.vacuum_count,
                format::human_ago(table.last_autovacuum_epoch_secs, now),
                table.autovacuum_count,
            ),
        ),
        style::kv(
            "analyze: ",
            format!(
                "manual {} (x{}) \u{b7} auto {} (x{})",
                format::human_ago(table.last_analyze_epoch_secs, now),
                table.analyze_count,
                format::human_ago(table.last_autoanalyze_epoch_secs, now),
                table.autoanalyze_count,
            ),
        ),
        style::kv(
            "estimated table bloat: ",
            bloat_summary(find_table_bloat(schema, table)),
        ),
        Line::from("indexes (estimated btree bloat):").style(style::label_style()),
    ];

    let mut any_index = false;
    for index in schema.index_bloat.iter().filter(|b| {
        b.schema == table.schema && b.table.as_deref() == Some(table.name.as_str())
    }) {
        any_index = true;
        lines.push(Line::from(format!(
            "  {} \u{2014} {} \u{b7} bloat {}",
            index.name,
            format::human_bytes(index.real_bytes),
            bloat_summary(Some(index)),
        )));
    }
    if !any_index {
        lines.push(Line::from("  (no btree index bloat estimates for this table)").dim());
    }

    let panel = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(Block::bordered().title(title));
    frame.render_widget(Clear, panel_area);
    frame.render_widget(panel, panel_area);
}

/// `54.0% (96.6 MB, fillfactor 100)` — or `~?` when not applicable/missing.
fn bloat_summary(bloat: Option<&BloatRow>) -> String {
    match bloat {
        Some(b) if !b.is_na => {
            let pct = b
                .bloat_pct
                .map_or_else(|| NO_ESTIMATE.to_string(), |p| format!("{p:.1}%"));
            let bytes = b
                .bloat_bytes
                .map_or_else(|| NO_ESTIMATE.to_string(), format::human_bytes);
            match b.fillfactor {
                Some(ff) => format!("{pct} ({bytes}, fillfactor {ff})"),
                None => format!("{pct} ({bytes})"),
            }
        }
        Some(_) => format!("{NO_ESTIMATE} (estimate not applicable)"),
        None => format!("{NO_ESTIMATE} (no estimate)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bloat(pct: Option<f64>, bytes: Option<i64>, is_na: bool) -> BloatRow {
        BloatRow {
            schema: "public".to_string(),
            name: "t".to_string(),
            table: None,
            real_bytes: 0,
            bloat_bytes: bytes,
            bloat_pct: pct,
            fillfactor: Some(100),
            is_na,
        }
    }

    #[test]
    fn severity_tiers_match_the_plan() {
        // Red needs BOTH > 50% and > 10 MB.
        let red = bloat(Some(53.9), Some(20 << 20), false);
        assert_eq!(severity(Some(&red)), Severity::Red);
        let high_pct_small = bloat(Some(80.0), Some(512 << 10), false);
        assert_eq!(severity(Some(&high_pct_small)), Severity::None);
        // Yellow: > 30% and > 1 MB, under the red bar.
        let yellow = bloat(Some(35.7), Some(5 << 20), false);
        assert_eq!(severity(Some(&yellow)), Severity::Yellow);
        // Red wins where both tiers match (by construction: checked first).
        let both = bloat(Some(60.0), Some(50 << 20), false);
        assert_eq!(severity(Some(&both)), Severity::Red);
        // Healthy, is_na, and missing rows.
        let healthy = bloat(Some(4.2), Some(23 << 20), false);
        assert_eq!(severity(Some(&healthy)), Severity::None);
        let na = bloat(None, None, true);
        assert_eq!(severity(Some(&na)), Severity::NotApplicable);
        assert_eq!(severity(None), Severity::None);
    }

    #[test]
    fn table_width_shrinks_with_the_terminal_and_never_underflows() {
        // 120 cols: 120 - (2 + 2 + 60 + 8) = 48 chars for the table name.
        assert_eq!(table_column_width(120), 48);
        assert!(table_column_width(80) < table_column_width(120));
        assert_eq!(table_column_width(10), 0);
    }

    #[test]
    fn bloat_summary_never_invents_numbers() {
        assert_eq!(
            bloat_summary(Some(&bloat(Some(53.98), Some(101_318_656), false))),
            "54.0% (96.6 MB, fillfactor 100)"
        );
        assert_eq!(
            bloat_summary(Some(&bloat(Some(1.0), Some(1), true))),
            "~? (estimate not applicable)"
        );
        assert_eq!(bloat_summary(None), "~? (no estimate)");
    }
}
