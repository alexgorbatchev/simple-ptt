#!/usr/bin/env bash
set -euo pipefail

if pkill -x simple-ptt; then
    :
else
    kill_status=$?
    if [ "$kill_status" -ne 1 ]; then
        echo "Failed to stop simple-ptt (pkill exit $kill_status)" >&2
        exit "$kill_status"
    fi
fi
tccutil reset Accessibility io.github.alexgorbatchev.simple-ptt
tccutil reset ListenEvent io.github.alexgorbatchev.simple-ptt
tccutil reset Microphone io.github.alexgorbatchev.simple-ptt
