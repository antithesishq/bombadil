#!/bin/sh
# A fake external driver for a counter app. It speaks protocol version 0
# (or the version in $PROTOCOL) and has a bug: nothing caps the count.
# With $STALL_MS set, it declares that stall timeout and then hangs.
# With $CHILD_PIDFILE set, it starts a long-running child and writes its pid
# there, without ever stopping it.
# With $EXTRAS set, it timestamps each state itself (one millisecond apart,
# from $EXTRAS microseconds), reports states after an increment as not
# settled, and doesn't apply a reset of a count that is already zero.
count=0
step=0
if [ -n "$CHILD_PIDFILE" ]; then
  sleep 1000 &
  echo $! >"$CHILD_PIDFILE"
fi
state() {
  screenshot=
  if [ -n "$SCREENSHOT_PNG" ] && [ "$count" -eq 0 ]; then
    screenshot=",\"screenshot_png\":\"$SCREENSHOT_PNG\""
  fi
  if [ -n "$EXTRAS" ]; then
    printf '{"type":"state","state":{"count":%d},"timestamp_us":%d%s%s}\n' \
      "$count" $((EXTRAS + step * 1000)) "$1" "$screenshot"
    step=$((step + 1))
  else
    printf '{"type":"state","state":{"count":%d}%s}\n' "$count" "$screenshot"
  fi
}
if [ -n "$STALL_MS" ]; then
  printf '{"type":"hello","protocol":0,"name":"counter","stall_timeout_ms":%d}\n' "$STALL_MS"
  read -r _
  exit 0
fi
printf '{"type":"hello","protocol":%d,"name":"counter"}\n' "${PROTOCOL:-0}"
state
while read -r line; do
  extra=
  case "$line" in
    *'"terminate"'*) exit 0 ;;
    *Increment*)
      by=$(printf '%s' "$line" | sed -n 's/.*"by":\([0-9]*\).*/\1/p')
      count=$((count + by))
      extra=',"settled":false'
      ;;
    *Reset*)
      if [ "$count" -eq 0 ]; then extra=',"not_applied":"already zero"'; fi
      count=0
      ;;
    *)
      printf '{"type":"error","message":"unknown action"}\n'
      exit 1
      ;;
  esac
  state "$extra"
done
