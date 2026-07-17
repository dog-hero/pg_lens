// Unit tests for the idle-session severity thresholds + oldest-suspect
// finder — mirrors the Rust core's idle_sessions.rs test suite so both
// implementations stay in lockstep (same runner setup as
// prepared_xacts.test.ts: node:test, no framework).

import { test } from "node:test";
import assert from "node:assert/strict";

import { idleSessionSeverity, oldestIdleSession } from "./idle_sessions.ts";
import type { IdleSessionRow } from "./types.ts";

function row(pid: number, idleAgeSecs: number): IdleSessionRow {
  return {
    pid,
    application_name: "app",
    database: "shop",
    client: "10.0.0.1",
    username: "app_rw",
    idle_age_secs: idleAgeSecs,
  };
}

test("severity tiers match the thresholds", () => {
  assert.equal(idleSessionSeverity(0), "");
  assert.equal(idleSessionSeverity(1_800), "", "boundary is not yet warn");
  assert.equal(idleSessionSeverity(1_800.1), "warn");
  assert.equal(idleSessionSeverity(14_400), "warn", "boundary is not yet bad");
  assert.equal(idleSessionSeverity(14_400.1), "bad");
  assert.equal(idleSessionSeverity(86_400), "bad");
});

test("oldestIdleSession picks the largest age", () => {
  const rows = [row(1, 100), row(2, 20_000), row(3, 500)];
  const oldest = oldestIdleSession(rows);
  assert.equal(oldest?.pid, 2);
});

test("oldestIdleSession is undefined when empty", () => {
  assert.equal(oldestIdleSession([]), undefined);
});
