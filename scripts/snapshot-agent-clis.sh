#!/bin/sh
# Regenerate the CLI interface snapshots the runtime adapters are checked
# against (crates/executors/src/adapters/cli_snapshots). Run after bumping a
# version in crates/executors/src/versions.rs and rebuilding the agent image:
#   scripts/snapshot-agent-clis.sh kanbain-agent:<tag>
set -eu
image=${1:?usage: $0 <kanbain-agent image>}
root=$(cd "$(dirname "$0")/.." && pwd)
out="$root/crates/executors/src/adapters/cli_snapshots"
versions="$root/crates/executors/src/versions.rs"
cc_version=$(sed -n 's/.*CLAUDE_CODE_VERSION: &str = "\(.*\)";/\1/p' "$versions")
oc_version=$(sed -n 's/.*OPENCODE_VERSION: &str = "\(.*\)";/\1/p' "$versions")
mkdir -p "$out"

# Prints the top-level keys of the zod object literal that starts at the
# given anchor text, one per line.
extract='
import re, sys
data = open(sys.argv[1], "rb").read().decode("latin1")
for title, anchor in zip(sys.argv[2::2], sys.argv[3::2]):
    start = data.index(anchor)
    depth, keys = 0, set()
    for i in range(start, len(data)):
        ch = data[i]
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            if depth == 0:
                break
            depth -= 1
        elif depth == 0 and ch == ":":
            m = re.search(r"([A-Za-z_$][\w$]*)$", data[max(start, i - 64):i])
            if m:
                keys.add(m.group(1))
    print("## " + title)
    print("\n".join(sorted(keys)))
'

docker run --rm --network none "$image" sh -c '
  cc=$(find ~/.npm/_npx -path "*@anthropic-ai/claude-code/bin/claude.exe" | head -1)
  echo "version: $("$cc" --version | cut -d" " -f1)"
  "$cc" --help
  echo "## hidden flags"
  grep -ao -e "--max-turns <turns>" "$cc" | head -1
  python3 -I -c "$1" "$cc" "--agents fields" \
    "description:E.string().min(1,\"Description cannot be empty\")"
' sh "$extract" >"$out/claude-code.txt"

docker run --rm --network none "$image" sh -c '
  oc=$(find ~/.npm/_npx -path "*opencode-linux-x64/bin/opencode" | head -1)
  echo "version: $("$oc" --version)"
  "$oc" serve --help 2>&1
  echo
  python3 -I -c "$1" "$oc" \
    "agent fields" "model:uZ.optional(),variant:q.string().optional().describe(\"Default model variant" \
    "permission keys" "__originalKeys:q.string().array().optional()," \
    "provider options" "apiKey:q.string().optional(),baseURL:q.string().optional()"
  echo "## config fields"
  grep -ao "instructions:q.array(q.string()).optional()" "$oc" | head -1 | cut -d: -f1
' sh "$extract" >"$out/opencode.txt"

grep -q "^version: $cc_version" "$out/claude-code.txt" || echo "warning: claude-code snapshot is not $cc_version"
grep -q "^version: $oc_version" "$out/opencode.txt" || echo "warning: opencode snapshot is not $oc_version"
echo "Snapshots written to $out"
