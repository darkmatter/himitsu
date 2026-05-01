# 5. Add a recipient and re-encrypt for the recipient set.
Type "h recipient ls"
Enter
Wait+Screen /self/
Sleep 700ms

Type `h recipient add common/alice --age-key "$(cat "$DEMO_HOME/share/key.pub")" --description "Alice laptop"`
Enter
Wait+Screen /Added recipient/
Sleep 700ms

Type "h recipient ls"
Enter
Wait+Screen /alice/
Sleep 900ms

Type "h rekey --force"
Enter
Wait+Screen /Re-encrypted 4 secret\(s\) for 1 recipient\(s\)/
Sleep 900ms
Screenshot target/vhs-out/screenshots/5-recipients.png

Type "clear"
Enter
Sleep 300ms
