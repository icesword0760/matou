#!/bin/sh
set -eu
printf '%s\0' "$@" > "$CODEX_HOME/argv"
printf '%s\n' "$PWD" > "$CODEX_HOME/cwd"
printf '%s\n' "$CODEX_HOME" "${TERM:-}" "${OPENAI_BASE_URL:-}" > "$CODEX_HOME/environment"
test -z "${MATOU_PROVIDER_HOOK_TOKEN:-}${MATOU_AGENT_STATUS_DIR:-}${CLAUDECODE:-}"
printf 'FIXTURE CODEX TUI\n'
if [ -n "${CODEX_FIXTURE_EXIT_CODE:-}" ]; then
    exit "$CODEX_FIXTURE_EXIT_CODE"
fi
IFS= read -r line
printf 'FIXTURE CODEX INPUT <%s>\n' "$line"
exec /bin/sleep 60
