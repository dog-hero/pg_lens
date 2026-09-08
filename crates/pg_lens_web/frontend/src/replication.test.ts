// Unit tests for the replication slot severity rule (F2.5) — mirrors the
// TUI's ui/macro_lens.rs `slot_severity` test suite so both implementations
// stay in lockstep (same runner setup as vacuum.test.ts: node:test, no
// framework).

import { test } from "node:test";
import assert from "node:assert/strict";

import {
  conflictsSeverity,
  conflictsText,
  publicationRow,
  renderReplication,
  slotRow,
  slotSeverity,
  subscriptionRow,
} from "./replication.ts";
import type {
  DatabaseConflicts,
  PublicationRow,
  ReplicationSlotRow,
  SubscriptionRow,
} from "./types.ts";

function slot(
  active: boolean,
  wal_status: string | null,
  retained_wal_bytes: number | null,
  xmin_age: number | null = null,
  invalidated: string | null = null,
  catalog_xmin_age: number | null = null,
): ReplicationSlotRow {
  return {
    slot_name: "probe_slot",
    slot_type: "physical",
    active,
    retained_wal_bytes,
    wal_status,
    safe_wal_size: null,
    xmin_age,
    catalog_xmin_age,
    invalidated,
  };
}

test("active reserved slot is calm, even retaining a lot", () => {
  assert.equal(slotSeverity(slot(true, "reserved", 0)), "");
  assert.equal(slotSeverity(slot(true, "reserved", 20 * 1024 * 1024 * 1024)), "");
});

test("inactive slot retaining WAL is yellow then red", () => {
  assert.equal(slotSeverity(slot(false, "extended", 0)), "", "retaining nothing stays calm");
  assert.equal(slotSeverity(slot(false, "extended", 1024)), "warn");
  assert.equal(slotSeverity(slot(false, "extended", 11 * 1024 * 1024 * 1024)), "bad");
});

test("unreserved or lost wal_status is always red", () => {
  assert.equal(slotSeverity(slot(false, "unreserved", 1024)), "bad");
  assert.equal(slotSeverity(slot(false, "lost", null)), "bad");
  assert.equal(slotSeverity(slot(true, "unreserved", 0)), "bad");
});

test("invalidated slot is always red", () => {
  assert.equal(slotSeverity(slot(true, "reserved", 0, null, "wal_removed")), "bad");
});

test("high xmin_age triggers warn and bad", () => {
  assert.equal(slotSeverity(slot(true, "reserved", 0, 15_000_000)), "warn");
  assert.equal(slotSeverity(slot(true, "reserved", 0, 60_000_000)), "bad");
});

test("conflictsSeverity: red if rate > 0, yellow if total > 0, calm otherwise", () => {
  const zeroConflicts: DatabaseConflicts = {
    datid: 12345,
    datname: "testdb",
    confl_tablespace: 0,
    confl_lock: 0,
    confl_snapshot: 0,
    confl_bufferpin: 0,
    confl_deadlock: 0,
    confl_total: 0,
    conflicts_per_sec: 0,
    lock_conflicts_per_sec: 0,
    snapshot_conflicts_per_sec: 0,
    deadlock_conflicts_per_sec: 0,
  };
  assert.equal(conflictsSeverity(zeroConflicts), "");

  const historicConflicts: DatabaseConflicts = {
    ...zeroConflicts,
    confl_total: 10,
    confl_lock: 10,
    conflicts_per_sec: 0,
  };
  assert.equal(conflictsSeverity(historicConflicts), "warn");

  const activeConflicts: DatabaseConflicts = {
    ...zeroConflicts,
    confl_total: 15,
    confl_lock: 15,
    conflicts_per_sec: 2.5,
  };
  assert.equal(conflictsSeverity(activeConflicts), "bad");
});

test("conflictsText formats breakdown and delta rates", () => {
  const c: DatabaseConflicts = {
    datid: 12345,
    datname: "testdb",
    confl_tablespace: 1,
    confl_lock: 5,
    confl_snapshot: 3,
    confl_bufferpin: 2,
    confl_deadlock: 0,
    confl_total: 11,
    conflicts_per_sec: 1.2,
    lock_conflicts_per_sec: 0.5,
    snapshot_conflicts_per_sec: 0.3,
    deadlock_conflicts_per_sec: 0,
  };
  assert.equal(
    conflictsText(c),
    "conflicts: 11 (1.2/s) · lock 5 · snapshot 3 · deadlock 0 · pin 2 · tblspc 1",
  );
});

test("high catalog_xmin_age triggers warn and bad", () => {
  assert.equal(slotSeverity(slot(true, "reserved", 0, null, null, 15_000_000)), "warn");
  assert.equal(slotSeverity(slot(true, "reserved", 0, null, null, 60_000_000)), "bad");
});

class MockElement {
  tagName: string;
  className = "";
  textContent = "";
  hidden = false;
  children: MockElement[] = [];
  classList = {
    add: (...classes: string[]) => {
      this.className = `${this.className} ${classes.join(" ")}`.trim();
    },
    contains: (cls: string) => this.className.split(/\s+/).includes(cls),
  };
  constructor(tagName: string) {
    this.tagName = tagName;
  }
  appendChild(child: MockElement) {
    this.children.push(child);
    return child;
  }
  replaceChildren(...children: MockElement[]) {
    this.children = [...children];
  }
}

// Install minimal DOM shim for render tests
(globalThis as unknown as { document: { createElement: (tag: string) => MockElement } }).document = {
  createElement: (tag: string) => new MockElement(tag),
};

test("publicationRow renders table list and operations", () => {
  const pub: PublicationRow = {
    pubname: "test_pub",
    owner: "postgres",
    all_tables: false,
    pubinsert: true,
    pubupdate: true,
    pubdelete: false,
    pubtruncate: false,
    pubviaroot: true,
    table_count: 2,
    published_tables: ["public.orders", "public.items"],
  };
  const row = publicationRow(pub);
  const text = (row as unknown as MockElement).children.map((c) => c.textContent).join(" ");
  assert.match(text, /pub test_pub/);
  assert.match(text, /ops: \[ins,upd\]/);
  assert.match(text, /\(via_root\)/);
  assert.match(text, /public\.orders, public\.items/);
});

test("subscriptionRow renders connection details, 2PC, streaming, and syncing tables", () => {
  const sub: SubscriptionRow = {
    subname: "test_sub",
    owner: "app_user",
    enabled: true,
    slot_name: "test_slot",
    publications: ["orders_pub"],
    publisher_host: "10.0.0.1",
    publisher_port: "5432",
    publisher_dbname: "orders_db",
    sync_commit: "off",
    streaming_mode: "parallel",
    binary_mode: true,
    two_phase: true,
    worker_pid: 4567,
    received_lsn: "0/18A2B00",
    last_msg_send_secs: 1.0,
    last_msg_receipt_secs: 1.0,
    latest_end_lsn: "0/18A2B00",
    latest_end_secs: 1.0,
    sync_tables: 1,
    ready_tables: 3,
    total_tables: 4,
    syncing_table_names: ["public.audit_log"],
    apply_error_count: 0,
    sync_error_count: 0,
  };
  const row = subscriptionRow(sub);
  const text = (row as unknown as MockElement).children.map((c) => c.textContent).join(" ");
  assert.match(text, /sub test_sub/);
  assert.match(text, /worker: pid 4567/);
  assert.match(text, /conn: 10\.0\.0\.1:5432\/orders_db/);
  assert.match(text, /streaming: parallel/);
  assert.match(text, /binary: on/);
  assert.match(text, /2PC: on/);
  assert.match(text, /sync_commit: off/);
  assert.match(text, /syncing tables: \[public\.audit_log\]/);
});

test("slotRow renders plugin, db, client, consumer lag, and flags", () => {
  const s: ReplicationSlotRow = {
    slot_name: "logical_orders",
    slot_type: "logical",
    plugin: "pgoutput",
    database: "orders_db",
    temporary: false,
    active: true,
    active_pid: 9876,
    application_name: "warehouse_sync",
    client_addr: "10.0.0.2",
    restart_lsn: "0/18A0000",
    confirmed_flush_lsn: "0/18A1000",
    retained_wal_bytes: 32 * 1024 * 1024,
    consumer_lag_bytes: 1024 * 1024,
    wal_status: "reserved",
    safe_wal_size: 500 * 1024 * 1024,
    xmin_age: 100,
    catalog_xmin_age: 50,
    two_phase: true,
    conflicting: false,
    invalidated: null,
  };
  const row = slotRow(s);
  const text = (row as unknown as MockElement).children.map((c) => c.textContent).join(" ");
  assert.match(text, /slot logical_orders\/logical/);
  assert.match(text, /active \(pid 9876\)/);
  assert.match(text, /plugin: pgoutput/);
  assert.match(text, /db: orders_db/);
  assert.match(text, /warehouse_sync \(10\.0\.0\.2\)/);
  assert.match(text, /lag: 1\.0 MB/);
  assert.match(text, /restart: 0\/18A0000/);
  assert.match(text, /flush: 0\/18A1000/);
  assert.match(text, /2PC: yes/);
});

test("renderReplication renders separate sections for publications, subscriptions, and slots", () => {
  const body = new MockElement("div");
  const placeholder = new MockElement("p");
  const pubs: PublicationRow[] = [
    {
      pubname: "pub_test",
      owner: "postgres",
      all_tables: true,
      pubinsert: true,
      pubupdate: true,
      pubdelete: true,
      pubtruncate: false,
      pubviaroot: false,
      table_count: 5,
      published_tables: [],
    },
  ];
  const subs: SubscriptionRow[] = [
    {
      subname: "sub_test",
      owner: "postgres",
      enabled: true,
      slot_name: "sub_slot",
      publications: ["pub_test"],
      worker_pid: 1234,
      received_lsn: "0/100",
      last_msg_send_secs: 1.0,
      last_msg_receipt_secs: 1.0,
      latest_end_lsn: "0/100",
      latest_end_secs: 1.0,
      sync_tables: 0,
      ready_tables: 5,
      total_tables: 5,
      apply_error_count: 0,
      sync_error_count: 0,
    },
  ];
  const slots: ReplicationSlotRow[] = [
    slot(true, "reserved", 1024),
  ];

  renderReplication(
    body as unknown as HTMLElement,
    placeholder as unknown as HTMLElement,
    { Primary: { senders: [] } },
    slots,
    null,
    null,
    pubs,
    subs,
  );

  assert.equal(placeholder.hidden, true);
  // body.children has 4 sections:
  // 1: Physical Replication (Role)
  // 2: Publications (pg_publication)
  // 3: Subscriptions (pg_subscription)
  // 4: Replication Slots
  assert.equal(body.children.length, 4);
  const titles = body.children.map((sec) => sec.children[0]?.textContent);
  assert.deepEqual(titles, [
    "Physical Replication (Role)",
    "Publications (pg_publication)",
    "Subscriptions (pg_subscription)",
    "Replication Slots",
  ]);
});
