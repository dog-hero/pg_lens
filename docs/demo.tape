# pg_lens demo — regenerate with: vhs docs/demo.tape (from the repo root,
# needs `cargo build --release -p pg_lens_tui` first and vhs installed).
Output docs/demo.gif

Set FontSize 13
Set Width 1400
Set Height 800
Set Theme "Catppuccin Mocha"
Set Padding 12

Hide
Type "./target/release/pg_lens --mock"
Enter
Sleep 3.5s
Show

# Macro Lens: vitals, sparklines, checkpointer + WAL generation, pg_stat_io,
# replication summary
Sleep 4.5s

# Database selector: pick a database on the cluster (d), then Esc out (mock)
Type "d"
Sleep 2s
Escape
Sleep 800ms

# Micro Lens: activity + top-waits strip. Dwell here — every row is colored
# by session state, with red/orange overriding it for long-running queries
# and blocked sessions winning over both.
Tab
Sleep 5s
# filter live
Type "/"
Sleep 500ms
Type "bench"
Sleep 1.5s
Enter
Sleep 1.5s
# `\` clears the active lens's filter in one key
Type "\"
Sleep 1.5s
# full waits panel (w), then close
Type "w"
Sleep 2.5s
Type "w"
Sleep 500ms
# idle-in-transaction census (I), then close
Type "I"
Sleep 2.5s
Type "I"
Sleep 500ms
# a query detail, then `y` yanks the selected session's query to the clipboard
Type "j"
Sleep 400ms
Enter
Sleep 1.8s
Type "y"
Sleep 2s
Escape
Sleep 600ms

# Replication Lens: senders + all slots (scrollable)
Tab
Sleep 3s

# Schema Lens: tables, then the full Vacuum sub-view (v)
Tab
Sleep 2s
Type "v"
Sleep 3s
Type "v"
Sleep 600ms

# Index Lens: advisor findings + detail
Tab
Sleep 2s
Type "j"
Sleep 300ms
Enter
Sleep 2s
Escape
Sleep 600ms

# Query Lens: pg_stat_statements
Tab
Sleep 2.5s

# help overlay (?): the full key map, then close
Type "?"
Sleep 3s
Escape
Sleep 800ms

Type "q"
Sleep 500ms
