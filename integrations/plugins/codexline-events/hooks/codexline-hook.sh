#!/bin/sh

if [ -z "${CODEXLINE_HOOK_BIN:-}" ] || [ -z "${CODEXLINE_EVENT_ENDPOINT:-}" ]; then
  exit 0
fi

exec "$CODEXLINE_HOOK_BIN" hook
