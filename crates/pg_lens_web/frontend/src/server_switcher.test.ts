import { test } from "node:test";
import assert from "node:assert/strict";

import { serverOptionLabel, hasSwitchableServers } from "./server_switcher.ts";
import type { ServiceSummary } from "./actions.ts";

test("serverOptionLabel formats name and connection coordinates", () => {
  const server: ServiceSummary = {
    name: "prod",
    host: "10.0.0.1",
    port: 5432,
    user: "postgres",
    dbname: "app",
  };
  assert.equal(serverOptionLabel(server), "prod (postgres@10.0.0.1:5432)");
});

test("serverOptionLabel falls back to defaults for missing coordinates", () => {
  const server: ServiceSummary = {
    name: "local",
    host: null,
    port: null,
    user: null,
    dbname: null,
  };
  assert.equal(serverOptionLabel(server), "local (-@localhost:5432)");
});

test("hasSwitchableServers is false when servers is null or less than 2", () => {
  assert.equal(hasSwitchableServers(null), false);
  assert.equal(hasSwitchableServers([]), false);
  assert.equal(
    hasSwitchableServers([
      { name: "prod", host: null, port: null, user: null, dbname: null },
    ]),
    false,
  );
});

test("hasSwitchableServers is true when 2 or more servers are present", () => {
  assert.equal(
    hasSwitchableServers([
      { name: "prod", host: null, port: null, user: null, dbname: null },
      { name: "staging", host: null, port: null, user: null, dbname: null },
    ]),
    true,
  );
});

