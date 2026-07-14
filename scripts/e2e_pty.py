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
        # Fase 4: '-' three times: 2.0s -> 0.5s, live through the watch
        # channel. Two snapshots 0.9s apart must differ (at the old 2.0s
        # cadence they could not have both refreshed).
        send("---"); pump(0.9); snaps["t8_fast_a"] = screen.snapshot()
        pump(0.9);             snaps["t9_fast_b"] = screen.snapshot()
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
    check("Tab during refresh switched to Micro Lens (Activity table)",
          "Activity" in snaps["t3_after_tab"] and "PID" in snaps["t3_after_tab"])
    if not BASIC:
        check("j moved selection (statusbar row 2/6)", "row 2/6" in snaps["t4_after_j"])
        check("s cycled sort (statusbar sort=state)", "sort=state" in snaps["t5_after_s"])
        check("Enter opened the detail panel", "Detail" in snaps["t6_detail_open"]
              and "Enter/Esc: close" in snaps["t6_detail_open"])
        check("Enter closed the detail panel again",
              "Enter/Esc: close" not in snaps["t7_detail_closed"])
        check("'-' x3 shows refresh=0.5s in the statusbar",
              "refresh=0.5s" in snaps["t8_fast_a"])
        check("snapshots arrive at the faster cadence (screens 0.9s apart differ)",
              snaps["t8_fast_a"] != snaps["t9_fast_b"])
    check("q exited cleanly (EXIT_CODE=0)", code == 0)
    print(f"EXIT_CODE={code}")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
