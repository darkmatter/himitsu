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

# ── US-013: store picker / switch store from search ─────
# Launch the TUI. The search view spans every registered store —
# results from `demo/main` and `acme/infra` appear together with
# per-store header rows (multi-store grouping, hm-57k).
Type "$HIMITSU_BIN"
Enter
Sleep 1.8s

# Open the store picker overlay with Ctrl+S. Both stores listed
# alphabetically: acme/infra and demo/main; the cursor starts on
# the active store (demo/main).
Ctrl+S
Sleep 1.2s

# Move UP to acme/infra.
Up
Sleep 600ms

# Select it — picker closes and the search view rebuilds against
# the new active store. (Multi-store mode still surfaces the other
# store's secrets, but acme/infra is now the default for new writes.)
Enter
Sleep 1.8s

# Reopen the picker and switch back to demo/main.
Ctrl+S
Sleep 1s
Down
Sleep 500ms
Enter
Sleep 1.2s

# One more open and Esc to demonstrate cancel.
Ctrl+S
Sleep 800ms
Escape
Sleep 800ms

# Quit.
Escape
Sleep 500ms
