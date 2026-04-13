Output demo/tui-us-013.gif

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
Type "source demo/tui-demo-setup.sh && DEMO_NAME=us013 tui_demo_prepare"
Enter
Sleep 200ms
Type "tui_demo_register_second_store"
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

# ── US-013: store picker / switch store from dashboard ──
# Launch the TUI — auto-resolves `demo/main` via default_store config.
# Header shows `demo/main · 2 envs`, left pane lists prod + staging.
Type "$HIMITSU_BIN"
Enter
Sleep 1.2s

# Press 's' from the dashboard to open the store picker overlay.
# Both stores are listed alphabetically: acme/infra, then demo/main.
# The current store (demo/main) is marked with `•` and the cursor
# starts on it (index 1).
Type "s"
Sleep 1.2s

# Move the list cursor UP to acme/infra (index 0).
Type "k"
Sleep 600ms

# Select acme/infra — picker closes, dashboard rebuilds against the
# new store. Header now reads `acme/infra · 1 env`, secrets pane
# shows `prod/SHARED_KEY` and `prod/SHARED_URL`.
Enter
Sleep 2s

# Reopen the picker to switch back to demo/main.
Type "s"
Sleep 1s

# Cursor starts on the current store (acme/infra, index 0); move
# DOWN to demo/main (index 1) and select it.
Type "j"
Sleep 500ms
Enter
Sleep 1s

# Demonstrate Esc: open the picker one more time and cancel with
# Escape — no change, still on demo/main.
Type "s"
Sleep 800ms
Escape
Sleep 800ms

# Quit.
Type "q"
Sleep 500ms
