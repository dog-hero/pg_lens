-- Table Lens (v0.15): the table's own constraints (PK/FK/UNIQUE/CHECK/
-- EXCLUDE) PLUS foreign keys on OTHER tables that point back at it
-- ("referenced by") — one UNION ALL so the poller runs a single query
-- instead of two. `pg_get_constraintdef` reproduces the exact DDL clause,
-- verbatim, the same way psql's \d does — never reconstructed from parts.
--
-- `referencing_table` is NULL for the table's own constraints; for the
-- second half (rows where confrelid = this table) it names the OTHER
-- table the constraint lives on, so the frontend can render "referenced by
-- <table>.<constraint>" instead of a bare constraint name.
--
-- Type notes: conname is `name` — cast ::text; contype is `char` — cast
-- ::text so tokio-postgres maps it as a String (mapped to a friendly label
-- in `db::table_detail_constraint_from_row`, never surfaced raw).
--
-- Bind param: $1 = the table's pg_class.oid, bound as int8 (see
-- `table_detail_columns.sql`'s header for why the COLUMN is cast to
-- `::int8` rather than the param to `::oid` — the latter makes
-- tokio-postgres's client-side type check reject an `i64` binding).
SELECT
    c.conname::text AS name,
    c.contype::text AS contype,
    pg_get_constraintdef(c.oid) AS definition,
    NULL::text AS referencing_table
FROM pg_constraint c
WHERE c.conrelid::int8 = $1

UNION ALL

SELECT
    c.conname::text AS name,
    c.contype::text AS contype,
    pg_get_constraintdef(c.oid) AS definition,
    rc.relname::text AS referencing_table
FROM pg_constraint c
JOIN pg_class rc ON rc.oid = c.conrelid
WHERE c.confrelid::int8 = $1
  AND c.contype = 'f';
