#!/usr/bin/env bash
set -euo pipefail

pkill -f '/Applications/simple-ptt.app/Contents/MacOS/simple-ptt' || true
tccutil reset Accessibility io.github.alexgorbatchev.simple-ptt
tccutil reset ListenEvent io.github.alexgorbatchev.simple-ptt
