Output demo/tui-us-010.gif

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
Type "source demo/tui-demo-setup.sh && DEMO_NAME=us010 tui_demo_prepare"
Enter
Sleep 200ms
Type "trap tui_demo_cleanup EXIT"
Enter
Sleep 100ms
Type "chmod +x demo/tui-us-010-editor.sh"
Enter
Sleep 100ms
Type `export EDITOR="$PWD/demo/tui-us-010-editor.sh"`
Enter
Sleep 100ms
Type "clear"
Enter
Sleep 300ms
Show
Set TypingSpeed 40ms

# ── US-010: edit secret value in-TUI via $EDITOR ────────
# Launch the TUI — EDITOR is exported to the fake-editor helper that
# rewrites the temp buffer with `sk_edited_in_tui`.
Type "$HIMITSU_BIN"
Enter
Sleep 1.5s

# Filter down to prod/API_KEY.
Type "api"
Sleep 800ms

# Open the viewer.
Enter
Sleep 1.2s

# Press `e` — the TUI suspends, $EDITOR runs against a temp file with
# the decrypted value, the helper rewrites it, the TUI resumes and
# re-encrypts the new value.
Type "e"
Sleep 1.8s

# Viewer now shows the edited value.
Sleep 1.2s

# Back out and quit.
Escape
Sleep 500ms
Escape
Sleep 500ms
