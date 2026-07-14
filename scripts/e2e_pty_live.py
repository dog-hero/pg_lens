#!/usr/bin/env python3
"""PTY end-to-end proof for pg_lens Phase 3 against a real PostgreSQL.

Reuses the VT-parsing Screen class from e2e_pty.py. Drives the TUI with a
real --dsn, verifies live data on both lenses, and (optionally) stops/starts
a Docker container mid-session to prove the error banner + reconnect path.

Usage:
  e2e_pty_live.py --dsn "host=localhost port=54316 user=postgres password=pg" \
      [--tag pg16] [--expect-header "PG 16"] [--expect-micro pg_sleep] \
      [--expect-tps-move] [--resilience-container pglens_pg16]
"""
import argparse
import fcntl
import os
import pty
import select
import struct
import subprocess
import sys
import termios
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from e2e_pty import BIN, COLS, ROWS, Screen  # noqa: E402


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dsn", required=True)
    ap.add_argument("--tag", default="live", help="prefix for /tmp snapshot files")
    ap.add_argument("--expect-header", default=None,
                    help="substring expected in the header, e.g. 'PG 16'")
    ap.add_argument("--expect-micro", default=None,
                    help="substring expected in the Micro Lens, e.g. 'pg_sleep'")
    ap.add_argument("--expect-tps-move", action="store_true",
                    help="require the TPS reading to change between snapshots")
    ap.add_argument("--resilience-container", default=None,
                    help="docker container to stop/start mid-session")
    args = ap.parse_args()

    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
    env = dict(os.environ, TERM="xterm-256color")
    proc = subprocess.Popen([BIN, "--dsn", args.dsn], stdin=slave, stdout=slave,
                            stderr=slave, env=env, close_fds=True)
    os.close(slave)
    screen = Screen()

    def pump(seconds):
        end = time.time() + seconds
        while time.time() < end:
            ready, _, _ = select.select([master], [], [], 0.05)
            if ready:
                try:
                    screen.feed(os.read(master, 65536))
                except OSError:
                    return

    def send(key):
        os.write(master, key.encode())

    def docker(*cmd):
        subprocess.run(["docker", *cmd], check=True, capture_output=True)

    snaps = {}
    checks = []

    def check(label, cond):
        checks.append((label, bool(cond)))

    # --- live data on the Macro Lens -------------------------------------
    pump(3.5)
    snaps["t1_macro"] = screen.snapshot()
    check("macro lens rendered (Connections gauge)", "Connections" in snaps["t1_macro"])
    check("no error banner while DB is up", "DB error" not in snaps["t1_macro"])
    if args.expect_header:
        check(f"header shows {args.expect_header!r}",
              args.expect_header in snaps["t1_macro"])

    if args.expect_tps_move:
        pump(4.5)
        snaps["t2_macro"] = screen.snapshot()
        tps1 = [l for l in snaps["t1_macro"].splitlines() if "TPS (now:" in l]
        tps2 = [l for l in snaps["t2_macro"].splitlines() if "TPS (now:" in l]
        check("TPS reading moved between macro snapshots",
              tps1 and tps2 and tps1 != tps2)

    # --- live data on the Micro Lens --------------------------------------
    send("\t")
    pump(1.5)
    snaps["t3_micro"] = screen.snapshot()
    check("micro lens rendered (Activity table)",
          "Activity" in snaps["t3_micro"] and "PID" in snaps["t3_micro"])
    if args.expect_micro:
        check(f"micro lens shows {args.expect_micro!r}",
              args.expect_micro in snaps["t3_micro"])

    # --- resilience: DB down -> banner + responsive UI -> recovery --------
    if args.resilience_container:
        send("\t")  # back to Macro Lens
        pump(0.5)
        docker("stop", args.resilience_container)
        pump(9.0)  # poll failure + error snapshot must land within this
        snaps["t4_down"] = screen.snapshot()
        check("error banner visible after docker stop", "DB error" in snaps["t4_down"])
        if args.expect_header:
            check("last data retained while down (header keeps version)",
                  args.expect_header in snaps["t4_down"])
        send("\t")  # keys must still work while down
        pump(1.0)
        snaps["t5_keys_down"] = screen.snapshot()
        check("UI responsive while DB down (Tab reached Micro Lens)",
              "Activity" in snaps["t5_keys_down"])
        check("banner persists on the other tab", "DB error" in snaps["t5_keys_down"])

        docker("start", args.resilience_container)
        recovered = False
        deadline = time.time() + 45  # container boot + poller backoff (max 10s)
        while time.time() < deadline:
            pump(2.0)
            snap = screen.snapshot()
            if "DB error" not in snap and "PID" in snap:
                recovered = True
                snaps["t6_recovered"] = snap
                break
        snaps.setdefault("t6_recovered", screen.snapshot())
        check("banner cleared after docker start (poller reconnected)", recovered)

    send("q")
    pump(1.0)
    try:
        code = proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
        code = "KILLED (did not exit on q)"

    for name, snap in snaps.items():
        with open(f"/tmp/pg_lens_{args.tag}_{name}.txt", "w") as f:
            f.write(snap + "\n")

    ok = True
    for label, cond in checks:
        print(f"{'PASS' if cond else 'FAIL'}: {label}")
        ok = ok and cond
    print(f"EXIT_CODE={code}")
    sys.exit(0 if ok and code == 0 else 1)


if __name__ == "__main__":
    main()
