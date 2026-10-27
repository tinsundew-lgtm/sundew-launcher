#!/bin/sh
# Removes LimeCraft (asks whether to delete your games and settings too).
exec "$(dirname "$0")/install.sh" --uninstall "$@"
