#!/usr/bin/env bash
# Fake $EDITOR for US-010 demo: rewrites the file with a new value and exits.
printf 'sk_edited_in_tui\n' > "$1"
sleep 0.4
