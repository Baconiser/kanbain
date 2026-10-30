#!/bin/sh
# KanbAIn PreToolUse hook for agents without the edit capability: Write may
# only create their results under /kanbain/out (plan.json, questions.md, …),
# never touch the checkout. Exit code 2 blocks the call and shows stderr to
# the agent.
path=$(jq -r '.tool_input.file_path // .tool_input.notebook_path // ""')
case "$path" in
*..*) ;;
/kanbain/out/*) exit 0 ;;
esac
echo "KanbAIn: this agent may only write files under /kanbain/out/, not \`$path\`" >&2
exit 2
