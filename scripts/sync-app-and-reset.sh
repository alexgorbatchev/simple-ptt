#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
installed_app_path="/Applications/simple-ptt.app"

cd "$repo_root"

./scripts/clear-macos-permissions.sh
./scripts/sync-app.sh

xattr -dr com.apple.quarantine "$installed_app_path" || true

echo "app synced and permissions cleared"
