#!/usr/bin/env bash
set -euo pipefail

pkill -9 simple-ptt 2>/dev/null || true
sleep 0.5

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

echo "building release binary..."
cargo build --locked --release

app_bundle_path="dist/simple-ptt.app"
binary_path="target/release/simple-ptt"

echo "building app bundle..."
./scripts/build-macos-app.sh "$binary_path" "$app_bundle_path"

feed_url="https://raw.githubusercontent.com/alexgorbatchev/simple-ptt/main/appcast.xml"
feed_xml="$(curl -sL "$feed_url" || true)"

if [[ -z "$feed_xml" ]]; then
  feed_xml="$(cat appcast.xml 2>/dev/null || true)"
fi

feed_version="$(echo "$feed_xml" | grep -oE '<sparkle:version>[^<]+' | head -n 1 | cut -d'>' -f2 || true)"

if [[ -z "$feed_version" ]]; then
  feed_version="$(awk -F'"' '$0 == "[package]" { in_pkg=1; next } /^\[/ && in_pkg { exit } in_pkg && $1 ~ /^version = / { print $2; exit }' Cargo.toml)"
fi

echo "detected latest published update version: $feed_version"

IFS='.' read -r major minor patch <<< "$feed_version"
major="${major:-1}"
minor="${minor:-0}"
patch="${patch:-0}"

if [[ "$patch" -gt 0 ]]; then
  test_patch=$((patch - 1))
  test_version="${major}.${minor}.${test_patch}"
elif [[ "$minor" -gt 0 ]]; then
  test_minor=$((minor - 1))
  test_version="${major}.${test_minor}.99"
elif [[ "$major" -gt 0 ]]; then
  test_major=$((major - 1))
  test_version="${test_major}.99.99"
else
  test_version="0.0.1"
fi

echo "setting test bundle version to $test_version (lower than $feed_version)..."

plutil -replace CFBundleVersion -string "$test_version" "${app_bundle_path}/Contents/Info.plist"
plutil -replace CFBundleShortVersionString -string "$test_version" "${app_bundle_path}/Contents/Info.plist"

codesign --force --deep --options runtime --sign - "$app_bundle_path"

echo "launching test app bundle ($app_bundle_path)..."
open -n "$app_bundle_path"

echo ""
echo "============================================================"
echo " Test App Bundle Launched!"
echo " Running Version : $test_version"
echo " Feed Version    : $feed_version"
echo " Instructions    : Click 'Check for Updates…' in the status bar."
echo "============================================================"
