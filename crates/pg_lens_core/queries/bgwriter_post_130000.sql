-- Checkpointer / bgwriter (F4), PG 13-16: `pg_stat_bgwriter` still carries
-- both the checkpointer's counters and the background writer's. Columns are
-- aliased to the SAME names the PG17+ variant produces (bgwriter_post_170000.sql)
-- so one Rust parser (`db::bgwriter_from_row`) serves both shapes. All
-- cumulative — the poller derives per-tick rates, never SQL.
--   * checkpoint_write_time / checkpoint_sync_time are `double precision`
--     milliseconds already (unlike EXTRACT(epoch...), no type trap here).
--   * buffers_backend exists on this range (moves to pg_stat_io on 17+, so
--     the PG17+ variant sends NULL and the model carries it as Option).
SELECT
      checkpoints_timed::int8 AS checkpoints_timed,
      checkpoints_req::int8 AS checkpoints_req,
      checkpoint_write_time::float8 AS checkpoint_write_time_ms,
      checkpoint_sync_time::float8 AS checkpoint_sync_time_ms,
      buffers_checkpoint::int8 AS buffers_checkpoint,
      buffers_clean::int8 AS buffers_clean,
      maxwritten_clean::int4 AS maxwritten_clean,
      buffers_backend::int8 AS buffers_backend,
      buffers_alloc::int8 AS buffers_alloc
 FROM pg_stat_bgwriter;
