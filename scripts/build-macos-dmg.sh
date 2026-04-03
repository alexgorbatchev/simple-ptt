#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 <app-bundle-path> <dmg-path>" >&2
  exit 64
fi

app_bundle_path="$1"
dmg_path="$2"

if [[ ! -d "$app_bundle_path" ]]; then
  echo "app bundle not found: $app_bundle_path" >&2
  exit 66
fi

volume_name="${DMG_VOLUME_NAME:-$(basename "$dmg_path" .dmg)}"
app_name="$(basename "$app_bundle_path")"

staging_dir="$(mktemp -d)"
cleanup() {
  rm -rf "$staging_dir"
}
trap cleanup EXIT

mkdir -p "$staging_dir"
ditto "$app_bundle_path" "${staging_dir}/${app_name}"
ln -s /Applications "${staging_dir}/Applications"

cat > "${staging_dir}/README.txt" <<'EOF'
simple-ptt macOS installer

Install:
1. Drag simple-ptt.app into Applications.
2. Launch it with: open -g /Applications/simple-ptt.app
3. If Settings opens, enter your Deepgram API key and click Save and Apply.

Notes:
- This app is ad-hoc signed but not notarized. If macOS blocks it, allow it in Privacy & Security or run:
  xattr -dr com.apple.quarantine /Applications/simple-ptt.app
- Grant Microphone and Accessibility/Input Monitoring permissions when prompted.
EOF

mkdir -p "$(dirname "$dmg_path")"
rm -f "$dmg_path"

hdiutil create \
  -volname "$volume_name" \
  -srcfolder "$staging_dir" \
  -ov \
  -format UDZO \
  "$dmg_path" >/dev/null

echo "created dmg: $dmg_path"
