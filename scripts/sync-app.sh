#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dist_app_path="${repo_root}/dist/simple-ptt.app"
installed_app_path="/Applications/simple-ptt.app"

cd "$repo_root"

cargo build --locked --release
./scripts/build-macos-app.sh "target/release/simple-ptt" "$dist_app_path"

rm -rf "$installed_app_path"
ditto "$dist_app_path" "$installed_app_path"

echo "synced app to: $installed_app_path"
