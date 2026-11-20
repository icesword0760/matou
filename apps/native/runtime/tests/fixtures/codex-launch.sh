#!/bin/sh
set -eu
printf '%s\0' "$@" > "$CODEX_HOME/argv"
printf '%s\n' "$PWD" > "$CODEX_HOME/cwd"
printf '%s\n' "$CODEX_HOME" "${TERM:-}" "${OPENAI_BASE_URL:-}" > "$CODEX_HOME/environment"
test -z "${MATOU_PROVIDER_HOOK_TOKEN:-}${MATOU_AGENT_STATUS_DIR:-}${CLAUDECODE:-}"
if [ "${CODEX_FIXTURE_EVENTS:-}" = 1 ]; then
    hook_command=$(/usr/bin/perl -MJSON::PP -0777 -ne '
        binmode STDOUT, ":encoding(UTF-8)";
        print decode_json($1) if /(?:^|\x00)hooks=.*?command = ("(?:\\.|[^"\\])*")/
    ' "$CODEX_HOME/argv")
    test -n "$hook_command"
    if [ -f "$CODEX_HOME/startup-hook.json" ]; then
        /bin/sh -c "$hook_command" < "$CODEX_HOME/startup-hook.json"
        : > "$CODEX_HOME/startup-hook.sent"
        if [ -n "${CODEX_FIXTURE_EXIT_CODE:-}" ]; then
            exit "$CODEX_FIXTURE_EXIT_CODE"
        fi
    fi
fi
printf 'FIXTURE CODEX TUI\n'
if [ -n "${CODEX_FIXTURE_EXIT_CODE:-}" ]; then
    exit "$CODEX_FIXTURE_EXIT_CODE"
fi
if [ "${CODEX_FIXTURE_EVENTS:-}" = 1 ]; then
    while IFS= read -r line; do
        case "$line" in
            hook\ *)
                name=${line#hook }
                /bin/sh -c "$hook_command" < "$CODEX_HOME/$name.json"
                printf 'HOOK SENT <%s>\n' "$name"
                ;;
            noise)
                printf 'tool exited 1; Conversation interrupted is documentation\n'
                printf '\033]133;D;1\007'
                printf 'model error: Server timed out\n'
                ;;
            interrupt)
                printf '\033[31m■ Conversation inter'
                /bin/sleep 0.08
                printf 'rupted\033[0m\n'
                ;;
            quit) exit 0 ;;
        esac
    done
    exit 0
fi
IFS= read -r line
printf 'FIXTURE CODEX INPUT <%s>\n' "$line"
exec /bin/sleep 60
