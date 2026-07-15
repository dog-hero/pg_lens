-- Cancel the current query of the session whose backend process has the
-- specified process ID.
-- Adapted from dalibo/pg_activity: pgactivity/queries/do_pg_cancel_backend.sql
-- (the `%(pid)s` placeholder becomes tokio-postgres's `$1`).
-- Returns false when the PID does not exist; raises an error when the caller
-- lacks privilege (same user or pg_signal_backend membership required).
SELECT pg_cancel_backend($1::int4) AS is_stopped;
