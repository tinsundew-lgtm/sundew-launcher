#!/bin/sh
# Builds LimeCraft from source on Linux Mint / Ubuntu / Debian.
#   ./packaging/linux/build-linux.sh   builds, result: ./dist/ in the project root
#   ./dist/install.sh                  installs it for your user
# dist/ is a build result: it is listed in .gitignore and never goes into git.
set -e
# Find the project root (the folder with Cargo.toml): the script may lie in
# packaging/linux/ (git checkout) or right in the root (source archive).
ROOT="$(cd "$(dirname "$0")" && pwd)"
while [ ! -f "$ROOT/Cargo.toml" ]; do
    if [ "$ROOT" = "/" ]; then
        echo "Cargo.toml not found: run this script from the LimeCraft sources" >&2
        exit 1
    fi
    ROOT="$(dirname "$ROOT")"
done
cd "$ROOT"

if ! command -v cargo >/dev/null 2>&1; then
    echo "Rust is not installed. Install it first (one command, from https://rustup.rs):" >&2
    echo "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh" >&2
    echo "then open a new terminal and run this script again." >&2
    exit 1
fi

echo "Installing the libraries needed to build (asks for your password) ..."
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libgtk-3-dev libxkbcommon-dev libwayland-dev libx11-dev libxcursor-dev libxi-dev libxrandr-dev libgl1-mesa-dev

echo "Building (the first time takes a few minutes) ..."
cargo build --release

rm -rf dist
mkdir -p dist
cp target/release/limecraft-launcher dist/
strip dist/limecraft-launcher || true
cp assets/icon-256.png dist/limecraft.png
cp packaging/linux/limecraft.desktop packaging/linux/install.sh packaging/linux/uninstall.sh dist/
chmod +x dist/limecraft-launcher dist/install.sh dist/uninstall.sh
echo
echo "Done: ./dist/limecraft-launcher"
echo "To put it into the applications menu: ./dist/install.sh"
