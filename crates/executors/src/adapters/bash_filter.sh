#!/bin/sh
# KanbAIn PreToolUse hook: lets a Bash call through only when its command
# matches one of the shell glob patterns in bash-allow.txt next to this
# script. Exit code 2 blocks the call and shows stderr to the agent.
allow_file="$(dirname "$0")/bash-allow.txt"
cmd=$(jq -r '.tool_input.command // ""')

deny() {
    echo "KanbAIn: $1. Allowed commands: $(paste -sd ',' "$allow_file" | sed 's/,/, /g')" >&2
    exit 2
}

# Redirecting stderr into stdout is harmless; any other redirection,
# chaining or substitution could run something outside the allowlist.
check=$(printf '%s' "$cmd" | sed 's/ 2>&1//g')
case "$check" in
*"
"* | *'&'* | *'|'* | *';'* | *'`'* | *'$('* | *'>'* | *'<'*)
    deny "this agent may not chain, pipe or redirect commands"
    ;;
esac

while IFS= read -r pattern || [ -n "$pattern" ]; do
    [ -z "$pattern" ] && continue
    # shellcheck disable=SC2254
    case "$check" in
    $pattern) exit 0 ;;
    esac
done <"$allow_file"

deny "this agent may not run \`$cmd\`"
