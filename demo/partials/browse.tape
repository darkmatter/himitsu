# 4. Read, list, and search without writing plaintext to disk.
Type `h read prod/API_KEY; printf "\n"`
Enter
Wait+Screen /sk_live_abc123/
Sleep 900ms
Screenshot target/vhs-out/screenshots/3-read.png

Type "h ls -R"
Enter
Wait+Screen /SENTRY_DSN/
Sleep 900ms

Type "h search DB --refresh"
Enter
Wait+Screen /DB_PASSWORD/
Sleep 900ms
Screenshot target/vhs-out/screenshots/4-list-search.png

Type "clear"
Enter
Sleep 300ms
