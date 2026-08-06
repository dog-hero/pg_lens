// Unit tests for the web copy-to-clipboard helper (v0.16, Part C) — this
// runner has no DOM (see package.json's `node --test src/*.test.ts`, no
// jsdom dependency), so only the environment-guard path of `copyText` is
// exercised here (both `navigator` and `document` are `undefined` under
// plain Node — the DOM-backed paths are exercised manually per the task's
// live-check, not asserted on here).

import { test } from "node:test";
import assert from "node:assert/strict";

import { copyText } from "./clipboard.ts";

test("copyText resolves false gracefully with no DOM/clipboard API present", async () => {
  // Guards against a crash (not a hang, not a throw) when neither
  // `navigator.clipboard` nor `document` exist — the exact environment this
  // test runs in, and a reasonable proxy for "clipboard API unavailable" in
  // a real browser too.
  const ok = await copyText("SELECT 1");
  assert.equal(ok, false);
});
