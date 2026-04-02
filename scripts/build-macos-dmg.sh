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

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
config_example_path="${repo_root}/config.example.toml"
volume_name="${DMG_VOLUME_NAME:-$(basename "$dmg_path" .dmg)}"
app_name="$(basename "$app_bundle_path")"

if [[ ! -f "$config_example_path" ]]; then
  echo "config example not found: $config_example_path" >&2
  exit 66
fi

staging_dir="$(mktemp -d)"
cleanup() {
  rm -rf "$staging_dir"
}
trap cleanup EXIT

mkdir -p "$staging_dir"
ditto "$app_bundle_path" "${staging_dir}/${app_name}"
cp "$config_example_path" "${staging_dir}/config.example.toml"
ln -s /Applications "${staging_dir}/Applications"

cat > "${staging_dir}/README.txt" <<'EOF'
simple-ptt macOS installer

Install:
1. Drag simple-ptt.app into Applications.
2. Create ~/.config/simple-ptt/config.toml from config.example.toml.
3. Launch from Terminal with: open -g /Applications/simple-ptt.app

Notes:
- This app is unsigned. If macOS blocks it, allow it in Privacy & Security or run:
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
