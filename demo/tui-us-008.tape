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

# ── US-008: create a new secret with metadata ───────────
# Launch the TUI — the search view is the root, populated with all
# secrets from the seeded store (folder-grouped by path prefix).
Type "$HIMITSU_BIN"
Enter
Sleep 1.5s

# Open the new-secret form with Ctrl+N. The form starts focused on
# the path field; Tab/Shift-Tab cycle through path → value →
# description → tags → url → totp → env_key → expires_at.
Ctrl+N
Sleep 900ms

# Type the secret path.
Type "prod/STRIPE_KEY"
Sleep 600ms
Tab
Sleep 500ms

# Value field — multi-line buffer, so we use Tab (not Enter) to advance.
Type "sk_live_51HxYz2"
Sleep 600ms
Tab
Sleep 500ms

# Description.
Type "Stripe live secret key"
Sleep 500ms
Tab
Sleep 400ms

# Tags — comma-separated; only [A-Za-z0-9_.-,] accepted at typing time,
# and each tag is validated against the grammar on save.
Type "pci,stripe,rotate-2026-q1"
Sleep 500ms
Tab
Sleep 400ms

# URL.
Type "https://dashboard.stripe.com"
Sleep 500ms
Tab
Sleep 400ms

# TOTP — skip (empty stays unset).
Tab
Sleep 400ms

# env_key — the variable name `himitsu exec` will inject this under.
Type "STRIPE_SECRET_KEY"
Sleep 500ms
Tab
Sleep 400ms

# expires_at — accepts RFC 3339, relative (`30d`, `6mo`, `1y`), or `never`.
Type "1y"
Sleep 700ms

# Save with Ctrl+W (tmux-safe alternative to Ctrl+S).
Ctrl+W
Sleep 1.8s

# Quit.
Escape
Sleep 500ms
