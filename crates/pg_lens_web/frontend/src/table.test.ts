// Unit tests for the Micro Lens activity table's column color system
// and duration-only time coloring — mirrors the TUI's tests in
// crates/pg_lens_tui/src/ui/micro_lens.rs so both stay in lockstep.

import { test } from "node:test";
import assert from "node:assert/strict";

import {
  ROW_DURATION_BAD_SECS,
  ROW_DURATION_WARN_SECS,
  stateColorClass,
  durationSeverityClass,
  waitEventClass,
} from "./table.ts";
import type { ActivityRow } from "./types.ts";

function row(state: string, duration_secs: number): ActivityRow {
  return {
    pid: 1,
    application_name: "app",
    database: "db",
    client: "10.0.0.1",
    duration_secs,
    xact_age_secs: null,
    wait_event: null,
    username: "u",
    state,
    query: "SELECT 1",
    query_leader_pid: 1,
    is_parallel_worker: false,
    query_id: null,
    ssl: false,
    ssl_version: null,
    ssl_cipher: null,
  };
}

test("stateColorClass maps every known state, neutral default for unknown", () => {
  assert.equal(stateColorClass("active"), "state-active");
  assert.equal(stateColorClass("idle"), "state-idle");
  assert.equal(stateColorClass("idle in transaction"), "state-idle-txn");
  assert.equal(
    stateColorClass("idle in transaction (aborted)"),
    "state-idle-txn-aborted",
  );
  assert.equal(stateColorClass("fastpath function call"), "");
  assert.equal(stateColorClass("disabled"), "");
});

test("durationSeverityClass applies time-based coloring only to active sessions", () => {
  assert.equal(
    durationSeverityClass("active", ROW_DURATION_BAD_SECS + 0.1),
    "duration-bad",
  );
  assert.equal(
    durationSeverityClass("active", ROW_DURATION_WARN_SECS + 0.1),
    "duration-warn",
  );
  // At/below warn: duration-ok
  assert.equal(
    durationSeverityClass("active", ROW_DURATION_WARN_SECS),
    "duration-ok",
  );
  // Same duration on an idle(-in-transaction) session stays dim/idle
  assert.equal(
    durationSeverityClass("idle", ROW_DURATION_BAD_SECS + 10_000),
    "duration-idle",
  );
  assert.equal(
    durationSeverityClass("idle in transaction", ROW_DURATION_BAD_SECS + 10_000),
    "duration-idle",
  );
});

test("waitEventClass highlights locks and other wait events", () => {
  assert.equal(waitEventClass("Lock:relation"), "wait-lock");
  assert.equal(waitEventClass("IO:DataFileRead"), "wait-other");
  assert.equal(waitEventClass("Client:ClientRead"), "wait-other");
  assert.equal(waitEventClass(null), "wait-none");
});

test("ActivityRow supports SSL encryption flags", () => {
  const r = row("active", 5);
  r.ssl = true;
  r.ssl_version = "TLSv1.3";
  r.ssl_cipher = "TLS_AES_256_GCM_SHA384";
  assert.equal(r.ssl, true);
  assert.equal(r.ssl_version, "TLSv1.3");
  assert.equal(r.ssl_cipher, "TLS_AES_256_GCM_SHA384");
});
