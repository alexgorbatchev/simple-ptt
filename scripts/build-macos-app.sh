#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 <built-binary-path> <app-bundle-path>" >&2
  exit 64
fi

binary_path="$1"
app_bundle_path="$2"

if [[ ! -f "$binary_path" ]]; then
  echo "built binary not found: $binary_path" >&2
  exit 66
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cargo_toml_path="${repo_root}/Cargo.toml"

read_cargo_package_field() {
  local field="$1"

  awk -F'"' -v field="$field" '
    $0 == "[package]" { in_package = 1; next }
    /^\[/ && in_package { exit }
    in_package && $1 ~ ("^" field " = ") { print $2; exit }
  ' "$cargo_toml_path"
}

package_name="$(read_cargo_package_field name)"
package_version="$(read_cargo_package_field version)"

if [[ -z "$package_name" || -z "$package_version" ]]; then
  echo "failed to read package metadata from $cargo_toml_path" >&2
  exit 65
fi

binary_name="$(basename "$binary_path")"
app_name="$(basename "$app_bundle_path" .app)"
bundle_identifier="${APP_BUNDLE_IDENTIFIER:-io.github.alexgorbatchev.${package_name}}"
microphone_usage_description="${APP_MICROPHONE_USAGE_DESCRIPTION:-simple-ptt needs microphone access to capture push-to-talk audio for real-time transcription.}"

app_contents_path="${app_bundle_path}/Contents"
app_macos_path="${app_contents_path}/MacOS"
app_resources_path="${app_contents_path}/Resources"
icon_file_name="AppIcon.icns"
icon_file_base_name="${icon_file_name%.icns}"
iconset_root_dir="$(mktemp -d)"
iconset_dir="${iconset_root_dir}/AppIcon.iconset"
cleanup() {
  rm -rf "$iconset_root_dir"
}
trap cleanup EXIT

rm -rf "$app_bundle_path"
mkdir -p "$app_macos_path" "$app_resources_path" "$iconset_dir"
cp "$binary_path" "${app_macos_path}/${binary_name}"
chmod 755 "${app_macos_path}/${binary_name}"
"${app_macos_path}/${binary_name}" --write-app-iconset "$iconset_dir"
iconutil -c icns -o "${app_resources_path}/${icon_file_name}" "$iconset_dir"

cat > "${app_contents_path}/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>
  <string>${app_name}</string>
  <key>CFBundleDisplayName</key>
  <string>${app_name}</string>
  <key>CFBundleIdentifier</key>
  <string>${bundle_identifier}</string>
  <key>CFBundleExecutable</key>
  <string>${binary_name}</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleIconFile</key>
  <string>${icon_file_base_name}</string>
  <key>CFBundleVersion</key>
  <string>${package_version}</string>
  <key>CFBundleShortVersionString</key>
  <string>${package_version}</string>
  <key>LSUIElement</key>
  <true/>
  <key>NSMicrophoneUsageDescription</key>
  <string>${microphone_usage_description}</string>
</dict>
</plist>
EOF

if [[ "${ADHOC_SIGN_APP:-0}" == "1" ]]; then
  codesign --force --deep --sign - "$app_bundle_path"
fi

echo "created app bundle: $app_bundle_path"
