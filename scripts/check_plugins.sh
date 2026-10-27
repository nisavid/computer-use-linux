#!/bin/sh
# Contract check for the Claude Code and Codex plugin under plugins/.
# With --binary, runs the HTTPS release-integrity fixtures and the MCP safety
# check through a copied plugin, covering download, trusted pins, tampering,
# cache reuse, and pruning without changing the shipped trust manifest.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
plugin="$root/plugins/computer-use-linux"
launcher="$plugin/bin/computer-use-linux"
cd "$root"

fail() {
  echo "check_plugins: $*" >&2
  exit 1
}

cargo_version=$(grep -m1 '^version = ' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
launcher_version=$(sed -n 's/^version=//p' "$launcher")
[ "$launcher_version" = "$cargo_version" ] ||
  fail "launcher version $launcher_version != Cargo version $cargo_version"

for manifest in \
  .claude-plugin/marketplace.json \
  "$plugin/.claude-plugin/plugin.json" \
  "$plugin/.codex-plugin/plugin.json" \
  "$plugin/codex-mcp.json"; do
  python3 -m json.tool "$manifest" >/dev/null || fail "invalid JSON: $manifest"
done

python3 - "$cargo_version" <<'PY'
import json, sys
version = sys.argv[1]
checks = {
    "plugins/computer-use-linux/.claude-plugin/plugin.json": lambda d: d["version"],
    "plugins/computer-use-linux/.codex-plugin/plugin.json": lambda d: d["version"],
    ".claude-plugin/marketplace.json": lambda d: d["plugins"][0]["version"],
}
for path, get in checks.items():
    with open(path) as f:
        found = get(json.load(f))
    if found != version:
        sys.exit(f"check_plugins: {path} version {found} != Cargo version {version}")
PY

diff -r skills/computer-use-linux "$plugin/skills/computer-use-linux" >/dev/null ||
  fail "plugins/computer-use-linux/skills is out of sync; run: rm -rf plugins/computer-use-linux/skills/computer-use-linux && cp -r skills/computer-use-linux plugins/computer-use-linux/skills/"

sh -n "$launcher"
[ -x "$launcher" ] || fail "launcher is not executable"

if [ "$#" -eq 0 ]; then
  echo "plugin manifests ok: version $cargo_version"
  exit 0
fi
[ "$#" -eq 2 ] && [ "$1" = "--binary" ] || fail "usage: scripts/check_plugins.sh [--binary PATH]"
node scripts/release_integrity_test.mjs --binary "$2"
echo "plugin launcher ok: download, tamper rejection, cache reuse, pruning"
