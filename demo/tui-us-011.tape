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

# ── US-011: drill into a secret from the dashboard ──────
# Launch the TUI — auto-resolves the seeded store from HIMITSU_HOME.
# Dashboard opens with the envs pane focused (cyan border), `prod`
# selected. The right pane previews prod's secrets with an inactive,
# greyed-out highlight.
Type "$HIMITSU_BIN"
Enter
Sleep 1.2s

# Tab — focus moves to the secrets pane. Cyan border jumps to the
# right; the left pane greys out. Selection lands on prod/API_KEY.
Tab
Sleep 600ms

# Navigate down to prod/DATABASE_URL.
Type "j"
Sleep 500ms

# And back up to prod/API_KEY.
Type "k"
Sleep 500ms

# Enter — open the secret viewer for prod/API_KEY. The decrypted
# value is rendered inside the viewer overlay.
Enter
Sleep 2s

# Escape back to the dashboard. Focus is preserved on the secrets
# pane with the same selection.
Escape
Sleep 800ms

# h — focus back to the envs pane.
Type "h"
Sleep 500ms

# j — select `staging`. The right pane swaps to staging's secrets.
Type "j"
Sleep 700ms

# l — focus back to the secrets pane (now listing staging).
Type "l"
Sleep 500ms

# Enter — open the viewer for staging/API_KEY.
Enter
Sleep 1.5s

# Escape back to the dashboard.
Escape
Sleep 500ms

# Quit.
Type "q"
Sleep 500ms
