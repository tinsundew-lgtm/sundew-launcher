#!/bin/sh
# Removes what install.sh put in place. Your games and settings (~/.local/share/LimeCraft) stay.
rm -f "$HOME/.local/bin/limecraft-launcher" \
      "$HOME/.local/share/applications/limecraft.desktop" \
      "$HOME/.local/share/icons/hicolor/256x256/apps/limecraft.png"
update-desktop-database "$HOME/.local/share/applications" >/dev/null 2>&1 || true
echo "LimeCraft is removed. Delete ~/.local/share/LimeCraft too if you want to remove the games and settings."
