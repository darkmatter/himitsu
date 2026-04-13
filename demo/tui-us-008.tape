Output demo/tui-us-008.gif

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
Type "source demo/tui-demo-setup.sh && DEMO_NAME=us008 tui_demo_prepare"
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

# ── US-008: new secret from dashboard ───────────────────
# Launch the TUI — auto-resolves the seeded store from HIMITSU_HOME.
# Dashboard renders with `prod` and `staging` envs.
Type "$HIMITSU_BIN"
Enter
Sleep 1.5s

# Press 'n' to open the new-secret form. The env field is
# pre-filled with the currently-selected env (`prod`) and focused.
Type "n"
Sleep 900ms

# Leave env as `prod` and press Enter to advance to the path field.
Enter
Sleep 700ms

# Type the new secret path letter-by-letter.
Type "N"
Type "E"
Type "W"
Type "_"
Type "A"
Type "P"
Type "I"
Type "_"
Type "K"
Type "E"
Type "Y"
Sleep 700ms

# Advance to the value field.
Enter
Sleep 700ms

# Type the secret value. Enter on the value field inserts a newline,
# so we save with Ctrl+S instead.
Type "sk_new_value_123"
Sleep 700ms

# Save the new secret. Form closes and dashboard reappears with the
# new secret highlighted plus a "created prod/NEW_API_KEY" status.
Ctrl+S
Sleep 1.5s

# Quit.
Type "q"
Sleep 500ms
