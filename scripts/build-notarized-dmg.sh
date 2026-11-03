#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]]; then
  echo "usage: $0 <built-binary-path> <dmg-path> <new-evidence-directory>" >&2
  exit 64
fi
: "${APPLE_SIGNING_IDENTITY:?set APPLE_SIGNING_IDENTITY to a Developer ID Application identity}"
: "${APPLE_TEAM_ID:?set APPLE_TEAM_ID}"
: "${APPLE_NOTARY_PROFILE:?set APPLE_NOTARY_PROFILE}"
if [[ "$APPLE_SIGNING_IDENTITY" == - ]]; then
  echo "notarized distribution requires Developer ID signing" >&2
  exit 64
fi
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
binary_path="$1"
dmg_path="$2"
evidence="$3"
[[ "$dmg_path" == *.dmg ]] || { echo "output must end in .dmg" >&2; exit 64; }
# Refuse to overwrite an artifact or evidence from a previous submission.
[[ ! -e "$dmg_path" ]] || { echo "output already exists: $dmg_path" >&2; exit 73; }
mkdir "$evidence"
evidence="$(cd "$evidence" && pwd)"
app_bundle_path="$evidence/simple-ptt.app"
mount_path="$evidence/mount"
mounted=false
cleanup() {
  if [[ "$mounted" == true ]]; then
    hdiutil detach "$mount_path" >&2
  fi
}
trap cleanup EXIT

"$script_dir/build-macos-app.sh" "$binary_path" "$app_bundle_path"
codesign --display --verbose=4 "$app_bundle_path" 2> "$evidence/app-signature.txt"
codesign --display --xml --entitlements - "$app_bundle_path" > "$evidence/app-entitlements.plist"
ditto -c -k --sequesterRsrc --keepParent "$app_bundle_path" "$evidence/submission.zip"
"$script_dir/notarize-macos-artifact.sh" "$evidence/submission.zip" "$evidence/app-notary"
xcrun stapler staple "$app_bundle_path"
xcrun stapler validate "$app_bundle_path"
codesign --verify --deep --strict --verbose=2 "$app_bundle_path"
spctl --assess --type execute --verbose=4 "$app_bundle_path"

"$script_dir/build-macos-dmg.sh" "$app_bundle_path" "$dmg_path"
sign_args=(--force --sign "$APPLE_SIGNING_IDENTITY" --timestamp)
if [[ -n "${APPLE_SIGNING_KEYCHAIN:-}" ]]; then
  sign_args+=(--keychain "$APPLE_SIGNING_KEYCHAIN")
fi
codesign "${sign_args[@]}" "$dmg_path"
codesign --verify --strict --verbose=2 "$dmg_path"
"$script_dir/notarize-macos-artifact.sh" "$dmg_path" "$evidence/dmg-notary"
xcrun stapler staple "$dmg_path"
xcrun stapler validate "$dmg_path"
spctl --assess --type open --context context:primary-signature --verbose=4 "$dmg_path"

mkdir "$mount_path"
hdiutil attach -readonly -nobrowse -mountpoint "$mount_path" "$dmg_path"
mounted=true
distributed_app="$mount_path/simple-ptt.app"
xcrun stapler validate "$distributed_app"
codesign --verify --deep --strict --verbose=2 "$distributed_app"
spctl --assess --type execute --verbose=4 "$distributed_app"
hdiutil detach "$mount_path"
mounted=false

# Checksums describe the final stapled bytes and are portable to downloads.
checksum_path="${dmg_path%.dmg}.sha256"
(cd "$(dirname "$dmg_path")" && shasum -a 256 "$(basename "$dmg_path")") > "$checksum_path"
echo "verified notarized DMG: $dmg_path"
echo "verification evidence: $evidence"
