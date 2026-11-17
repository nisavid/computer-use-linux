#!/usr/bin/env bash
set -euo pipefail
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
entrypoint="$root/.agents/upstream-drift.sh"
fixture="$(mktemp -d)"
trap 'rm -rf -- "$fixture"' EXIT
mkdir "$fixture/bin"
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_ALLOW_PROTOCOL=file
export RELEASE_TEST_FIXTURE="$fixture" PATH="$fixture/bin:$PATH" GH_HOST=other.example.test
git init --quiet --initial-branch=main --object-format=sha1 "$fixture/upstream"
printf '[package]\nname = "computer-use-linux"\nversion = "0.7.47"\n' >"$fixture/upstream/Cargo.toml"
git -C "$fixture/upstream" add Cargo.toml
git -C "$fixture/upstream" -c user.name=Fixture -c user.email=fixture@example.test commit --quiet -m 'Fixture release source'
git -C "$fixture/upstream" -c user.name=Fixture -c user.email=fixture@example.test tag -a v0.7.47 -m 'Fixture annotated tag'
commit="$(git -C "$fixture/upstream" rev-parse HEAD)"
tag_object="$(git -C "$fixture/upstream" rev-parse refs/tags/v0.7.47)"
git clone --quiet --no-tags "$fixture/upstream" "$fixture/fork"
git -C "$fixture/fork" remote add upstream "$fixture/upstream"
git -C "$fixture/fork" fetch --quiet --no-tags upstream refs/heads/main:refs/remotes/upstream/main
git -C "$fixture/fork" tag v0.7.47 "$commit"
git -C "$fixture/fork" tag fork-only "$commit"
git -C "$fixture/fork" config --add remote.upstream.fetch '+refs/tags/*:refs/tags/*'
git -C "$fixture/fork" config remote.upstream.prune true
git -C "$fixture/fork" config remote.upstream.pruneTags true
git -C "$fixture/upstream" tag upstream-only "$commit"
cat >"$fixture/release.json" <<'JSON'
{"id":47,"tag_name":"v0.7.47","draft":false,"prerelease":false,"published_at":"2026-11-05T00:00:00Z","body":"Fixture notes\n\n"}
JSON
printf '{"ref":"refs/tags/v0.7.47","object":{"type":"tag","sha":"%s"}}\n' "$tag_object" >"$fixture/ref.json"
cat >"$fixture/bin/gh" <<'SH'
#!/bin/sh
set -eu
case "$*" in
  'api --hostname github.com repos/agent-sh/computer-use-linux/releases/latest --jq @base64')
    python3 -c 'import base64,json,os,pathlib; p=pathlib.Path(os.environ["RELEASE_TEST_FIXTURE"]); v=json.loads((p / "release.json").read_text()); t=p / "selected-tag"; v["tag_name"] = t.read_bytes()[:-1].decode() if t.exists() else v["tag_name"]; print(base64.b64encode(json.dumps(v).encode()).decode())'
    if [ -f "$RELEASE_TEST_FIXTURE/change-on-selection" ]; then touch "$RELEASE_TEST_FIXTURE/selection-seen"; fi ;;
  'api --hostname github.com repos/agent-sh/computer-use-linux/releases/latest --jq .tag_name | @base64')
    if [ -f "$RELEASE_TEST_FIXTURE/selected-tag" ]; then
      head -c -1 "$RELEASE_TEST_FIXTURE/selected-tag" | base64 -w 0
    else
      printf '%s' v0.7.47 | base64 -w 0
    fi
    printf '\n' ;;
  'api --hostname github.com repos/agent-sh/computer-use-linux/releases/latest')
    if [ -f "$RELEASE_TEST_FIXTURE/selection-seen" ]; then cat "$RELEASE_TEST_FIXTURE/release.changed"
    elif [ -f "$RELEASE_TEST_FIXTURE/change-release" ]; then
      if [ -f "$RELEASE_TEST_FIXTURE/release-seen" ]; then cat "$RELEASE_TEST_FIXTURE/release.changed"; else touch "$RELEASE_TEST_FIXTURE/release-seen"; cat "$RELEASE_TEST_FIXTURE/release.json"; fi
    else cat "$RELEASE_TEST_FIXTURE/release.json"; fi ;;
  'api --hostname github.com repos/agent-sh/computer-use-linux/git/ref/tags/v0.7.47')
    if [ -f "$RELEASE_TEST_FIXTURE/change-ref" ]; then
      if [ -f "$RELEASE_TEST_FIXTURE/ref-seen" ]; then cat "$RELEASE_TEST_FIXTURE/ref.changed"; else touch "$RELEASE_TEST_FIXTURE/ref-seen"; cat "$RELEASE_TEST_FIXTURE/ref.json"; fi
    else cat "$RELEASE_TEST_FIXTURE/ref.json"; fi ;;
  *) printf 'Unexpected GitHub fixture invocation %s\n' "$*" >&2; exit 97 ;;
esac
SH
chmod +x "$fixture/bin/gh"
cd "$fixture/fork"
run_case() {
  local case_name="$1" case_dir result
  shift
  case_dir="$fixture/cases/$case_name"
  mkdir -p "$case_dir"
  cp "$fixture/release.json" "$case_dir/release.json"
  cp "$fixture/ref.json" "$case_dir/ref.json"
  [[ ! -f "$fixture/selected-tag" ]] || cp "$fixture/selected-tag" "$case_dir/selected-tag"
  [[ ! -f "$fixture/release.changed" ]] || cp "$fixture/release.changed" "$case_dir/release.changed"
  [[ ! -f "$fixture/ref.changed" ]] || cp "$fixture/ref.changed" "$case_dir/ref.changed"
  git -C "$fixture/upstream" show-ref >"$case_dir/upstream-refs"
  git show-ref >"$case_dir/fork-refs-before"
  printf '%q ' bash "$entrypoint" "$@" >"$case_dir/invocation"
  printf '\n' >>"$case_dir/invocation"
  result=0
  bash "$entrypoint" "$@" >"$case_dir/stdout" 2>"$case_dir/stderr" || result=$?
  printf '%s\n' "$result" >"$case_dir/exit"
  git show-ref >"$case_dir/fork-refs-after"
  cmp "$case_dir/fork-refs-before" "$case_dir/fork-refs-after" || {
    printf '%s\n' 'Fixture failed: fork refs changed' >&2
    exit 98
  }
  cat "$case_dir/stdout"
  cat "$case_dir/stderr" >&2
  return "$result"
}
run_case current-drift --report "$fixture/drift.md" >"$fixture/drift.out"
cat >"$fixture/drift.expected" <<EXPECTED
status=current
behind=0
ahead=0
oldest_missing_age_days=0
stale_days=14
fork_sha=$commit
upstream_sha=$commit
fork_release=fork-only
release_tag=v0.7.47
release_contained=true
EXPECTED
diff -u "$fixture/drift.expected" "$fixture/drift.out"
[[ -s "$fixture/drift.md" ]]
printf '%s\n' 'Ordinary drift control passed'
printf '#!/bin/sh\nprintf "unexpected python invocation\\n" >&2\nexit 91\n' >"$fixture/bin/python3"
chmod +x "$fixture/bin/python3"
export release_record_mode=true
run_case inherited-mode-drift --report "$fixture/mode.md" >"$fixture/mode.out"
unset release_record_mode
rm "$fixture/bin/python3"
diff -u "$fixture/drift.expected" "$fixture/mode.out"
[[ -s "$fixture/mode.md" ]]
printf '%s\n' 'Inherited mode drift control passed'
record="$fixture/release-input.json"
run_case annotated-record --release-input "$record"
python3 - "$record" "$tag_object" "$commit" "$entrypoint" <<'PY'
import hashlib, json, sys
from pathlib import Path
record, tag, commit, entrypoint = sys.argv[1:]
value = json.loads(Path(record).read_bytes())
assert value['schema'] == 'computer-use-linux/upstream-release-input/v1'
assert value['upstream_repository'] == 'agent-sh/computer-use-linux'
assert value['release'] == {'id': 47, 'tag': 'v0.7.47', 'draft': False, 'prerelease': False, 'published_at': '2026-11-05T00:00:00Z', 'body': 'Fixture notes\n\n', 'body_sha256': hashlib.sha256(b'Fixture notes\n\n').hexdigest()}
assert value['source'] == {'tag_object': tag, 'tag_object_type': 'tag', 'commit': commit, 'cargo_version': '0.7.47', 'cargo_sha256': hashlib.sha256(Path('Cargo.toml').read_bytes()).hexdigest()}
assert value['procedure'] == {'resolver_sha256': hashlib.sha256(Path(entrypoint).with_name('upstream-release-input.py').read_bytes()).hexdigest(), 'drift_entrypoint_sha256': hashlib.sha256(Path(entrypoint).read_bytes()).hexdigest()}
PY
[[ "$(git rev-parse refs/tags/v0.7.47)" == "$commit" ]]
[[ "$(git rev-parse refs/tags/fork-only)" == "$commit" ]]
printf '%s\n' 'v0.7.47:refs/tags/unintended' >"$fixture/selected-tag"
malformed_status=0
run_case malformed-selector --release-input "$fixture/malformed-selector.json" >"$fixture/malformed-selector.stdout" 2>"$fixture/malformed-selector.stderr" || malformed_status=$?
cmp "$fixture/cases/malformed-selector/fork-refs-before" "$fixture/cases/malformed-selector/fork-refs-after"
[[ "$malformed_status" == 1 && ! -e "$fixture/malformed-selector.json" ]]
grep -q 'invalid selected release tag' "$fixture/malformed-selector.stderr"
rm "$fixture/selected-tag"
printf 'v0.7.47\n\n' >"$fixture/selected-tag"
newline_status=0
run_case newline-selector --release-input "$fixture/newline-selector.json" >"$fixture/newline-selector.stdout" 2>"$fixture/newline-selector.stderr" || newline_status=$?
if [[ "$newline_status" != 1 || -e "$fixture/newline-selector.json" ]]; then
  printf 'Malformed newline selector produced exit %s; record exists: ' "$newline_status" >&2
  if [[ -e "$fixture/newline-selector.json" ]]; then printf 'yes\n' >&2; else printf 'no\n' >&2; fi
  exit 1
fi
grep -q 'invalid selected release tag' "$fixture/newline-selector.stderr"
rm "$fixture/selected-tag"
printf 'v0.7\000.47\n' >"$fixture/selected-tag"
nul_status=0
run_case nul-selector --release-input "$fixture/nul-selector.json" >"$fixture/nul-selector.stdout" 2>"$fixture/nul-selector.stderr" || nul_status=$?
[[ "$nul_status" == 1 && ! -e "$fixture/nul-selector.json" ]]
grep -q 'invalid selected release tag' "$fixture/nul-selector.stderr"
rm "$fixture/selected-tag"
cp "$fixture/release.json" "$fixture/release.original"
for invalid in draft prerelease unpublished malformed_timestamp missing_body; do
  cp "$fixture/release.original" "$fixture/release.json"
  python3 - "$fixture/release.json" "$invalid" <<'PY'
import json, sys
from pathlib import Path
p, case = Path(sys.argv[1]), sys.argv[2]
v = json.loads(p.read_text())
if case == 'draft': v['draft'] = True
elif case == 'prerelease': v['prerelease'] = True
elif case == 'unpublished': v['published_at'] = None
elif case == 'malformed_timestamp': v['published_at'] = '2026-1-5T0:0:0Z'
elif case == 'missing_body': del v['body']
p.write_text(json.dumps(v))
PY
  if run_case "refused-$invalid" --release-input "$fixture/refused-$invalid.json" >"$fixture/$invalid.stdout" 2>"$fixture/$invalid.stderr"; then
    printf 'Accepted invalid release: %s\n' "$invalid" >&2
    exit 1
  fi
  [[ ! -e "$fixture/refused-$invalid.json" && -s "$fixture/$invalid.stderr" ]]
done
cp "$fixture/release.original" "$fixture/release.json"
cp "$record" "$fixture/original-record.json"
existing_status=0
run_case existing-record --release-input "$record" >"$fixture/existing.stdout" 2>"$fixture/existing.stderr" || existing_status=$?
[[ "$existing_status" == 1 ]]
grep -q 'File exists' "$fixture/existing.stderr"
cmp "$record" "$fixture/original-record.json"
printf '%s\n' 'Invalid release and existing-record controls passed'
for change in id notes; do
  python3 - "$fixture/release.json" "$fixture/release.changed" "$change" <<'PY'
import json, sys
from pathlib import Path
value = json.loads(Path(sys.argv[1]).read_text())
if sys.argv[3] == 'id': value['id'] += 1
else: value['body'] = 'Changed after selection'
Path(sys.argv[2]).write_text(json.dumps(value))
PY
  touch "$fixture/change-on-selection"
  selection_status=0
  run_case "selection-$change" --release-input "$fixture/selection-$change.json" >"$fixture/selection-$change.stdout" 2>"$fixture/selection-$change.stderr" || selection_status=$?
  [[ "$selection_status" == 1 && ! -e "$fixture/selection-$change.json" ]]
  grep -q 'latest release changed after selection' "$fixture/selection-$change.stderr"
  rm "$fixture/selection-seen"
  selection_status=0
  run_case "selection-$change-existing" --release-input "$record" >"$fixture/selection-$change-existing.stdout" 2>"$fixture/selection-$change-existing.stderr" || selection_status=$?
  [[ "$selection_status" == 1 ]]
  grep -q 'latest release changed after selection' "$fixture/selection-$change-existing.stderr"
  cmp "$record" "$fixture/original-record.json"
  rm "$fixture/change-on-selection" "$fixture/selection-seen"
done
printf '%s\n' 'Selection identity controls passed'
python3 - "$fixture/release.json" <<'PY'
import json, sys
from pathlib import Path
path = Path(sys.argv[1])
value = json.loads(path.read_text())
value['body'] = 'x' * 150_000
path.write_text(json.dumps(value))
PY
run_case large-notes --release-input "$fixture/large-notes.json"
python3 - "$fixture/large-notes.json" <<'PY'
import json, sys
from pathlib import Path
assert json.loads(Path(sys.argv[1]).read_text())['release']['body'] == 'x' * 150_000
PY
cp "$fixture/release.original" "$fixture/release.json"
python3 - "$fixture/release.json" "$fixture/release.changed" <<'PY'
import json, sys
from pathlib import Path
value = json.loads(Path(sys.argv[1]).read_text())
value['body'] = 'Changed after first observation'
Path(sys.argv[2]).write_text(json.dumps(value))
PY
touch "$fixture/change-release"
if run_case moving-release --release-input "$fixture/moving.json" >"$fixture/moving.stdout" 2>"$fixture/moving.stderr"; then
  printf '%s\n' 'Accepted a changing release' >&2
  exit 1
fi
[[ ! -e "$fixture/moving.json" && -s "$fixture/moving.stderr" ]]
run_case moving-release-drift --report "$fixture/moving-release-drift.md" >"$fixture/moving-release-drift.out"
[[ "$(sed -n 's/^status=//p' "$fixture/moving-release-drift.out")" == current ]]
rm "$fixture/change-release" "$fixture/release-seen"
git -C "$fixture/upstream" tag -d v0.7.47 >/dev/null
git -C "$fixture/upstream" tag v0.7.47 "$commit"
printf '{"ref":"refs/tags/v0.7.47","object":{"type":"commit","sha":"%s"}}\n' "$commit" >"$fixture/ref.json"
run_case lightweight-record --release-input "$fixture/lightweight.json"
python3 - "$fixture/lightweight.json" "$commit" <<'PY'
import json, sys
from pathlib import Path
source = json.loads(Path(sys.argv[1]).read_text())['source']
assert source['tag_object_type'] == 'commit'
assert source['tag_object'] == source['commit'] == sys.argv[2]
PY
[[ "$(git rev-parse refs/tags/v0.7.47)" == "$commit" ]]
[[ "$(git rev-parse refs/tags/fork-only)" == "$commit" ]]
printf '%s\n' 'Changing-release and lightweight-tag controls passed'
printf '{"ref":"refs/tags/v0.7.47","object":{"type":"commit","sha":"%040d"}}\n' 1 >"$fixture/ref.changed"
touch "$fixture/change-ref"
if run_case moving-ref --release-input "$fixture/moving-ref.json" >"$fixture/moving-ref.stdout" 2>"$fixture/moving-ref.stderr"; then
  printf '%s\n' 'Accepted a changing tag reference' >&2
  exit 1
fi
[[ ! -e "$fixture/moving-ref.json" && -s "$fixture/moving-ref.stderr" ]]
rm "$fixture/change-ref" "$fixture/ref-seen"
printf 'later upstream work\n' >"$fixture/upstream/later.txt"
git -C "$fixture/upstream" add later.txt
git -C "$fixture/upstream" -c user.name=Fixture -c user.email=fixture@example.test commit --quiet -m 'Later upstream work'
git fetch --quiet --no-tags upstream refs/heads/main:refs/remotes/upstream/main
run_case trailing-drift --report "$fixture/trailing.md" >"$fixture/trailing.out"
[[ "$(sed -n 's/^status=//p' "$fixture/trailing.out")" == trailing ]]
[[ "$(sed -n 's/^behind=//p' "$fixture/trailing.out")" == 1 ]]
[[ "$(sed -n 's/^release_contained=//p' "$fixture/trailing.out")" == true ]]
[[ -s "$fixture/trailing.md" ]]
later_commit="$(git -C "$fixture/upstream" rev-parse HEAD)"
git -C "$fixture/upstream" tag -f v0.7.47 "$later_commit" >/dev/null
printf '{"ref":"refs/tags/v0.7.47","object":{"type":"commit","sha":"%s"}}\n' "$later_commit" >"$fixture/ref.json"
run_case drifted --report "$fixture/drifted.md" >"$fixture/drifted.out"
[[ "$(sed -n 's/^status=//p' "$fixture/drifted.out")" == drifted ]]
[[ "$(sed -n 's/^release_contained=//p' "$fixture/drifted.out")" == false ]]
[[ -s "$fixture/drifted.md" ]]
printf '%s\n' 'Changing-ref, trailing and drifted controls passed'
printf '[package]\nname = "computer-use-linux"\nversion = "0.7.48"\n' >"$fixture/upstream/Cargo.toml"
git -C "$fixture/upstream" add Cargo.toml
git -C "$fixture/upstream" -c user.name=Fixture -c user.email=fixture@example.test commit --quiet -m 'Mismatch version'
mismatch_commit="$(git -C "$fixture/upstream" rev-parse HEAD)"
git -C "$fixture/upstream" tag -f v0.7.47 "$mismatch_commit" >/dev/null
printf '{"ref":"refs/tags/v0.7.47","object":{"type":"commit","sha":"%s"}}\n' "$mismatch_commit" >"$fixture/ref.json"
if run_case mismatch-record --release-input "$fixture/mismatch.json" >"$fixture/mismatch.stdout" 2>"$fixture/mismatch.stderr"; then
  printf '%s\n' 'Accepted a Cargo and tag version mismatch' >&2
  exit 1
fi
[[ ! -e "$fixture/mismatch.json" ]]
grep -q 'Cargo package version' "$fixture/mismatch.stderr"
run_case mismatch-drift --report "$fixture/mismatch-drift.md" >"$fixture/mismatch-drift.out"
[[ "$(sed -n 's/^status=//p' "$fixture/mismatch-drift.out")" == drifted ]]
[[ "$(sed -n 's/^release_contained=//p' "$fixture/mismatch-drift.out")" == false ]]
blob="$(git -C "$fixture/upstream" rev-parse 'HEAD:Cargo.toml')"
git -C "$fixture/upstream" tag -f v0.7.47 "$blob" >/dev/null
printf '{"ref":"refs/tags/v0.7.47","object":{"type":"blob","sha":"%s"}}\n' "$blob" >"$fixture/ref.json"
if run_case blob-record --release-input "$fixture/blob.json" >"$fixture/blob.stdout" 2>"$fixture/blob.stderr"; then
  printf '%s\n' 'Accepted a non-commit tag target' >&2
  exit 1
fi
[[ ! -e "$fixture/blob.json" && -s "$fixture/blob.stderr" ]]
printf '%s\n' 'Version mismatch and non-commit target controls passed'
if [[ -n "${RELEASE_TEST_EVIDENCE_DIR:-}" ]]; then
  [[ ! -e "$RELEASE_TEST_EVIDENCE_DIR" ]] || {
    printf '%s\n' 'Evidence destination already exists' >&2
    exit 1
  }
  cp -a "$fixture" "$RELEASE_TEST_EVIDENCE_DIR"
fi
