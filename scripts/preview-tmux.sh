#!/bin/sh
# Test-only adapter. Always addresses the harness's private socket and execs
# tmux so cancellation kills the actual client, not an intermediate shell.
case " $* " in
  *" -C attach-session "*) printf 'stream\n' >> "$AWB_STRESS_LOG" ;;
  *" capture-pane -p -e -S -200 "*) printf 'poll\n' >> "$AWB_STRESS_LOG" ;;
esac
exec "$AWB_STRESS_TMUX" -S "$AWB_STRESS_SOCKET" "$@"
