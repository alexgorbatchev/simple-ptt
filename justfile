set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

run:
    if [ ! -f config.toml ]; then cp config.example.toml config.toml && echo "Created config.toml from config.example.toml"; fi
    SIMPLE_PTT_CONFIG="./config.toml" cargo run

run-config config_path="config.toml":
    SIMPLE_PTT_CONFIG="{{config_path}}" cargo run

run-local:
    SIMPLE_PTT_CONFIG="./config.toml" cargo run

run-simulated-error error_msg="deepgram connection failed":
    SIMPLE_PTT_SIMULATE_ERROR="{{error_msg}}" SIMPLE_PTT_CONFIG="./config.toml" cargo run

run-xdg:
    cargo run

# Overlay tuner: the overlay with live controls for every glass setting (dev tool).
debug-overlay:
    cargo run -- --debug

bundle-release:
    cargo build --locked --release
    ./scripts/build-macos-app.sh "target/release/simple-ptt" "dist/simple-ptt.app"

bundle-dmg:
    just bundle-release
    ./scripts/build-macos-dmg.sh "dist/simple-ptt.app" "dist/simple-ptt.dmg"

install-app app_dir="$HOME/Applications":
    just bundle-release
    mkdir -p "{{app_dir}}"
    rm -rf "{{app_dir}}/simple-ptt.app"
    ditto "dist/simple-ptt.app" "{{app_dir}}/simple-ptt.app"

start app_path="$HOME/Applications/simple-ptt.app":
    test -d "{{app_path}}"
    open -g "{{app_path}}"

test-update:
    ./scripts/test-sparkle-update.sh

list-devices app_path="$HOME/Applications/simple-ptt.app":
    test -x "{{app_path}}/Contents/MacOS/simple-ptt"
    "{{app_path}}/Contents/MacOS/simple-ptt" --list-devices
