Output demo/demo-vhs.gif

Set Theme { "name": "Minted", "black": "#0B0A0F", "red": "#963543", "green": "#709C86", "yellow": "#C7B485", "blue": "#31457D", "magenta": "#B86EB8", "cyan": "#527180", "white": "#76788C", "brightBlack": "#96a5b6", "brightRed": "#E61C6B", "brightGreen": "#709C86", "brightYellow": "#B39864", "brightBlue": "#33b3cc", "brightMagenta": "#5F5887", "brightCyan": "#b3e6de", "brightWhite": "#b1b1bf", "background": "#0C0C13", "foreground": "#afd1e9", "selection": "#181E2D", "cursor": "#69E7B8" }
Set Shell "bash"
Set FontSize 32
Set Width 1200
Set Height 600
Set Padding 24
# Set LineHeight 1.6
Set Margin 60
Set MarginFill "#0c0c13"
Set BorderRadius 10


# ── Boilerplate setup (flies by at 1ms/char, then clears) ────────
Set TypingSpeed 1ms
Hide
Type "HIMITSU_BIN=$PWD/target/release/himitsu"
Enter
Type "[ -x $HIMITSU_BIN ] || { echo 'missing binary – run cargo build --release'; exit 1; }"
Enter
Type "SHIM=$(mktemp -d) && ln -s $HIMITSU_BIN $SHIM/himitsu && export PATH=$SHIM:$PATH"
Enter
Type "export DEMO_HOME=/tmp/hdemo && export HIMITSU_HOME=/tmp/hdemo"
Enter
Type "DEMO_STORE=$DEMO_HOME/store/.himitsu"
Enter
Type "rm -rf $DEMO_HOME && mkdir -p $DEMO_HOME && cd $DEMO_HOME"
Enter
Type `h() { himitsu -s $DEMO_STORE "$@"; }`
Enter
Sleep 100ms
Type "clear"
Enter
Sleep 600ms
Show

Set TypingSpeed 45ms

# ── 1. Help ──────────────────────────────────────────────────────
Type "himitsu --help"
Enter 2
Sleep 2.5s

# ── 2. Initialize ────────────────────────────────────────────────
Type "h init"
Enter 2
Sleep 1.8s
Set TypingSpeed 1ms
Type "PUBKEY=$(cat $DEMO_STORE/recipients/common/self.pub)"
Enter 2
Sleep 100ms
Set TypingSpeed 45ms
Sleep 200ms

# ── 3. Set secrets ───────────────────────────────────────────────
Type "h set prod/API_KEY sk_live_abc123"
Enter 2
Sleep 800ms

Type "h set prod/DB_PASSWORD hunter2"
Enter 2
Sleep 800ms

Type "h set dev/DB_PASSWORD devpass"
Enter 2
Sleep 800ms

# ── 4. Get secrets ───────────────────────────────────────────────
Type "h get prod/API_KEY; echo"
Enter 2
Sleep 800ms

Type "h get dev/DB_PASSWORD; echo"
Enter 2
Sleep 800ms

# ── 5. List ──────────────────────────────────────────────────────
Type "h ls"
Enter 2
Sleep 800ms

Type "h ls prod"
Enter 2
Sleep 800ms

# ── 6. Re-encrypt for current recipients ─────────────────────────
Type "h rekey"
Enter 2
Sleep 1.2s

# ── 7. Recipient management ──────────────────────────────────────
Type "h recipient ls"
Enter 2
Sleep 800ms

Type "h recipient add alice --age-key $PUBKEY --group common"
Enter 2
Sleep 1s

Type "h recipient ls"
Enter 2
Sleep 800ms

# ── 8. Group management ──────────────────────────────────────────
Type "h group add ops"
Enter 2
Sleep 700ms

Type "h group ls"
Enter 2
Sleep 800ms

# ── 9. Search ────────────────────────────────────────────────────
Type "h search DB --refresh"
Enter 2
Sleep 1.2s

# ── 10. Codegen ──────────────────────────────────────────────────
Type "h codegen --lang typescript --env prod --stdout"
Enter 2
Sleep 1.8s

# ── 11. Decrypt is rejected ──────────────────────────────────────
Type "h decrypt"
Enter 2
Sleep 1.2s

# ── 12. File layout ──────────────────────────────────────────────
Type `find $DEMO_STORE -type f | sed "s|$DEMO_HOME|~|" | sort`
Enter 2
Sleep 1.5s

Set TypingSpeed 1ms
Type "rm -rf $DEMO_HOME $SHIM"
Enter 2
Sleep 200ms
