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

# 1. Macro Lens: vitals, sparklines, checkpointer + WAL generation, pg_stat_io,
# replication summary
Sleep 4s

# Database selector: pick a database on the cluster (d), then Esc out (mock)
Type "d"
Sleep 1.5s
Escape
Sleep 600ms

# 2. Micro Lens: activity + top-waits strip. Column colors & duration severity.
Tab
Sleep 3.5s
# Filter live (/)
Type "/"
Sleep 400ms
Type "bench"
Sleep 1.2s
Enter
Sleep 1.2s
# Clear filter (\)
Type "\"
Sleep 1.2s
# Full waits panel (w), then close
Type "w"
Sleep 2s
Type "w"
Sleep 500ms
# Session query detail (Enter), yank (y), close (Esc)
Type "j"
Sleep 300ms
Enter
Sleep 1.5s
Type "y"
Sleep 1.2s
Escape
Sleep 500ms

# 3. Blocks & Locks Lens: wait-for tree + active locks table
Tab
Sleep 3s
# Toggle between wait tree and active locks table (p)
Type "p"
Sleep 2.5s
Type "p"
Sleep 600ms

# 4. Replication Lens: senders + all replication slots
Tab
Sleep 2.5s

# 5. Schema Lens: tables, vacuum health sub-view (v), and refresh (B)
Tab
Sleep 2.5s
Type "v"
Sleep 2.5s
Type "v"
Sleep 500ms
Type "B"
Sleep 1.2s

# 6. Index Lens: advisor findings + detail
Tab
Sleep 2s
Type "j"
Sleep 300ms
Enter
Sleep 1.5s
Escape
Sleep 500ms

# 7. Query Lens: pg_stat_statements metrics & temp spills
Tab
Sleep 2.5s

# 8. Progress Lens: live DDL & maintenance tracking gauges
Tab
Sleep 2.5s
Enter
Sleep 1.5s
Escape
Sleep 500ms

# Incident Recording Mode: Shift+R (R) activates recording (REC indicator in header)
Type "R"
Sleep 2.5s
# Export snapshot bookmark (E)
Type "E"
Sleep 2s
# Stop recording (R)
Type "R"
Sleep 1s

# Help overlay (?): the full key map, then close
Type "?"
Sleep 3s
Escape
Sleep 800ms

Type "q"
Sleep 500ms
