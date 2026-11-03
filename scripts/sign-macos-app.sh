#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <app-bundle-path>" >&2
  exit 64
fi

app_bundle_path="$1"
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
identity="${APPLE_SIGNING_IDENTITY:--}"
sign_args=(--force --sign "$identity" --options runtime)
if [[ "$identity" == - ]]; then
  echo "signing development bundle ad hoc: $app_bundle_path"
  sign_args+=(--timestamp=none)
else
  : "${APPLE_TEAM_ID:?set APPLE_TEAM_ID for Developer ID signing}"
  [[ "$APPLE_TEAM_ID" =~ ^[A-Z0-9]{10}$ ]] || { echo "invalid APPLE_TEAM_ID" >&2; exit 64; }
  sign_args+=(--timestamp)
fi
if [[ -n "${APPLE_SIGNING_KEYCHAIN:-}" ]]; then
  sign_args+=(--keychain "$APPLE_SIGNING_KEYCHAIN")
fi

# Sparkle's downloader owns its sandbox entitlements. Sign code inside out.
sparkle="${app_bundle_path}/Contents/Frameworks/Sparkle.framework"
codesign "${sign_args[@]}" "${sparkle}/Versions/B/XPCServices/Installer.xpc"
codesign "${sign_args[@]}" --preserve-metadata=entitlements \
  "${sparkle}/Versions/B/XPCServices/Downloader.xpc"
codesign "${sign_args[@]}" "${sparkle}/Versions/B/Autoupdate"
codesign "${sign_args[@]}" "${sparkle}/Versions/B/Updater.app"
codesign "${sign_args[@]}" "$sparkle"
codesign "${sign_args[@]}" --entitlements "${script_dir}/macos-entitlements.plist" "$app_bundle_path"
codesign --verify --deep --strict --verbose=2 "$app_bundle_path"
if [[ "$identity" != - ]]; then
  codesign --verify --strict --test-requirement \
    "=anchor apple generic and certificate leaf[subject.OU] = \"${APPLE_TEAM_ID}\" and certificate leaf[field.1.2.840.113635.100.6.1.13] exists" \
    "$app_bundle_path"
fi
