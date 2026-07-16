-- Checkpointer / bgwriter (F4), PG 17+: the checkpoint counters moved into
-- their own `pg_stat_checkpointer` view; `pg_stat_bgwriter` kept only the
-- background-writer-proper counters (buffers_clean, maxwritten_clean,
-- buffers_alloc). `buffers_backend` moved to `pg_stat_io` (backend_type x
-- context, out of scope for this cheap single-row query) — sent as a typed
-- NULL so the same parser as bgwriter_post_130000.sql produces `None` for it.
-- Column names are aliased to match the PG13-16 variant exactly.
SELECT
      c.num_timed::int8 AS checkpoints_timed,
      c.num_requested::int8 AS checkpoints_req,
      c.write_time::float8 AS checkpoint_write_time_ms,
      c.sync_time::float8 AS checkpoint_sync_time_ms,
      c.buffers_written::int8 AS buffers_checkpoint,
      b.buffers_clean::int8 AS buffers_clean,
      b.maxwritten_clean::int4 AS maxwritten_clean,
      NULL::int8 AS buffers_backend,
      b.buffers_alloc::int8 AS buffers_alloc
 FROM pg_stat_checkpointer c, pg_stat_bgwriter b;
