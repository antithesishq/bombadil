#!/bin/sh
# A fake external driver for a counter app. It speaks protocol version 0
# (or the version in $PROTOCOL) and has a bug: nothing caps the count.
# With $STALL_MS set, it declares that stall timeout and then hangs.
count=0
state() {
  printf '{"type":"state","state":{"count":%d}}\n' "$count"
}
if [ -n "$STALL_MS" ]; then
  printf '{"type":"hello","protocol":0,"name":"counter","stall_timeout_ms":%d}\n' "$STALL_MS"
  read -r _
  exit 0
fi
printf '{"type":"hello","protocol":%d,"name":"counter"}\n' "${PROTOCOL:-0}"
state
while read -r line; do
  case "$line" in
    *'"terminate"'*) exit 0 ;;
    *Increment*)
      by=$(printf '%s' "$line" | sed -n 's/.*"by":\([0-9]*\).*/\1/p')
      count=$((count + by))
      ;;
    *Reset*) count=0 ;;
    *)
      printf '{"type":"error","message":"unknown action"}\n'
      exit 1
      ;;
  esac
  state
done
