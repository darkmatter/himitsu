Output demo/tui-us-012.gif

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
Type "source demo/tui-demo-setup.sh && DEMO_NAME=us012 tui_demo_prepare"
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

# ── US-012: per-view help overlay ───────────────────────
# Launch the TUI.
Type "$HIMITSU_BIN"
Enter
Sleep 1.5s

# `?` opens the help overlay scoped to the current view (search).
Type "?"
Sleep 2.5s

# Dismiss the overlay with Esc.
Escape
Sleep 1s

# Open a secret to demonstrate the overlay re-populates with the
# secret-viewer bindings.
Type "api"
Sleep 600ms
Enter
Sleep 1.2s

# `?` again — different help, scoped to the secret viewer.
Type "?"
Sleep 2.5s
Escape
Sleep 800ms

# Quit.
Escape
Sleep 500ms
Escape
Sleep 500ms
