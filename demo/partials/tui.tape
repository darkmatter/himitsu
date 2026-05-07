# Launch the app with no subcommands: the recorded demo is entirely TUI.
Hide
Type "$HIMITSU_BIN"
Enter
Show
Wait+Screen /results/
Sleep 1.8s
Screenshot target/vhs-out/screenshots/0-search.png

# Contextual help for the search dashboard.
Type "?"
Wait+Screen /filter results/
Sleep 1.5s
Screenshot target/vhs-out/screenshots/1-search-help.png
Escape
Wait+Screen /results/
Sleep 1.3s

# Fuzzy search, open a secret, and reveal its value on demand.
Type "api"
Wait+Screen /API_KEY/
Sleep 1.2s
Enter
Wait+Screen /metadata/
Sleep 1.4s
Screenshot target/vhs-out/screenshots/2-secret-metadata.png

Type "r"
Wait+Screen /sk_live_abc123/
Sleep 1.8s
Screenshot target/vhs-out/screenshots/3-secret-revealed.png

# Viewer help is scoped to the active view.
Type "?"
Wait+Screen /reveal/
Sleep 1.4s
Screenshot target/vhs-out/screenshots/4-secret-help.png
Escape
Wait+Screen /value/
Sleep 800ms
Escape
Wait+Screen /query/
Sleep 1s

# Create a new secret in the TUI.
Backspace
Backspace
Backspace
Sleep 300ms
Ctrl+N
Wait+Screen /new secret/
Sleep 700ms
Screenshot target/vhs-out/screenshots/5-new-secret-empty.png

Type "prod/STRIPE_KEY"
Tab
Type "sk_live_demo_456"
Tab
Type "Stripe checkout key"
Tab
Type "pci,stripe,rotate-2026-q1"
Tab
Type "https://dashboard.stripe.com"
Tab
Tab
Type "STRIPE_SECRET_KEY"
Tab
Type "1y"
Sleep 1.3s
Screenshot target/vhs-out/screenshots/6-new-secret-filled.png

Ctrl+W
Wait+Screen /STRIPE_KEY/
Sleep 1.2s
Screenshot target/vhs-out/screenshots/7-new-secret-created.png

# Store picker remains in the TUI and shows registered stores.
Ctrl+S
Wait+Screen /demo/
Sleep 1.4s
Screenshot target/vhs-out/screenshots/8-store-picker.png
Escape
Sleep 1s

Escape
Sleep 800ms
