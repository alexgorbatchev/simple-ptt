#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 <submission-zip-or-dmg> <evidence-directory>" >&2
  exit 64
fi
artifact="$1"
evidence="$2"
: "${APPLE_NOTARY_PROFILE:?set APPLE_NOTARY_PROFILE to an authenticated notarytool profile}"
auth=(--keychain-profile "$APPLE_NOTARY_PROFILE")
if [[ -n "${APPLE_NOTARY_KEYCHAIN:-}" ]]; then
  auth+=(--keychain "$APPLE_NOTARY_KEYCHAIN")
fi
command -v jq >/dev/null
digest="$(shasum -a 256 "$artifact" | awk '{print $1}')"
mkdir -p "$evidence"
if [[ -f "$evidence/sha256" ]]; then
  if [[ "$(cat "$evidence/sha256")" != "$digest" ]]; then
    echo "artifact changed; retain previous evidence and choose a new evidence directory" >&2
    exit 65
  fi
else
  printf '%s\n' "$digest" > "$evidence/sha256"
fi

if [[ ! -f "$evidence/submission.json" ]]; then
  # Keep partial output after a failed submit. Reconcile Apple's history before
  # retrying an ambiguous upload, rather than creating duplicate submissions.
  if [[ -f "$evidence/submission-pending.json" ]]; then
    echo "previous submit did not finish locally; reconcile notarytool history with $evidence before retrying" >&2
    exit 75
  fi
  xcrun notarytool submit "$artifact" "${auth[@]}" --output-format json \
    > "$evidence/submission-pending.json"
  jq -e '.id | strings | select(length > 0)' "$evidence/submission-pending.json" >/dev/null
  mv "$evidence/submission-pending.json" "$evidence/submission.json"
fi
submission_id="$(jq -er '.id | strings | select(length > 0)' "$evidence/submission.json")"
echo "notarization submission: $submission_id"

# Timeout leaves Apple's job running. Every poll uses the same persisted ID.
deadline=$((SECONDS + 1800))
while true; do
  xcrun notarytool info "$submission_id" "${auth[@]}" --output-format json > "$evidence/status.json"
  status="$(jq -er '.status' "$evidence/status.json")"
  case "$status" in
    Accepted) break ;;
    'In Progress') ;;
    *)
      xcrun notarytool log "$submission_id" "${auth[@]}" "$evidence/log.json"
      echo "notarization failed ($status); inspect $evidence/log.json" >&2
      exit 65
      ;;
  esac
  if (( SECONDS >= deadline )); then
    echo "notarization still pending; rerun this command with the same artifact and evidence directory" >&2
    exit 75
  fi
  # A wait timeout is expected; the next info call determines the actual state.
  if ! xcrun notarytool wait "$submission_id" "${auth[@]}" --timeout 60s --output-format json \
    > "$evidence/wait.json"; then
    echo "wait ended before acceptance; checking submission $submission_id"
  fi
done
xcrun notarytool log "$submission_id" "${auth[@]}" "$evidence/log.json"
jq -e '.status == "Accepted" and ((.issues // []) | length == 0)' "$evidence/log.json" >/dev/null || {
  echo "notarization log requires review: $evidence/log.json" >&2
  exit 65
}
echo "notarization accepted without issues: $submission_id"
