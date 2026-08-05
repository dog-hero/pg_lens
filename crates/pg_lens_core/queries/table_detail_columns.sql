-- Table Lens (v0.15): `\d`-style column list for one table, fetched
-- ON-DEMAND when the Schema Lens detail overlay opens (never on the poll
-- cadence). Mirrors psql's own \d backing query: pg_attribute joined with
-- pg_attrdef for column defaults (no single system view carries both).
--
-- Type notes (tokio-postgres):
--   * attname is `name` — cast ::text for a clean String map (also true of
--     format_type's result, which is already text).
--   * pg_get_expr(...) returns `text` already; the default is NULL when the
--     column has none — kept as Option, never faked as "".
--   * attidentity/attgenerated are `char` (PG10+/PG12+, so present on every
--     supported version) — cast ::text so tokio-postgres maps them as
--     Strings, mapped to friendly labels in `db.rs`, never surfaced raw.
--     A `GENERATED ... AS IDENTITY` column has NO `pg_attrdef` row (identity
--     is tracked entirely via attidentity, unlike a plain `serial`'s
--     `nextval(...)` default, which DOES have one) — `default_expr` stays
--     NULL for those, and the identity kind carries the information instead.
--     A `GENERATED ... AS (...) STORED` column DOES have a `pg_attrdef` row
--     (the generation expression lives there) — `attgenerated = 's'` flags
--     that `default_expr` is a generation expression, not a plain default.
--
-- Bind param: $1 = the table's pg_class.oid, bound as int8 (no native oid
-- mapping in tokio-postgres — casting the PARAM itself `$1::oid` makes
-- postgres infer the parameter's wire type as `oid`, which tokio-postgres's
-- client-side type check then rejects for an `i64` binding
-- (`WrongType { postgres: Oid, rust: "i64" }`, verified against a live
-- PG16). Casting the COLUMN to `::int8` instead keeps the inferred param
-- type `int8`, matching `i64`'s `ToSql` impl.
SELECT
    a.attname::text AS name,
    format_type(a.atttypid, a.atttypmod) AS data_type,
    a.attnotnull AS not_null,
    pg_get_expr(ad.adbin, ad.adrelid) AS default_expr,
    a.attidentity::text AS identity,
    a.attgenerated::text AS generated
FROM pg_attribute a
LEFT JOIN pg_attrdef ad
    ON ad.adrelid = a.attrelid AND ad.adnum = a.attnum
WHERE a.attrelid::int8 = $1
  AND a.attnum > 0
  AND NOT a.attisdropped
ORDER BY a.attnum;
