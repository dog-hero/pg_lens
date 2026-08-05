-- Table Lens (v0.15): full `CREATE INDEX` statements of this table's
-- indexes, fetched fresh ON-DEMAND when the detail overlay opens. The Index
-- Lens already carries `indexdef` (`queries/indexes.sql`), but that copy is
-- slow-cadence and covers only the top `INDEXES_LIMIT` indexes cluster-wide
-- by size — this query is per-table, complete, and coherent with the rest
-- of the on-demand `\d` detail (fetched in the same request).
--
-- Bind param: $1 = the table's pg_class.oid, bound as int8 (see
-- `table_detail_columns.sql`'s header for why the COLUMN is cast to
-- `::int8` rather than the param to `::oid`).
SELECT
    ic.relname::text AS name,
    pg_get_indexdef(ix.indexrelid) AS definition
FROM pg_index ix
JOIN pg_class ic ON ic.oid = ix.indexrelid
WHERE ix.indrelid::int8 = $1
ORDER BY ic.relname;
