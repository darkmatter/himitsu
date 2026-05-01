# 6. Generate typed bindings and show that bulk decrypt is rejected.
Type "h codegen --lang typescript --env prod --stdout"
Enter
Wait+Screen /HimitsuKey/
Sleep 1.2s
Screenshot target/vhs-out/screenshots/6-codegen.png

Type "clear"
Enter
Sleep 300ms

Type "h decrypt || true"
Enter
Wait+Screen /bulk decrypt is not supported/
Sleep 1.2s
Screenshot target/vhs-out/screenshots/7-no-bulk-decrypt.png
