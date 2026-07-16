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

# Macro Lens: vitals, sparklines, checkpointer, replication summary
Sleep 3s

# Micro Lens: activity + top-waits strip
Tab
Sleep 2.5s
# filter live
Type "/"
Sleep 500ms
Type "bench"
Sleep 1.5s
Enter
Sleep 1.5s
# clear the filter again
Type "/"
Backspace 5
Enter
Sleep 800ms
# open a query detail
Type "j"
Sleep 400ms
Enter
Sleep 2s
Escape
Sleep 800ms

# Replication Lens: senders + all slots
Tab
Sleep 3s

# Schema Lens: tables + vacuum/wraparound
Tab
Sleep 3s

# Index Lens: advisor findings
Tab
Sleep 2s
Type "j"
Sleep 300ms
Enter
Sleep 2.2s
Escape
Sleep 600ms

# Query Lens: pg_stat_statements
Tab
Sleep 2.5s

# quit (double-Esc barrier demo would confuse the loop — use q)
Type "q"
Sleep 500ms
