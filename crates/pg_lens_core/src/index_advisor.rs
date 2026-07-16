//! Index advisor (F3): unused / duplicate / prefix-redundant index
//! detection, computed purely in Rust over the catalog facts fetched by
//! `queries/indexes.sql` — no SQL-side comparison logic, so the rules are
//! unit-testable independent of any live database.
//!
//! PRD pillar 6 ("signal, not verdict"): this module never deletes or
//! recommends an action — it stamps each index with a [`crate::models::IndexFinding`]
//! and lets the operator decide, seeing the underlying evidence (scans,
//! size, indexdef, stats freshness) alongside the flag.

use crate::models::{IndexFinding, IndexRow};

/// The raw catalog facts for one index, as parsed from `queries/indexes.sql`
/// — a strict superset of [`IndexRow`]: the `ind*` columns are opaque
/// catalog signatures used only to classify duplicates, never shown to a
/// frontend (they carry no meaning beyond "same or not").
#[derive(Clone, Debug)]
pub struct IndexCatalogRow {
    pub schema: String,
    pub table: String,
    pub name: String,
    pub index_bytes: i64,
    pub idx_scan: i64,
    pub idx_tup_read: i64,
    pub idx_tup_fetch: i64,
    pub is_unique: bool,
    pub is_primary: bool,
    pub is_exclusion: bool,
    pub is_constraint: bool,
    pub indexdef: String,
    /// `pg_index.indkey::text` — space-separated attribute numbers, in
    /// index column order (`"0"` marks an expression column).
    pub indkey: String,
    /// `pg_index.indclass::text` — space-separated operator-class OIDs.
    pub indclass: String,
    /// `pg_index.indcollation::text` — space-separated collation OIDs.
    pub indcollation: String,
    /// `pg_get_expr(indpred, indrelid)`, `""` for a non-partial index.
    pub indpred: String,
}

/// An index serves a constraint (PK/UNIQUE/EXCLUDE) — it exists to enforce
/// data integrity, not to speed up reads, so a zero scan count is expected
/// and must never be flagged [`IndexFinding::Unused`]. Matches the plan's
/// "NOT unique/primary/exclusion" rule verbatim (`is_constraint` is kept on
/// the row for display/debugging but is deliberately NOT part of this gate:
/// `indisunique`/`indisprimary`/`indisexclusion` are pg_index's own
/// ground truth and cover manually created unique indexes with no
/// pg_constraint row too).
fn serves_a_constraint(row: &IndexCatalogRow) -> bool {
    row.is_unique || row.is_primary || row.is_exclusion
}

/// Whitespace-tokenized comparison key so `"1 2"` and `"1  2"` (shouldn't
/// happen from Postgres, but cheap to be defensive about) compare equal.
fn tokens(s: &str) -> Vec<&str> {
    s.split_whitespace().collect()
}

/// Exact-duplicate signature: same column list (order matters — a btree on
/// `(a, b)` is not interchangeable with one on `(b, a)`), same opclasses,
/// same collations, same partial predicate, same uniqueness semantics (a
/// UNIQUE and a non-unique index over identical columns enforce different
/// things and are NOT interchangeable, so they must not be flagged as
/// exact duplicates of each other).
fn exact_signature(row: &IndexCatalogRow) -> (Vec<&str>, Vec<&str>, Vec<&str>, &str, bool) {
    (
        tokens(&row.indkey),
        tokens(&row.indclass),
        tokens(&row.indcollation),
        row.indpred.as_str(),
        row.is_unique,
    )
}

/// `a` is a strict, column-order-respecting prefix of `b`: every token of
/// `a` appears at the same position in `b`, and `b` has more columns.
fn is_strict_prefix<'a>(a: &[&'a str], b: &[&'a str]) -> bool {
    a.len() < b.len() && a.iter().zip(b.iter()).all(|(x, y)| x == y)
}

/// Classifies every row of one collection, positionally aligned with the
/// input slice (`classify(rows)[i]` describes `rows[i]`).
///
/// Priority when more than one signal applies to the same index: `Unused`
/// (red, the strongest and cheapest-to-verify claim) wins over `Duplicate*`
/// (yellow/dim-yellow) — a single flag per row keeps the severity-then-size
/// sort in the UI unambiguous.
pub fn classify(rows: &[IndexCatalogRow]) -> Vec<IndexFinding> {
    let mut findings = vec![IndexFinding::None; rows.len()];

    // Exact duplicates: group indices of the same table by exact_signature.
    for i in 0..rows.len() {
        if findings[i] != IndexFinding::None {
            continue;
        }
        for j in 0..rows.len() {
            if i == j || rows[i].table != rows[j].table || rows[i].schema != rows[j].schema {
                continue;
            }
            if exact_signature(&rows[i]) == exact_signature(&rows[j]) {
                findings[i] = IndexFinding::DuplicateExact {
                    partner: rows[j].name.clone(),
                };
                break;
            }
        }
    }

    // Prefix-redundant: only for indexes not already an exact duplicate, and
    // only when the (narrower) candidate does not itself serve a constraint
    // — dropping a unique/primary/exclusion index loses a guarantee the
    // wider superset index does not provide, so it is never the "redundant"
    // side of a prefix pair.
    for i in 0..rows.len() {
        if findings[i] != IndexFinding::None || serves_a_constraint(&rows[i]) {
            continue;
        }
        let a_key = tokens(&rows[i].indkey);
        for j in 0..rows.len() {
            if i == j || rows[i].table != rows[j].table || rows[i].schema != rows[j].schema {
                continue;
            }
            if rows[i].indpred != rows[j].indpred {
                continue;
            }
            let b_key = tokens(&rows[j].indkey);
            if is_strict_prefix(&a_key, &b_key)
                && tokens(&rows[i].indclass) == b_key_prefix(&rows[j].indclass, a_key.len())
                && tokens(&rows[i].indcollation) == b_key_prefix(&rows[j].indcollation, a_key.len())
            {
                findings[i] = IndexFinding::DuplicatePrefix {
                    partner: rows[j].name.clone(),
                };
                break;
            }
        }
    }

    // Unused overrides any duplicate flag found above — the strongest signal
    // wins so the UI shows exactly one marker per row.
    for (i, row) in rows.iter().enumerate() {
        if row.idx_scan == 0 && !serves_a_constraint(row) {
            findings[i] = IndexFinding::Unused;
        }
    }

    findings
}

/// First `len` whitespace tokens of `s` — used to compare opclass/collation
/// only over the overlapping prefix range of a wider index.
fn b_key_prefix(s: &str, len: usize) -> Vec<&str> {
    tokens(s).into_iter().take(len).collect()
}

/// Parses catalog rows into the final, serializable [`IndexRow`]s, stamping
/// each with its [`classify`] finding. The only place a `Vec<IndexCatalogRow>`
/// gets consumed — callers (the poller, and `SchemaSnapshot::mock`) never
/// see the raw catalog signature fields.
pub fn build_index_rows(rows: Vec<IndexCatalogRow>) -> Vec<IndexRow> {
    let findings = classify(&rows);
    rows.into_iter()
        .zip(findings)
        .map(|(row, finding)| IndexRow {
            schema: row.schema,
            table: row.table,
            name: row.name,
            index_bytes: row.index_bytes,
            idx_scan: row.idx_scan,
            idx_tup_read: row.idx_tup_read,
            idx_tup_fetch: row.idx_tup_fetch,
            is_unique: row.is_unique,
            is_primary: row.is_primary,
            is_exclusion: row.is_exclusion,
            is_constraint: row.is_constraint,
            indexdef: row.indexdef,
            finding,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        table: &str,
        name: &str,
        idx_scan: i64,
        is_unique: bool,
        is_primary: bool,
        indkey: &str,
    ) -> IndexCatalogRow {
        IndexCatalogRow {
            schema: "public".to_string(),
            table: table.to_string(),
            name: name.to_string(),
            index_bytes: 1_048_576,
            idx_scan,
            idx_tup_read: 0,
            idx_tup_fetch: 0,
            is_unique,
            is_primary,
            is_exclusion: false,
            is_constraint: is_unique || is_primary,
            indexdef: format!("CREATE INDEX {name} ON {table} USING btree (...)"),
            indkey: indkey.to_string(),
            indclass: indkey.split_whitespace().map(|_| "1978").collect::<Vec<_>>().join(" "),
            indcollation: indkey.split_whitespace().map(|_| "0").collect::<Vec<_>>().join(" "),
            indpred: String::new(),
        }
    }

    #[test]
    fn unused_flags_a_never_scanned_plain_index() {
        let rows = vec![row("orders", "orders_customer_idx", 0, false, false, "2")];
        assert_eq!(classify(&rows), vec![IndexFinding::Unused]);
    }

    #[test]
    fn unused_never_flags_unique_primary_or_exclusion_indexes() {
        let rows = vec![
            row("orders", "orders_pkey", 0, true, true, "1"),
            row("orders", "orders_email_key", 0, true, false, "3"),
        ];
        let findings = classify(&rows);
        assert_eq!(findings, vec![IndexFinding::None, IndexFinding::None]);
    }

    #[test]
    fn scanned_plain_index_is_not_flagged_unused() {
        let rows = vec![row("orders", "orders_customer_idx", 42, false, false, "2")];
        assert_eq!(classify(&rows), vec![IndexFinding::None]);
    }

    #[test]
    fn exact_duplicates_flag_each_other() {
        let rows = vec![
            row("orders", "orders_customer_idx", 10, false, false, "2"),
            row("orders", "orders_customer_idx2", 5, false, false, "2"),
        ];
        let findings = classify(&rows);
        assert_eq!(
            findings[0],
            IndexFinding::DuplicateExact {
                partner: "orders_customer_idx2".to_string()
            }
        );
        assert_eq!(
            findings[1],
            IndexFinding::DuplicateExact {
                partner: "orders_customer_idx".to_string()
            }
        );
    }

    #[test]
    fn unique_and_nonunique_over_the_same_columns_are_not_exact_duplicates() {
        let rows = vec![
            row("orders", "orders_email_key", 10, true, false, "3"),
            row("orders", "orders_email_idx", 5, false, false, "3"),
        ];
        // Different table would obviously not match either; same table but
        // different uniqueness semantics must also not match.
        assert_eq!(classify(&rows), vec![IndexFinding::None, IndexFinding::None]);
    }

    #[test]
    fn different_tables_never_cross_match() {
        let rows = vec![
            row("orders", "orders_a", 10, false, false, "2"),
            row("line_items", "line_items_a", 5, false, false, "2"),
        ];
        assert_eq!(classify(&rows), vec![IndexFinding::None, IndexFinding::None]);
    }

    #[test]
    fn prefix_redundant_flags_the_narrower_index_only() {
        let rows = vec![
            row("orders", "orders_customer_idx", 3, false, false, "2"),
            row("orders", "orders_customer_created_idx", 900, false, false, "2 5"),
        ];
        let findings = classify(&rows);
        assert_eq!(
            findings[0],
            IndexFinding::DuplicatePrefix {
                partner: "orders_customer_created_idx".to_string()
            }
        );
        // The wider index is not itself redundant.
        assert_eq!(findings[1], IndexFinding::None);
    }

    #[test]
    fn prefix_check_never_flags_a_unique_or_primary_narrower_index() {
        let rows = vec![
            row("orders", "orders_pkey", 900, true, true, "1"),
            row("orders", "orders_id_created_idx", 5, false, false, "1 5"),
        ];
        // The PK enforces uniqueness the wider index does not — must not be
        // flagged as redundant even though its column list is a prefix.
        assert_eq!(classify(&rows)[0], IndexFinding::None);
    }

    #[test]
    fn non_prefix_column_order_does_not_match() {
        // (5) is not a prefix of (2, 5) — order matters for btree indexes.
        let rows = vec![
            row("orders", "orders_created_idx", 3, false, false, "5"),
            row("orders", "orders_customer_created_idx", 900, false, false, "2 5"),
        ];
        assert_eq!(classify(&rows), vec![IndexFinding::None, IndexFinding::None]);
    }

    #[test]
    fn unused_wins_over_a_duplicate_flag_on_the_same_row() {
        let rows = vec![
            row("orders", "orders_dup_a", 0, false, false, "2"),
            row("orders", "orders_dup_b", 10, false, false, "2"),
        ];
        let findings = classify(&rows);
        // orders_dup_a is both unused AND an exact duplicate of dup_b —
        // Unused (the stronger, red signal) must win.
        assert_eq!(findings[0], IndexFinding::Unused);
        assert_eq!(
            findings[1],
            IndexFinding::DuplicateExact {
                partner: "orders_dup_a".to_string()
            }
        );
    }

    #[test]
    fn build_index_rows_strips_raw_catalog_fields_and_keeps_the_finding() {
        let rows = vec![row("orders", "orders_customer_idx", 0, false, false, "2")];
        let built = build_index_rows(rows);
        assert_eq!(built.len(), 1);
        assert_eq!(built[0].name, "orders_customer_idx");
        assert_eq!(built[0].finding, IndexFinding::Unused);
    }
}
