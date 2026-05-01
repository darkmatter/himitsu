# 2. Initialize an isolated store in headless mode.
Type "h init --no-tui"
Enter
Wait+Screen /Created state directory/
Sleep 1s
Screenshot target/vhs-out/screenshots/1-init.png

Hide
Set TypingSpeed 1ms
Type `git -C "$DEMO_STORE" remote add origin "$DEMO_HOME/origin.git"`
Enter
Sleep 100ms
Show
Set TypingSpeed 42ms

Type "clear"
Enter
Sleep 300ms
