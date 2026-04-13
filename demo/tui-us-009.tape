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
# Launch the TUI — auto-resolves the seeded store from HIMITSU_HOME.
Type "$HIMITSU_BIN"
Enter
Sleep 1.5s

# Move env selection from `prod` down to `staging`. The right pane
# now lists staging's secrets: API_KEY (top) and DEBUG_TOKEN.
Type "j"
Sleep 800ms

# Tab into the secrets pane — selection starts on staging/API_KEY.
Tab
Sleep 600ms

# Move down to staging/DEBUG_TOKEN, the deletion target.
Type "j"
Sleep 600ms

# Open the secret viewer so the decrypted value + footer are visible.
Enter
Sleep 1.2s

# Press `d` to enter ConfirmDelete mode — prompt reads
# "Delete env/path? (y/N)" in the footer.
Type "d"
Sleep 1.2s

# Confirm with lowercase `y`. The viewer pops back to the dashboard,
# status line reports success, and staging/DEBUG_TOKEN is gone from
# the list (staging env remains because staging/API_KEY is still there).
Type "y"
Sleep 1.5s

# Quit.
Type "q"
Sleep 500ms
