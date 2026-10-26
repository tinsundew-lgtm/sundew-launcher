#!/bin/sh
# Installs LimeCraft for the current user (no root needed):
#   ~/.local/bin/limecraft-launcher, the icon and an entry in the applications menu.
set -e
DIR="$(cd "$(dirname "$0")" && pwd)"
if [ ! -f "$DIR/limecraft-launcher" ]; then
    echo "limecraft-launcher is not next to install.sh" >&2
    exit 1
fi
BIN="$HOME/.local/bin"
APPS="$HOME/.local/share/applications"
ICONS="$HOME/.local/share/icons/hicolor/256x256/apps"
mkdir -p "$BIN" "$APPS" "$ICONS"
install -m 755 "$DIR/limecraft-launcher" "$BIN/limecraft-launcher"
install -m 644 "$DIR/limecraft.png" "$ICONS/limecraft.png"
# the menu entry points at the installed program
sed "s|^Exec=.*|Exec=$BIN/limecraft-launcher|" "$DIR/limecraft.desktop" > "$APPS/limecraft.desktop"
chmod 644 "$APPS/limecraft.desktop"
update-desktop-database "$APPS" >/dev/null 2>&1 || true
gtk-update-icon-cache -q "$HOME/.local/share/icons/hicolor" >/dev/null 2>&1 || true
echo "LimeCraft is installed. Find it in the menu (Games) or run: $BIN/limecraft-launcher"
case ":$PATH:" in
    *":$BIN:"*) ;;
    *) echo "Note: $BIN is not in your PATH yet; the menu entry works anyway." ;;
esac
