set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

run:
    SIMPLE_PTT_CONFIG="./config.yaml" cargo run

run-config config_path="config.yaml":
    SIMPLE_PTT_CONFIG="{{config_path}}" cargo run

run-local:
    SIMPLE_PTT_CONFIG="./config.yaml" cargo run

run-xdg:
    cargo run
