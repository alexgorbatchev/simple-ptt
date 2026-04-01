set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

run:
    SIMPLE_PTT_CONFIG="./config.toml" cargo run

run-config config_path="config.toml":
    SIMPLE_PTT_CONFIG="{{config_path}}" cargo run

run-local:
    SIMPLE_PTT_CONFIG="./config.toml" cargo run

run-xdg:
    cargo run
