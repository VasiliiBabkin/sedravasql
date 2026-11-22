#!/usr/bin/env bash
set -euo pipefail

command -v cargo >/dev/null || { echo "cargo not installed" >&2; exit 1; }
command -v python3 >/dev/null || { echo "python3 not installed" >&2; exit 1; }

cd "$(dirname "$0")/.."
cargo build --quiet
bin="./target/debug/sedravasql"
work="$(mktemp -d)"
trap 'rm -rf -- "$work"' EXIT
db="$work/fixture.db"

"$bin" init --db "$db" >/dev/null
"$bin" put --db "$db" --key "trial:1" --value "first" >/dev/null
"$bin" put --db "$db" --key "trial:2" --value "second" >/dev/null

first_preview="$("$bin" preview-delete --db "$db" --prefix "trial:")"
stale_token="$(printf '%s' "$first_preview" | python3 -c 'import sys,json; print(json.load(sys.stdin)["token"])')"

"$bin" put --db "$db" --key "trial:2" --value "changed" >/dev/null

if "$bin" apply-delete --db "$db" --prefix "trial:" --token "$stale_token" >"$work/should_not_succeed" 2>"$work/error"; then
  echo "FAIL: stale preview accepted" >&2
  exit 1
fi
grep -q 'stale preview' "$work/error"

fresh_preview="$("$bin" preview-delete --db "$db" --prefix "trial:")"
token="$(printf '%s' "$fresh_preview" | python3 -c 'import sys,json; print(json.load(sys.stdin)["token"])')"
actual="$("$bin" apply-delete --db "$db" --prefix "trial:" --token "$token")"
printf '%s' "$actual" | python3 -c 'import sys,json; assert json.load(sys.stdin)["deleted"] == 2'
"$bin" list --db "$db" --prefix "trial:" | python3 -c 'import sys,json; assert json.load(sys.stdin) == []'
"$bin" audit --db "$db" | python3 -c 'import sys,json; assert len([x for x in json.load(sys.stdin) if x["action"] == "DELETE"]) == 2'

echo "PASS: guarded delete and stale-preview smoke test"
