Output demo/tui-us-009.gif

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
Type "source demo/tui-demo-setup.sh && DEMO_NAME=us009 tui_demo_prepare"
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

# ── US-009: delete secret with confirm prompt ──────────
# Launch the TUI — search view is the root.
Type "$HIMITSU_BIN"
Enter
Sleep 1.5s

# Type to filter down to the deletion target.
Type "debug"
Sleep 800ms

# Enter opens the secret viewer for staging/DEBUG_TOKEN.
Enter
Sleep 1.2s

# Press `d` to enter the confirm-delete prompt — footer reads
# "Delete <path>? (y/N)".
Type "d"
Sleep 1.2s

# Confirm with `y`. The viewer pops back to a refreshed search view
# with staging/DEBUG_TOKEN gone from the listing.
Type "y"
Sleep 1.5s

# Quit.
Escape
Sleep 500ms
