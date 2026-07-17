//! Index Lens (U1): the index advisor (invalid / unused / duplicate /
//! prefix-redundant indexes), promoted out of the Schema Lens's old `i`
//! toggle into its own full-height tab — same table, same fixed
//! severity-then-size order, same detail panel, just with the room the
//! owner asked for.
//!
//! Row semantics (unchanged from the Schema Lens precedent):
//! - `INVALID` red (failed `CREATE INDEX CONCURRENTLY`), `!!` red (Unused),
//!   `DUP` yellow (exact duplicate), `pre` dim-yellow (prefix-redundant), or
//!   blank — the Flag column's textual marker, provable in VT captures
//!   without color;
//! - `Enter` opens a detail panel: the full `CREATE INDEX` statement, usage
//!   counters, constraint flags, and the finding spelled out with its
//!   duplicate partner (if any) — evidence, not a bare label;
//! - the footer names the database, the row count, the slow collection's
//!   staleness, and the stats-reset age: an `idx_scan = 0` claim means
//!   nothing if counters were zeroed five minutes ago (PRD pillar 6).

use std::time::{SystemTime, UNIX_EPOCH};

use pg_lens_core::{IndexFinding, IndexRow, SchemaSnapshot, SchemaStatus};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, Row, Table, Wrap},
};

use crate::app::App;
use crate::ui::{format, style};

/// Highlight symbol "▶ " rendered left of the selected row.
const HIGHLIGHT_WIDTH: u16 = 2;
const COLUMN_SPACING: u16 = 1;

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
        .block(Block::bordered().title("Indexes"));
        frame.render_widget(placeholder, area);
        return;
    };

    let error_height = match schema.status {
        SchemaStatus::Ok => 0,
        SchemaStatus::Error(_) => 1,
    };
    let [table_area, error_area, footer_area] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(error_height),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_table(app, &schema, frame, table_area);
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

/// `INVALID` red (failed concurrent build), `!!` red (Unused), `DUP` yellow
/// (exact duplicate), `pre` dim-yellow (prefix-redundant), or blank —
/// mirrors the Schema Lens's bloat-severity convention (provable in VT
/// captures without color). Ranked by [`crate::app::index_finding_rank`].
fn index_marker_and_style(finding: &IndexFinding) -> (&'static str, Style) {
    match finding {
        IndexFinding::Invalid => ("INVALID", Style::new().fg(Color::Red).bold()),
        IndexFinding::Unused => ("UNUSED", Style::new().fg(Color::Red).bold()),
        IndexFinding::DuplicateExact { .. } => ("DUP", Style::new().fg(Color::Yellow)),
        IndexFinding::DuplicatePrefix { .. } => ("prefix", Style::new().fg(Color::Yellow).dim()),
        IndexFinding::None => ("", Style::new()),
    }
}

/// Fixed widths of every column except the flexible Index one, in order:
/// Table, Size, Scans, Tup Read, Flag.
const FIXED_WIDTHS: [u16; 5] = [16, 9, 8, 9, 7];

fn draw_table(app: &mut App, schema: &SchemaSnapshot, frame: &mut Frame, area: Rect) {
    let header = Row::new(["Index", "Table", "Size", "Scans", "Tup Read", "Flag"])
        .style(Style::new().bold());

    let index_width = index_column_width(area.width);

    let rows = app
        .index_row_order
        .iter()
        .filter_map(|&i| schema.indexes.get(i))
        .map(|idx| {
            let (marker, style) = index_marker_and_style(&idx.finding);
            Row::new([
                format::truncate_with_ellipsis(&idx.name, index_width),
                format::truncate_with_ellipsis(&idx.table, FIXED_WIDTHS[0] as usize),
                format::human_bytes(idx.index_bytes),
                format::human_count(idx.idx_scan),
                format::human_count(idx.idx_tup_read),
                marker.to_string(),
            ])
            .style(style)
        });

    let widths = [
        Constraint::Min(8),
        Constraint::Length(FIXED_WIDTHS[0]),
        Constraint::Length(FIXED_WIDTHS[1]),
        Constraint::Length(FIXED_WIDTHS[2]),
        Constraint::Length(FIXED_WIDTHS[3]),
        Constraint::Length(FIXED_WIDTHS[4]),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::bordered().title("Indexes"))
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_symbol("\u{25b6} ");

    frame.render_stateful_widget(table, area, &mut app.index_table_state);
}

/// How many characters the flexible Index column can hold at this terminal
/// width (same arithmetic as the Schema Lens's table column).
fn index_column_width(area_width: u16) -> usize {
    let fixed: u16 = FIXED_WIDTHS.iter().sum();
    let overhead = 2 /* block borders */ + HIGHLIGHT_WIDTH + fixed + 5 * COLUMN_SPACING;
    usize::from(area_width.saturating_sub(overhead))
}

/// `db: shop · 7 indexes · collected 12s ago · stats reset 12d ago` — which
/// database (the lens is per-database), how fresh the slow collection is,
/// and the stats-reset age: an `idx_scan = 0` UNUSED claim means nothing if
/// counters were zeroed five minutes ago (PRD pillar 6).
fn draw_footer(app: &App, schema: &SchemaSnapshot, frame: &mut Frame, area: Rect) {
    let staleness_secs =
        (pg_lens_core::history::epoch_ms_now().saturating_sub(schema.collected_at_epoch_ms))
            / 1_000;
    let now = now_epoch_secs();
    let reset_age = match schema.stats_reset_epoch_secs {
        Some(_) => format!(
            "stats reset {}",
            format::human_ago(schema.stats_reset_epoch_secs, now)
        ),
        None => "stats reset: unknown".to_string(),
    };
    let line = Line::from(format!(
        " db: {db} \u{b7} {n} indexes \u{b7} collected {staleness_secs}s ago \u{b7} \
         {reset_age} \u{b7} signal, not verdict \u{2014} verify against the workload",
        db = app.snapshot.vitals.database,
        n = schema.indexes.len(),
    ))
    .dim();
    frame.render_widget(Paragraph::new(line), area);
}

/// Detail panel of the selected row: the full `CREATE INDEX` statement
/// (never reconstructed from parts), usage counters, constraint flags, and
/// — the whole point of a "signal, not verdict" advisor — the finding
/// spelled out with its duplicate partner, if any.
fn draw_detail(app: &App, schema: &SchemaSnapshot, frame: &mut Frame, area: Rect) {
    let Some(idx) = app.selected_index() else {
        return;
    };
    let now = now_epoch_secs();

    let [_, panel_area] =
        Layout::vertical([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(area);

    let title = format!(
        "Index \u{2014} {}.{} on {} (Enter/Esc: close)",
        idx.schema, idx.table, idx.name
    );

    let flags = index_flags_summary(idx);
    let mut lines = vec![
        style::kv("definition: ", idx.indexdef.clone()),
        style::kv("size: ", format::human_bytes(idx.index_bytes)),
        style::kv(
            "scans: ",
            format!(
                "{} \u{b7} tuples read {} \u{b7} tuples fetched {}",
                format::human_count(idx.idx_scan),
                format::human_count(idx.idx_tup_read),
                format::human_count(idx.idx_tup_fetch),
            ),
        ),
        style::kv("flags: ", flags),
        style::kv(
            "stats freshness: ",
            format!(
                "counters {}",
                format::human_ago(schema.stats_reset_epoch_secs, now)
            ),
        ),
        Line::from("finding:").style(style::label_style()),
    ];
    lines.push(index_finding_line(&idx.finding));
    lines.push(
        Line::from("  signal, not verdict \u{2014} verify against the real workload before \
                     dropping anything")
            .dim(),
    );

    let panel = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(Block::bordered().title(title));
    frame.render_widget(Clear, panel_area);
    frame.render_widget(panel, panel_area);
}

/// `unique · primary key` or `plain` — which pg_index flags apply, so the
/// detail explains WHY (if at all) `Unused` was withheld.
fn index_flags_summary(idx: &IndexRow) -> String {
    let mut flags = Vec::new();
    if idx.is_primary {
        flags.push("primary key");
    } else if idx.is_unique {
        flags.push("unique");
    }
    if idx.is_exclusion {
        flags.push("exclusion");
    }
    if idx.is_constraint && !idx.is_primary {
        flags.push("constraint-backed");
    }
    if flags.is_empty() {
        "plain (non-unique, no constraint)".to_string()
    } else {
        flags.join(" \u{b7} ")
    }
}

/// One descriptive line per finding — the detail panel's evidence, not a
/// bare label.
fn index_finding_line(finding: &IndexFinding) -> Line<'static> {
    match finding {
        IndexFinding::Invalid => Line::from(vec![
            Span::styled("  INVALID", Style::new().fg(Color::Red).bold()),
            Span::styled(
                " \u{2014} indisvalid/indisready is false: a CREATE INDEX CONCURRENTLY \
                 likely failed or was cancelled; this index is dead weight (never served \
                 to the planner, still costs every write) and can safely be dropped and \
                 rebuilt",
                style::label_style(),
            ),
        ]),
        IndexFinding::Unused => Line::from(vec![
            Span::styled("  UNUSED", Style::new().fg(Color::Red).bold()),
            Span::styled(
                " \u{2014} zero scans since the last stats reset; serves no constraint",
                style::label_style(),
            ),
        ]),
        IndexFinding::DuplicateExact { partner } => Line::from(vec![
            Span::styled("  DUP", Style::new().fg(Color::Yellow).bold()),
            Span::styled(
                format!(
                    " \u{2014} exact duplicate of \u{2018}{partner}\u{2019} (same columns, \
                     opclasses, predicate and uniqueness)"
                ),
                style::label_style(),
            ),
        ]),
        IndexFinding::DuplicatePrefix { partner } => Line::from(vec![
            Span::styled("  prefix", Style::new().fg(Color::Yellow).dim()),
            Span::styled(
                format!(
                    " \u{2014} this index's columns are a strict prefix of \u{2018}{partner}\u{2019}\
                     's; the wider index can likely serve both"
                ),
                style::label_style(),
            ),
        ]),
        IndexFinding::None => Line::from("  no finding \u{2014} in use, or uniquely useful").dim(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn index_lens_renders_the_invalid_marker_from_mock() {
        let mut app = crate::app::App::new();
        app.active_tab = crate::app::Tab::IndexLens;
        let snapshot = app.snapshot.clone();
        crate::app::update(&mut app, crate::app::Action::Snapshot(snapshot));

        let backend = ratatui::backend::TestBackend::new(120, 36);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
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
        // The mock's order_items_shipped_at_idx is indisvalid = false.
        assert!(screen.contains("INVALID"), "{screen}");
        assert!(screen.contains("order_items_shipped_at_idx"), "{screen}");
    }

    #[test]
    fn index_lens_detail_panel_explains_the_invalid_finding() {
        let mut app = crate::app::App::new();
        app.active_tab = crate::app::Tab::IndexLens;
        let snapshot = app.snapshot.clone();
        crate::app::update(&mut app, crate::app::Action::Snapshot(snapshot));

        let pos = app
            .index_row_order
            .iter()
            .filter_map(|&i| app.snapshot.schema.as_ref().map(|s| &s.indexes[i]))
            .position(|idx| idx.name == "order_items_shipped_at_idx")
            .expect("mock's invalid index is present");
        app.index_table_state.select(Some(pos));
        app.detail_open = true;

        let backend = ratatui::backend::TestBackend::new(120, 36);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
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
        assert!(screen.contains("INVALID"), "{screen}");
        assert!(
            screen.contains("CREATE INDEX CONCURRENTLY"),
            "detail panel names the likely cause: {screen}"
        );
    }
}
