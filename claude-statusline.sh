#!/bin/sh
# herdr-pacer — claude-statusline.sh
# Builds before 0.7 wrapped the Claude Code statusLine with this script. It
# hands over to the binary, so a statusLine wrapped by one of them keeps
# working after an update until the first hook rewraps it around the binary.
exec "$(dirname "$0")/target/release/herdr-pacer" statusline
