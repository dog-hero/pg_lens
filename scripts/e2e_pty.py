#!/usr/bin/env python3
"""PTY end-to-end proof for pg_lens (mock mode).

Runs the TUI binary (with --mock, so no database is needed) in a real PTY,
reconstructs the rendered screen from the escape stream (cursor positioning +
text; styles ignored), snapshots it at timed moments, sends keys, and checks
the exit code. The Screen class is reused by e2e_pty_live.py for the
real-database run.
"""
import os, pty, re, select, signal, subprocess, sys, time

BIN = "/Users/leonardo.benedet/BenedetLabs/pg_lens/target/debug/pg_lens"
# Overridable so the same harness proves the 80x24 resize case (Fase 4).
COLS = int(os.environ.get("PG_LENS_E2E_COLS", 120))
ROWS = int(os.environ.get("PG_LENS_E2E_ROWS", 36))
# PG_LENS_E2E_BASIC=1: render + quit only (used for the small-terminal run).
BASIC = bool(os.environ.get("PG_LENS_E2E_BASIC"))


class Screen:
    def __init__(self):
        self.grid = [[" "] * COLS for _ in range(ROWS)]
        self.r = self.c = 0
        self.buf = b""

    def feed(self, data: bytes):
        self.buf += data
        text = self.buf.decode("utf-8", errors="ignore")
        self.buf = b""
        i = 0
        while i < len(text):
            ch = text[i]
            if ch == "\x1b":
                m = re.match(r"\x1b\[([0-9;?]*)([A-Za-z])", text[i:])
                if m:
                    self._csi(m.group(1), m.group(2))
                    i += m.end()
                    continue
                m = re.match(r"\x1b\][^\x07\x1b]*(\x07|\x1b\\)", text[i:])
                if m:  # OSC (title etc.)
                    i += m.end()
                    continue
                i += 2  # unknown 2-byte escape
                continue
            if ch == "\r":
                self.c = 0
            elif ch == "\n":
                self.r = min(self.r + 1, ROWS - 1)
            elif ch == "\b":
                self.c = max(self.c - 1, 0)
            elif ch >= " ":
                if self.r < ROWS and self.c < COLS:
                    self.grid[self.r][self.c] = ch
                self.c = min(self.c + 1, COLS)
            i += 1

    def _csi(self, params: str, final: str):
        if params.startswith("?"):
            return  # private modes (altscreen, cursor visibility)
        nums = [int(x) for x in params.split(";") if x.isdigit()]
        if final in "Hf":
            self.r = (nums[0] if nums else 1) - 1
            self.c = (nums[1] if len(nums) > 1 else 1) - 1
        elif final == "A":
            self.r = max(self.r - (nums[0] if nums else 1), 0)
        elif final == "B":
            self.r = min(self.r + (nums[0] if nums else 1), ROWS - 1)
        elif final == "C":
            self.c = min(self.c + (nums[0] if nums else 1), COLS - 1)
        elif final == "D":
            self.c = max(self.c - (nums[0] if nums else 1), 0)
        elif final == "J":
            if nums and nums[0] in (2, 3):
                self.grid = [[" "] * COLS for _ in range(ROWS)]
        elif final == "K":
            self.grid[self.r][self.c:] = [" "] * (COLS - self.c)
        # 'm' (SGR) and everything else: ignored

    def snapshot(self) -> str:
        return "\n".join("".join(row).rstrip() for row in self.grid).rstrip()


def main():
    master, slave = pty.openpty()
    import fcntl, struct, termios
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
    env = dict(os.environ, TERM="xterm-256color")
    proc = subprocess.Popen([BIN, "--mock"], stdin=slave, stdout=slave,
                            stderr=slave, env=env, close_fds=True)
    os.close(slave)
    screen = Screen()

    def pump(seconds: float):
        end = time.time() + seconds
        while time.time() < end:
            ready, _, _ = select.select([master], [], [], 0.05)
            if ready:
                try:
                    screen.feed(os.read(master, 65536))
                except OSError:
                    return

    def send(key: str):
        os.write(master, key.encode())

    snaps = {}
    pump(2.6);            snaps["t1_nokeys"] = screen.snapshot()
    pump(2.4);            snaps["t2_nokeys"] = screen.snapshot()
    send("\t"); pump(0.9); snaps["t3_after_tab"] = screen.snapshot()
    if not BASIC:
        send("j");  pump(0.6); snaps["t4_after_j"] = screen.snapshot()
        send("s");  pump(0.6); snaps["t5_after_s"] = screen.snapshot()
        # Fase 4: Enter opens the detail panel; Enter closes it again.
        send("\r"); pump(0.6); snaps["t6_detail_open"] = screen.snapshot()
        send("\r"); pump(0.6); snaps["t7_detail_closed"] = screen.snapshot()
        # Admin actions: c opens the cancel confirmation modal; y confirms —
        # the statusbar feedback appears ("sent…" then the mock's immediate
        # re-poll carries "query cancelled (PID …)").
        send("c");  pump(0.6); snaps["a1_cancel_modal"] = screen.snapshot()
        send("y");  pump(0.9); snaps["a2_after_confirm"] = screen.snapshot()
        # K opens the terminate modal (red variant with the kill warning);
        # Esc aborts it cleanly (no command, no quit).
        send("K");  pump(0.6); snaps["a3_kill_modal"] = screen.snapshot()
        send("\x1b"); pump(0.6); snaps["a4_kill_aborted"] = screen.snapshot()
        # Pause (Space): UI-side freeze on the Micro Lens. First wait for
        # the admin feedback line above to fade (~10s TTL) — otherwise its
        # expiry mid-pause would shift the layout and break the
        # frozen-screen comparison. Then: two captures 3.2s apart must be
        # identical (mock data changes every 2s) except the statusbar
        # staleness, which keeps counting up ON PURPOSE — mask it.
        fade_deadline = time.time() + 14
        while time.time() < fade_deadline and (
                "query cancelled (PID" in screen.snapshot()
                or "cancel sent to PID" in screen.snapshot()):
            pump(0.5)
        stale_data_re = re.compile(r"data: (\d+)s ago")
        def masked(snap):
            return stale_data_re.sub("data: Xs ago", snap)
        send(" "); pump(0.5); snaps["p1_paused"] = screen.snapshot()
        pump(3.2);            snaps["p2_still_paused"] = screen.snapshot()
        send(" "); pump(2.5); snaps["p3_resumed"] = screen.snapshot()
        # Fase 4: '-' three times: 2.0s -> 0.5s, live through the watch
        # channel. Two snapshots 0.9s apart must differ (at the old 2.0s
        # cadence they could not have both refreshed).
        send("---"); pump(0.9); snaps["t8_fast_a"] = screen.snapshot()
        pump(0.9);             snaps["t9_fast_b"] = screen.snapshot()
        # Fase S3: restore the 2.0s cadence first — the mock refreshes its
        # schema every 5 ticks, and the R-forces-recollection proof below
        # needs staleness to climb well past the check thresholds.
        send("+++"); pump(0.4)
    # U1: second Tab now reaches the Replication Lens (Macro/Micro/
    # Replication/Schema/Indexes/Queries) — also exercised in BASIC, proving
    # the 80x24 layout doesn't panic.
    send("\t"); pump(0.9); snaps["r1_replication"] = screen.snapshot()
    # Third Tab reaches the Schema Lens (also exercised in BASIC).
    send("\t"); pump(0.9); snaps["s1_schema"] = screen.snapshot()
    if not BASIC:
        # s: size (default) -> dead tuples; order must visibly change.
        send("s");  pump(0.6); snaps["s2_schema_sorted"] = screen.snapshot()
        # Enter: table detail (selected row 0 = order_items under dead sort)
        # must list the table's indexes with their bloat estimates.
        send("\r"); pump(0.6); snaps["s3_schema_detail"] = screen.snapshot()
        send("\r"); pump(0.6); snaps["s4_detail_closed"] = screen.snapshot()
        # R forces a schema re-collection: wait until the footer staleness
        # climbed to >= 4s (the mock recollects naturally every 5 ticks =
        # 10s, so 4..7s is a natural-refresh-free window), press R, and the
        # staleness must drop back below it despite the wait in between.
        stale_re = re.compile(r"collected (\d+)s ago")
        before = None
        deadline = time.time() + 15
        while time.time() < deadline:
            pump(1.0)
            m = stale_re.search(screen.snapshot())
            if m and 4 <= int(m.group(1)) <= 7:
                before = int(m.group(1))
                break
        snaps["s5_before_R"] = screen.snapshot()
        send("R"); pump(2.8)
        snaps["s6_after_R"] = screen.snapshot()
        m = stale_re.search(snaps["s6_after_R"])
        after = int(m.group(1)) if m else None
    # U1: fourth Tab reaches the Index Lens (also in BASIC, proving the
    # 80x24 layout doesn't panic).
    send("\t"); pump(0.9); snaps["x1_index_lens"] = screen.snapshot()
    # Query Lens (pg_stat_statements): fifth Tab reaches it (also in BASIC).
    send("\t"); pump(0.9); snaps["q1_query_lens"] = screen.snapshot()
    if not BASIC:
        # s: total (default) -> calls; order may change, label must.
        send("s");  pump(0.6); snaps["q2_query_sorted"] = screen.snapshot()
        send("s");  pump(0.6); snaps["q3_query_sorted_mean"] = screen.snapshot()
        # Enter: statement detail with the highlighted full query + queryid.
        send("\r"); pump(0.6); snaps["q4_query_detail"] = screen.snapshot()
        send("\r"); pump(0.6); snaps["q5_detail_closed"] = screen.snapshot()
    # v0.9: `?` opens the keyboard help overlay (static, works at any grid
    # size); Esc closes it again without disturbing the dashboard underneath.
    send("?"); pump(0.6); snaps["h1_help_open"] = screen.snapshot()
    send("\x1b"); pump(0.6); snaps["h2_help_closed"] = screen.snapshot()
    send("q");  pump(1.0)

    try:
        code = proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
        code = "KILLED (did not exit on q)"

    for name, snap in snaps.items():
        with open(f"/tmp/pg_lens_{name}.txt", "w") as f:
            f.write(snap + "\n")

    ok = True
    def check(label, cond):
        nonlocal ok
        print(f"{'PASS' if cond else 'FAIL'}: {label}")
        ok = ok and cond

    check("screen changed between t1 and t2 with NO keypress (poller pipeline live)",
          snaps["t1_nokeys"] != snaps["t2_nokeys"])
    check("t1 shows Macro Lens (Connections gauge)", "Connections" in snaps["t1_nokeys"])
    check("Macro Lens shows the Replication panel with both mock replicas",
          "Replication" in snaps["t1_nokeys"] and "replica-1" in snaps["t1_nokeys"]
          and "replica-2-dr" in snaps["t1_nokeys"])
    check("lagging replica carries the '!' severity marker",
          any("!" in ln and "replica-2-dr" in ln
              for ln in snaps["t1_nokeys"].splitlines()))
    check("Tab during refresh switched to Micro Lens (Activity table)",
          "Activity" in snaps["t3_after_tab"] and "PID" in snaps["t3_after_tab"])
    check("Micro Lens shows the top-waits strip (ratio + ranked waits)",
          "5/7 waiting" in snaps["t3_after_tab"]
          and "Lock:transactionid ×1" in snaps["t3_after_tab"]
          and "IO:DataFileRead ×1" in snaps["t3_after_tab"])
    if not BASIC:
        check("j moved selection (statusbar row 2/7)", "row 2/7" in snaps["t4_after_j"])
        check("s cycled sort (statusbar sort=state)", "sort=state" in snaps["t5_after_s"])
        check("Enter opened the detail panel", "Detail" in snaps["t6_detail_open"]
              and "Enter/Esc: close" in snaps["t6_detail_open"])
        check("Enter closed the detail panel again",
              "Enter/Esc: close" not in snaps["t7_detail_closed"])
        # --- admin actions (cancel/terminate) ------------------------------
        check("Micro Lens statusbar shows the admin hints",
              "c: cancel" in snaps["t3_after_tab"] and "K: kill" in snaps["t3_after_tab"])
        check("c opened the cancel confirmation modal",
              "Cancel query on PID" in snaps["a1_cancel_modal"]
              and "y: confirm" in snaps["a1_cancel_modal"])
        check("y confirmed: modal closed, statusbar feedback appeared",
              "y: confirm" not in snaps["a2_after_confirm"]
              and ("cancel sent to PID" in snaps["a2_after_confirm"]
                   or "query cancelled (PID" in snaps["a2_after_confirm"]))
        check("mock poller reported the cancel result in the snapshot",
              "query cancelled (PID" in snaps["a2_after_confirm"])
        check("K opened the terminate modal (red variant text)",
              "Terminate backend PID" in snaps["a3_kill_modal"]
              and "The connection will be killed." in snaps["a3_kill_modal"])
        check("Esc aborted the terminate modal cleanly (no quit)",
              "Terminate backend PID" not in snaps["a4_kill_aborted"]
              and "Activity" in snaps["a4_kill_aborted"])
        # --- pause / freeze (Space) ----------------------------------------
        check("header shows the Space: pause hint while live",
              "Space: pause" in snaps["t3_after_tab"])
        check("Space froze the screen (PAUSED indicator + resume hint)",
              "PAUSED" in snaps["p1_paused"]
              and "Space: resume" in snaps["p1_paused"])
        check("data stopped changing while paused (masked screens 3.2s apart identical)",
              masked(snaps["p1_paused"]) == masked(snaps["p2_still_paused"])
              and "PAUSED" in snaps["p2_still_paused"])
        p1_stale = stale_data_re.search(snaps["p1_paused"])
        p2_stale = stale_data_re.search(snaps["p2_still_paused"])
        check("staleness kept counting up while paused",
              p1_stale is not None and p2_stale is not None
              and int(p2_stale.group(1)) > int(p1_stale.group(1)))
        check("Space again resumed (PAUSED gone, data changed)",
              "PAUSED" not in snaps["p3_resumed"]
              and masked(snaps["p3_resumed"]) != masked(snaps["p2_still_paused"]))
        check("'-' x3 shows refresh=0.5s in the statusbar",
              "refresh=0.5s" in snaps["t8_fast_a"])
        check("snapshots arrive at the faster cadence (screens 0.9s apart differ)",
              snaps["t8_fast_a"] != snaps["t9_fast_b"])
    # --- U1: Replication Lens ----------------------------------------------
    check("Tab x2 reached the Replication Lens (Role + Slots panels)",
          "Role" in snaps["r1_replication"] and "Slots" in snaps["r1_replication"])
    check("Replication Lens shows ALL mock slots, unlike the Macro summary",
          "replica_1_slot" in snaps["r1_replication"]
          and "analytics_cdc" in snaps["r1_replication"])
    # --- Fase S3: Schema Lens ---------------------------------------------
    check("Tab x3 reached the Schema Lens (Tables + Bloat% columns)",
          "Tables" in snaps["s1_schema"] and "Bloat%" in snaps["s1_schema"])
    # At 80 cols the Table column ellipsis-truncates, so BASIC only asserts
    # the schema prefix; the full name is checked at the default 120 cols.
    check("schema table shows mock rows",
          ("public." if BASIC else "public.order_items") in snaps["s1_schema"])
    check("footer: db name + ESTIMATED bloat label",
          "db: shop" in snaps["s1_schema"] and "ESTIMATED" in snaps["s1_schema"])
    if not BASIC:
        def first_data_row_table(snap):
            for line in snap.splitlines():
                if "public." in line or "audit." in line:
                    return line
            return ""
        check("default sort=size puts pgbench_accounts on top",
              "sort=size" in snaps["s1_schema"]
              and "pgbench_accounts" in first_data_row_table(snaps["s1_schema"]))
        check("s cycled schema sort (sort=dead, order_items now on top)",
              "sort=dead" in snaps["s2_schema_sorted"]
              and "order_items" in first_data_row_table(snaps["s2_schema_sorted"]))
        check("severity markers rendered (red '!!' row present)",
              "!!" in snaps["s1_schema"])
        check("is_na renders '~?' instead of a number", "~?" in snaps["s1_schema"])
        check("Enter opened the table detail with its index bloat rows",
              "Table — public.order_items" in snaps["s3_schema_detail"]
              and "order_items_pkey" in snaps["s3_schema_detail"])
        check("Enter closed the table detail again",
              "order_items_pkey" not in snaps["s4_detail_closed"])
        check("staleness climbed into the 4..7s window before R",
              before is not None)
        check(f"R reset the collection staleness ({before}s -> {after}s "
              "despite 2.8s more elapsing)",
              before is not None and after is not None and after < before
              and after <= 3)
    # --- U1: Index Lens ------------------------------------------------------
    check("Tab x4 reached the Index Lens (its own tab now, Flag column)",
          "Indexes" in snaps["x1_index_lens"] and "Flag" in snaps["x1_index_lens"])
    check("Index Lens shows the mock's findings (UNUSED/DUP/prefix)",
          "UNUSED" in snaps["x1_index_lens"] and "DUP" in snaps["x1_index_lens"])
    # --- Query Lens (pg_stat_statements) -----------------------------------
    check("Tab x5 reached the Query Lens (Statements + Hit% columns)",
          "Statements" in snaps["q1_query_lens"] and "Hit%" in snaps["q1_query_lens"])
    check("query lens footer: db + count + scope + staleness",
          "db: shop" in snaps["q1_query_lens"]
          and "8 statements" in snaps["q1_query_lens"]
          and "current database only" in snaps["q1_query_lens"]
          and re.search(r"collected \d+s ago", snaps["q1_query_lens"]))
    if not BASIC:
        def first_data_row_query(snap):
            for line in snap.splitlines():
                if "SELECT" in line or "UPDATE" in line or "INSERT" in line:
                    return line
            return ""
        check("default sort=total puts the pgbench UPDATE on top",
              "sort=total" in snaps["q1_query_lens"]
              and "UPDATE pgbench_accounts" in first_data_row_query(snaps["q1_query_lens"]))
        check("Hit% dash rendered for the zero-blocks row",
              "—" in snaps["q1_query_lens"])
        check("s cycled statements sort (sort=calls)",
              "sort=calls" in snaps["q2_query_sorted"])
        check("s cycled statements sort again (sort=mean, pg_sleep on top)",
              "sort=mean" in snaps["q3_query_sorted_mean"]
              and "pg_sleep" in first_data_row_query(snaps["q3_query_sorted_mean"]))
        check("Enter opened the statement detail with its queryid",
              "Statement — queryid" in snaps["q4_query_detail"]
              and "shared blocks:" in snaps["q4_query_detail"]
              and "pg_sleep" in snaps["q4_query_detail"])
        check("Enter closed the statement detail again",
              "Statement — queryid" not in snaps["q5_detail_closed"])
    # --- v0.9: keyboard help overlay (`?`) ----------------------------------
    check("? opened the keyboard help overlay (known bindings visible)",
          "keyboard help" in snaps["h1_help_open"]
          and "terminate the backend" in snaps["h1_help_open"])
    check("Esc closed the help overlay again",
          "keyboard help" not in snaps["h2_help_closed"])
    check("q exited cleanly (EXIT_CODE=0)", code == 0)
    print(f"EXIT_CODE={code}")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
