# 3. Add secrets with metadata.
Type `h set prod/API_KEY sk_live_abc123 --description "Stripe production API key" --env-key STRIPE_API_KEY --url https://api.stripe.com --no-push`
Enter
Wait+Screen /API_KEY/
Sleep 500ms

Type `h set prod/DB_PASSWORD hunter2 --description "Production database password" --no-push`
Enter
Wait+Screen /DB_PASSWORD/
Sleep 500ms

Type `h set dev/DB_PASSWORD devpass --description "Developer database password" --no-push`
Enter
Wait+Screen /DB_PASSWORD/
Sleep 500ms

Type `h set common/SENTRY_DSN https://example@sentry.io/1 --description "Shared Sentry DSN" --no-push`
Enter
Wait+Screen /SENTRY_DSN/
Sleep 900ms
Screenshot target/vhs-out/screenshots/2-set.png

Type "clear"
Enter
Sleep 300ms
