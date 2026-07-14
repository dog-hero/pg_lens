#!/usr/bin/env node
// End-to-end SSE consumer check for `pg_lens serve`.
//
// Node 24 has no global EventSource (verified: `typeof EventSource` →
// "undefined"), so this reads /api/stream with fetch and parses SSE frames
// by hand — which also proves the wire format is standard SSE, not just
// something the browser class happens to accept.
//
// Usage: node e2e_sse.mjs [url] [--token T] [--count N] [--timeout SECS]
// Exits 0 iff N snapshots arrive, all parse as JSON, and the data changes
// between snapshots (the "dashboard updates in real time" stand-in).

const args = process.argv.slice(2);
let url = "http://127.0.0.1:8080/api/stream";
let token = null;
let count = 2;
let timeoutSecs = 30;
for (let i = 0; i < args.length; i++) {
  if (args[i] === "--token") token = args[++i];
  else if (args[i] === "--count") count = Number(args[++i]);
  else if (args[i] === "--timeout") timeoutSecs = Number(args[++i]);
  else url = args[i];
}

const headers = { Accept: "text/event-stream" };
if (token !== null) headers.Authorization = `Bearer ${token}`;

const controller = new AbortController();
const timer = setTimeout(() => controller.abort(), timeoutSecs * 1000);

const res = await fetch(url, { headers, signal: controller.signal });
if (!res.ok) {
  console.error(`FAIL: HTTP ${res.status} from ${url}`);
  process.exit(1);
}
const contentType = res.headers.get("content-type") ?? "";
if (!contentType.startsWith("text/event-stream")) {
  console.error(`FAIL: content-type is ${contentType}, not text/event-stream`);
  process.exit(1);
}

const decoder = new TextDecoder();
let buffer = "";
const snapshots = [];

outer: for await (const chunk of res.body) {
  buffer += decoder.decode(chunk, { stream: true });
  // SSE events are separated by a blank line.
  let sep;
  while ((sep = buffer.indexOf("\n\n")) !== -1) {
    const frame = buffer.slice(0, sep);
    buffer = buffer.slice(sep + 2);
    const data = frame
      .split("\n")
      .filter((line) => line.startsWith("data:"))
      .map((line) => line.slice(5).trimStart())
      .join("\n");
    if (data === "") continue; // keep-alive comment frame
    const snapshot = JSON.parse(data);
    const point = snapshot.history.points.at(-1);
    console.log(
      `snapshot ${snapshots.length + 1}: status=${JSON.stringify(snapshot.status)} ` +
        `tps=${snapshot.vitals.tps.toFixed(1)} active=${snapshot.vitals.active} ` +
        `history_len=${snapshot.history.points.length} ` +
        `last_point=${point ? `${point.epoch_ms}/${point.tps.toFixed(1)}tps` : "none"} ` +
        `activity_rows=${snapshot.activity.length}`,
    );
    snapshots.push(data);
    if (snapshots.length >= count) break outer;
  }
}
clearTimeout(timer);
controller.abort(); // close the connection

if (snapshots.length < count) {
  console.error(`FAIL: only ${snapshots.length}/${count} snapshots arrived`);
  process.exit(1);
}
const allSame = snapshots.every((s) => s === snapshots[0]);
if (allSame) {
  console.error("FAIL: snapshots did not change between events");
  process.exit(1);
}
console.log(`OK: ${snapshots.length} parsed snapshots, data changing`);
