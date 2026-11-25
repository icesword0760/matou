#!/bin/sh
set -eu
printf '%s\0' "$@" > "$CODEX_HOME/argv"
printf '%s\n' "$PWD" > "$CODEX_HOME/cwd"
printf '%s\n' "$CODEX_HOME" "${TERM:-}" "${OPENAI_BASE_URL:-}" > "$CODEX_HOME/environment"
if [ "${CODEX_FIXTURE_RESUME:-}" = 1 ]; then
    printf '%s\n' "${OPENAI_API_KEY:-}" >> "$CODEX_HOME/environment"
fi
printf '%s\n' "${MATOU_FOREMAN_ROLES_FILE:-}" "${MATOU_CONTROL_NODE_EXECUTABLE:-}" > "$CODEX_HOME/dispatch-environment"
test -z "${MATOU_PROVIDER_HOOK_TOKEN:-}${CLAUDECODE:-}"
test -d "${MATOU_AGENT_STATUS_DIR:?}"
case "$MATOU_AGENT_STATUS_DIR" in
    */agent-status/*) ;;
    *) exit 91 ;;
esac
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
if [ "${CODEX_FIXTURE_RESUME:-}" = 1 ]; then
    target=
    while [ "$#" -gt 1 ]; do
        if [ "$1" = resume ] && [ "$#" = 2 ]; then
            target=$2
            break
        fi
        shift
    done
    if [ -n "$target" ]; then
        if [ -f "$CODEX_HOME/silent-resume" ]; then exec /bin/sleep 60; fi
        material="$CODEX_HOME/sessions/rollout-fixture-$target.jsonl"
        if [ -f "$CODEX_HOME/early-exit" ]; then exit 23; fi
        if [ -f "$CODEX_HOME/background" ]; then
            printf 'That session is running in the background (%s).\n' "$(printf '%.8s' "$target")"
        elif [ ! -f "$material" ]; then
            printf '\033[31mNo session '
            /bin/sleep 0.08
            printf '%s found\033[0m\n' "$target"
        elif ! /usr/bin/perl -MJSON::PP -ne 'decode_json($_)' "$material" 2>/dev/null; then
            printf 'Invalid session %s\n' "$target"
        else
            printf 'FIXTURE CODEX RESUME\n'
            : > "$CODEX_HOME/resume-screen-ready"
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
                /bin/sh -c "$hook_command" < "$CODEX_HOME/$name.json" > /dev/null
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
