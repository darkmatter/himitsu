Output demo/tui-us-011.gif

Set Theme { "name": "Minted", "black": "#0B0A0F", "red": "#963543", "green": "#709C86", "yellow": "#C7B485", "blue": "#31457D", "magenta": "#B86EB8", "cyan": "#527180", "white": "#76788C", "brightBlack": "#96a5b6", "brightRed": "#E61C6B", "brightGreen": "#709C86", "brightYellow": "#B39864", "brightBlue": "#33b3cc", "brightMagenta": "#5F5887", "brightCyan": "#b3e6de", "brightWhite": "#b1b1bf", "background": "#0C0C13", "foreground": "#afd1e9", "selection": "#181E2D", "cursor": "#69E7B8" }
Set Shell "bash"
Set FontSize 22
Set Width 1200
Set Height 700
Set Padding 20
Set TypingSpeed 40ms

# ── Silent setup ─────────────────────────────────────────
Hide
Set TypingSpeed 1ms
Type "source demo/tui-demo-setup.sh && DEMO_NAME=us011 tui_demo_prepare"
Enter
Sleep 200ms
Type "trap tui_demo_cleanup EXIT"
Enter
Sleep 100ms
Type "clear"
Enter
Sleep 300ms
Show
Set TypingSpeed 40ms

# ── US-011: browse and drill into a secret from search ──
# Launch the TUI — search view is the root and lists every secret
# in the seeded store, folder-grouped by top-level path prefix.
Type "$HIMITSU_BIN"
Enter
Sleep 1.5s

# Walk the result list with ↓ / ↑. The first selectable row is the
# first secret under the `prod` folder header.
Down
Sleep 500ms
Down
Sleep 500ms
Up
Sleep 500ms

# Live-filter by typing — narrows to anything matching `data`.
Type "data"
Sleep 900ms

# Open the secret viewer for prod/DATABASE_URL — the decrypted value
# and metadata block render in the viewer overlay.
Enter
Sleep 2s

# Back out to search; the query and selection are preserved.
Escape
Sleep 800ms

# Clear the filter and pick a secret from a different folder.
Backspace
Backspace
Backspace
Backspace
Sleep 500ms
Type "staging"
Sleep 700ms
Enter
Sleep 1.5s

# Back to search and quit.
Escape
Sleep 500ms
Escape
Sleep 500ms
