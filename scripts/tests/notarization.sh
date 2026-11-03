#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
mkdir "$stage/bin"
export NOTARY_TEST_STATE="$stage"
export APPLE_NOTARY_PROFILE=test-profile
export APPLE_NOTARY_KEYCHAIN=/test/keychain
export PATH="$stage/bin:$PATH"
cat > "$stage/bin/xcrun" <<'FAKE'
#!/usr/bin/env bash
set -euo pipefail
[[ "$1" == notarytool ]]
printf '%s\n' "$2" >> "$NOTARY_TEST_STATE/calls"
case "$2" in
  submit)
    printf '{"id":"test-submission"}\n'
    if [[ -f "$NOTARY_TEST_STATE/ambiguous" ]]; then exit 1; fi
    ;;
  info)
    [[ "$3" == test-submission ]]
    if [[ -f "$NOTARY_TEST_STATE/rejected" ]]; then
      printf '{"status":"Invalid"}\n'
    elif [[ -f "$NOTARY_TEST_STATE/waited" ]]; then
      printf '{"status":"Accepted"}\n'
    else
      printf '{"status":"In Progress"}\n'
    fi
    ;;
  wait)
    [[ "$3" == test-submission ]]
    touch "$NOTARY_TEST_STATE/waited"
    exit 1 # Timeout does not cancel the server's submission.
    ;;
  log)
    [[ "$3" == test-submission ]]
    if [[ -f "$NOTARY_TEST_STATE/rejected" ]]; then
      printf '{"status":"Invalid","issues":[{"severity":"error"}]}\n' > "${@: -1}"
    elif [[ -f "$NOTARY_TEST_STATE/warning" ]]; then
      printf '{"status":"Accepted","issues":[{"severity":"warning"}]}\n' > "${@: -1}"
    else
      printf '{"status":"Accepted","issues":null}\n' > "${@: -1}"
    fi
    ;;
  *) exit 1 ;;
esac
FAKE
chmod +x "$stage/bin/xcrun"
printf 'signed archive bytes' > "$stage/app.zip"
notarize="$repo_root/scripts/notarize-macos-artifact.sh"

"$notarize" "$stage/app.zip" "$stage/accepted" > "$stage/output" 2>&1
[[ "$(grep -c '^submit$' "$stage/calls")" == 1 ]]
[[ "$(grep -c '^wait$' "$stage/calls")" == 1 ]]
[[ -s "$stage/accepted/log.json" ]]
"$notarize" "$stage/app.zip" "$stage/accepted" >> "$stage/output" 2>&1
[[ "$(grep -c '^submit$' "$stage/calls")" == 1 ]]
printf 'changed bytes' >> "$stage/app.zip"
if "$notarize" "$stage/app.zip" "$stage/accepted" >> "$stage/output" 2>&1; then
  echo "changed archive unexpectedly reused submission" >&2; exit 1
fi
[[ "$(grep -c '^submit$' "$stage/calls")" == 1 ]]

touch "$stage/ambiguous"
if "$notarize" "$stage/app.zip" "$stage/ambiguous-evidence" >> "$stage/output" 2>&1; then
  echo "failed submission unexpectedly succeeded" >&2; exit 1
fi
if "$notarize" "$stage/app.zip" "$stage/ambiguous-evidence" >> "$stage/output" 2>&1; then
  echo "ambiguous upload unexpectedly resubmitted" >&2; exit 1
fi
[[ "$(grep -c '^submit$' "$stage/calls")" == 2 ]]
rm "$stage/ambiguous"

touch "$stage/rejected"
if "$notarize" "$stage/app.zip" "$stage/rejected-evidence" >> "$stage/output" 2>&1; then
  echo "rejected submission unexpectedly succeeded" >&2; exit 1
fi
[[ -s "$stage/rejected-evidence/log.json" ]]
rm "$stage/rejected"
touch "$stage/warning"
if "$notarize" "$stage/app.zip" "$stage/warning-evidence" >> "$stage/output" 2>&1; then
  echo "notarization warning unexpectedly passed without review" >&2; exit 1
fi
if APPLE_SIGNING_IDENTITY=- APPLE_TEAM_ID=TESTTEAM01 \
  "$repo_root/scripts/build-notarized-dmg.sh" "$stage/binary" "$stage/output.dmg" "$stage/distribution" \
  >> "$stage/output" 2>&1; then
  echo "distribution unexpectedly accepted ad-hoc signing" >&2; exit 1
fi
[[ ! -e "$stage/distribution" ]]
echo "PASS: timeout, resume, changed archive, ambiguous submit, rejection, warning, and ad-hoc signing guards"
