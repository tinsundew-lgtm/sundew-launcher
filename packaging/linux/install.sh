#!/usr/bin/env bash
# LimeCraft installer for Linux (Linux Mint / Ubuntu / Debian and others).
# Installs for the current user, no root needed:
#   ~/.local/lib/limecraft/          the program, this installer, VERSION
#   ~/.local/bin/limecraft-launcher  link to the program
#   menu entry, icon and (optionally) a desktop shortcut.
# Games and settings live in ~/.local/share/LimeCraft and are never touched
# unless you ask the uninstaller to remove them.
#
#   ./install.sh               interactive install
#   ./install.sh -y            install with the default choices, no questions
#   ./install.sh --uninstall   remove LimeCraft (the same as ./uninstall.sh)

# dash and other plain sh cannot run this script: restart it in bash
if [ -z "${BASH_VERSION:-}" ]; then
    exec bash "$0" "$@"
fi
set -euo pipefail

SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LIB="$HOME/.local/lib/limecraft"
BIN="$HOME/.local/bin"
APPS="$HOME/.local/share/applications"
ICONS="$HOME/.local/share/icons/hicolor/256x256/apps"
DATA="${LIMECRAFT_HOME:-${XDG_DATA_HOME:-$HOME/.local/share}/LimeCraft}"

MODE=install
ASSUME_YES=0
USE_COLOR=1
for arg in "$@"; do
    case "$arg" in
        -y|--yes) ASSUME_YES=1 ;;
        -u|--uninstall) MODE=uninstall ;;
        --no-color) USE_COLOR=0 ;;
        -h|--help)
            sed -n '2,13p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
            exit 0 ;;
        *) echo "Unknown option: $arg (see --help)" >&2; exit 2 ;;
    esac
done
# no terminal to ask in (a pipe, a script): take the defaults
INTERACTIVE=1
if [ ! -t 0 ] || [ ! -t 1 ] || [ "$ASSUME_YES" = 1 ]; then INTERACTIVE=0; fi
if [ ! -t 1 ] || [ -n "${NO_COLOR:-}" ] || [ "${TERM:-dumb}" = dumb ]; then USE_COLOR=0; fi

# ---------------------------------------------------------------- look & feel
if [ "$USE_COLOR" = 1 ]; then
    if [ "${COLORTERM:-}" = truecolor ] || [ "${COLORTERM:-}" = 24bit ]; then
        LIME=$'\e[38;2;132;204;60m'; LIME2=$'\e[38;2;155;219;90m'; DIMG=$'\e[38;2;147;160;143m'
    else
        LIME=$'\e[38;5;113m'; LIME2=$'\e[38;5;149m'; DIMG=$'\e[38;5;246m'
    fi
    B=$'\e[1m'; D=$'\e[2m'; R=$'\e[0m'; RED=$'\e[38;5;203m'; YEL=$'\e[38;5;222m'
else
    LIME=''; LIME2=''; DIMG=''; B=''; D=''; R=''; RED=''; YEL=''
fi
OK="${LIME}✔${R}"; FAIL="${RED}✘${R}"; WARN="${YEL}!${R}"; ARROW="${LIME}›${R}"

cursor_show() { [ "$USE_COLOR" = 1 ] && printf '\e[?25h'; return 0; }
cursor_hide() { [ "$USE_COLOR" = 1 ] && printf '\e[?25l'; return 0; }
trap 'echo; echo "${D}Cancelled.${R}"; exit 130' INT

banner() {
    local subtitle="$1" cols
    cols="$(tput cols 2>/dev/null || echo 80)"
    echo
    if [ "$cols" -ge 76 ]; then
        printf '%s
'             "  ${LIME}██╗     ██╗███╗   ███╗███████╗${LIME2} ██████╗██████╗  █████╗ ███████╗████████╗${R}"             "  ${LIME}██║     ██║████╗ ████║██╔════╝${LIME2}██╔════╝██╔══██╗██╔══██╗██╔════╝╚══██╔══╝${R}"             "  ${LIME}██║     ██║██╔████╔██║█████╗  ${LIME2}██║     ██████╔╝███████║█████╗     ██║${R}"             "  ${LIME}██║     ██║██║╚██╔╝██║██╔══╝  ${LIME2}██║     ██╔══██╗██╔══██║██╔══╝     ██║${R}"             "  ${LIME}███████╗██║██║ ╚═╝ ██║███████╗${LIME2}╚██████╗██║  ██║██║  ██║██║        ██║${R}"             "  ${DIMG}╚══════╝╚═╝╚═╝     ╚═╝╚══════╝ ╚═════╝╚═╝  ╚═╝╚═╝  ╚═╝╚═╝        ╚═╝${R}"
    else
        echo "  ${LIME}${B}▙▄ LIME${LIME2}CRAFT${R}"
    fi
    echo
    echo "  ${DIMG}Minecraft launcher · mods, modpacks, offline skins${R}"
    echo "  ${B}${subtitle}${R}"
}

section() { echo; echo "  ${LIME}${B}$1${R}"; echo "  ${DIMG}$(printf '─%.0s' $(seq 1 ${#1}))${R}"; }
info() { echo "  ${ARROW} $*"; }
okay() { echo "  ${OK} $*"; }
warn() { echo "  ${WARN} $*"; }
fail() { echo "  ${FAIL} ${RED}$*${R}" >&2; exit 1; }

# run "$@" with a spinner and a label; on failure show its output and stop
SPIN_LOG="$(mktemp)"
trap 'rm -f "$SPIN_LOG"; cursor_show; stty echo 2>/dev/null || true' EXIT
step() {
    local label="$1"; shift
    if [ "$USE_COLOR" = 0 ]; then
        if "$@" >"$SPIN_LOG" 2>&1; then echo "  [ok] $label"; return 0; fi
        echo "  [fail] $label"; cat "$SPIN_LOG" >&2; exit 1
    fi
    cursor_hide
    "$@" >"$SPIN_LOG" 2>&1 &
    local pid=$! i=0 frames='⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏'
    while kill -0 "$pid" 2>/dev/null; do
        printf '\r  %s %s' "${LIME}${frames:i++%${#frames}:1}${R}" "$label"
        sleep 0.08
    done
    if wait "$pid"; then
        printf '\r\e[2K  %s %s\n' "$OK" "$label"
        cursor_show
    else
        printf '\r\e[2K  %s %s\n' "$FAIL" "$label"
        cursor_show
        sed 's/^/      /' "$SPIN_LOG" >&2
        exit 1
    fi
}

# yes/no question; $2 is the default (y or n)
ask() {
    local q="$1" def="$2" hint ans
    if [ "$INTERACTIVE" = 0 ]; then [ "$def" = y ]; return; fi
    if [ "$def" = y ]; then hint="${B}Y${R}${D}/n${R}"; else hint="${D}y/${R}${B}N${R}"; fi
    while true; do
        printf '  %s %s [%s] ' "${LIME}?${R}" "$q" "$hint"
        read -r ans || ans=""
        case "${ans:-$def}" in
            [YyДд]*) return 0 ;;
            [NnНн]*) return 1 ;;
        esac
    done
}

# checkbox list: arrows (or j/k) move, Space toggles, Enter confirms.
# Usage: checklist LABELS_ARRAY STATES_ARRAY  (states are 1/0, updated in place)
checklist() {
    local -n _labels=$1 _states=$2
    local n=${#_labels[@]} cur=0 key rest i
    if [ "$INTERACTIVE" = 0 ]; then return; fi
    if [ "$USE_COLOR" = 0 ]; then
        # no cursor movement without escape codes: one question per option
        for ((i = 0; i < n; i++)); do
            if ask "${_labels[i]}?" "$([ "${_states[i]}" = 1 ] && echo y || echo n)"; then _states[i]=1; else _states[i]=0; fi
        done
        return
    fi
    echo "  ${DIMG}↑/↓ move · Space toggle · Enter continue${R}"
    cursor_hide
    stty -echo 2>/dev/null || true
    while true; do
        for ((i = 0; i < n; i++)); do
            local box="${DIMG}[ ]${R}" ptr="  "
            [ "${_states[i]}" = 1 ] && box="${LIME}[✔]${R}"
            [ "$i" = "$cur" ] && ptr="${LIME}❯${R} "
            printf '\e[2K  %s%s %s\n' "$ptr" "$box" "${_labels[i]}"
        done
        IFS= read -rsn1 key || key=""
        case "$key" in
            $'\e')
                read -rsn2 -t 0.05 rest || rest=""
                case "$rest" in
                    '[A') cur=$(((cur - 1 + n) % n)) ;;
                    '[B') cur=$(((cur + 1) % n)) ;;
                esac ;;
            k) cur=$(((cur - 1 + n) % n)) ;;
            j) cur=$(((cur + 1) % n)) ;;
            ' ') _states[cur]=$((1 - _states[cur])) ;;
            '') break ;;
        esac
        printf '\e[%dA' "$n"
    done
    stty echo 2>/dev/null || true
    cursor_show
}

installed_version() { cat "$LIB/VERSION" 2>/dev/null || { [ -e "$BIN/limecraft-launcher" ] && echo "older"; } || true; }

desktop_dir() {
    local d=""
    command -v xdg-user-dir >/dev/null 2>&1 && d="$(xdg-user-dir DESKTOP 2>/dev/null || true)"
    if [ -z "$d" ] || [ "$d" = "$HOME" ]; then d="$HOME/Desktop"; fi
    echo "$d"
}

refresh_menus() {
    update-desktop-database "$APPS" >/dev/null 2>&1 || true
    gtk-update-icon-cache -q -t "$HOME/.local/share/icons/hicolor" >/dev/null 2>&1 || true
}

# ------------------------------------------------------------------ uninstall
do_uninstall() {
    banner "Uninstall"
    local ver; ver="$(installed_version)"
    if [ -z "$ver" ] && [ ! -e "$APPS/limecraft.desktop" ]; then
        info "LimeCraft is not installed for $USER — nothing to remove."
        exit 0
    fi
    info "Installed version: ${B}${ver:-unknown}${R}"
    echo
    ask "Remove LimeCraft?" y || { echo "  ${D}Nothing changed.${R}"; exit 0; }
    local wipe=0
    if [ -d "$DATA" ]; then
        local size; size="$(du -sh "$DATA" 2>/dev/null | cut -f1 || true)"
        warn "Games, worlds and settings: ${B}$DATA${R} ${DIMG}(${size:-?})${R}"
        ask "Delete them too? This cannot be undone" n && wipe=1
    fi
    section "Removing"
    local desk; desk="$(desktop_dir)"
    step "Program" rm -rf "$LIB" "$BIN/limecraft-launcher"
    step "Menu entry and icon" rm -f "$APPS/limecraft.desktop" "$ICONS/limecraft.png" "$desk/limecraft.desktop"
    step "Refreshing the menu" refresh_menus
    if [ "$wipe" = 1 ]; then
        step "Games and settings" rm -rf "$DATA"
    fi
    echo
    okay "${B}LimeCraft is removed.${R}"
    if [ "$wipe" = 0 ] && [ -d "$DATA" ]; then
        echo "    ${DIMG}Your games are kept in $DATA${R}"
    fi
    echo
}

# -------------------------------------------------------------------- install
check_system() {
    section "Checking your system"
    [ -f "$SRC/limecraft-launcher" ] || fail "limecraft-launcher is not next to install.sh ($SRC)"
    [ "$SRC" != "$LIB" ] || fail "This is the installed copy. Run install.sh from the downloaded folder to reinstall."

    local arch; arch="$(uname -m)"
    if [ "$arch" = x86_64 ]; then okay "Processor: 64-bit ($arch)"; else fail "This build is for x86_64, your processor is $arch"; fi

    local glibc; glibc="$(getconf GNU_LIBC_VERSION 2>/dev/null | awk '{print $2}' || true)"
    if [ -n "$glibc" ]; then
        if [ "$(printf '%s\n2.35\n' "$glibc" | sort -V | head -1)" = 2.35 ]; then
            okay "glibc $glibc"
        else
            warn "glibc $glibc is older than 2.35: the program may not start (Linux Mint 21+ is needed)"
        fi
    fi

    local distro=""
    [ -r /etc/os-release ] && distro="$(. /etc/os-release && echo "${PRETTY_NAME:-}")"
    [ -n "$distro" ] && okay "System: $distro"

    # libraries the window and the file dialogs need (some are loaded at run time,
    # so ldd alone does not see them)
    local missing="" cache=""
    if command -v ldd >/dev/null 2>&1; then
        missing="$(ldd "$SRC/limecraft-launcher" 2>/dev/null | awk '/not found/ {print $1}' || true)"
    fi
    local ldconfig; ldconfig="$(command -v ldconfig || echo /sbin/ldconfig)"
    if [ -x "$ldconfig" ]; then
        cache="$("$ldconfig" -p 2>/dev/null || true)"
        for lib in libgtk-3.so.0 libxkbcommon.so.0 libGL.so.1; do
            case "$cache" in *"$lib "*) ;; *) missing="$missing"$'
'"$lib" ;; esac
        done
    fi
    missing="$(printf '%s
' "$missing" | sed '/^$/d' | sort -u | tr '
' ' ')"
    if [ -z "${missing// /}" ]; then
        okay "Libraries: all in place"
    else
        warn "Missing libraries: ${B}$missing${R}"
        if command -v apt-get >/dev/null 2>&1; then
            if ask "Install them now with apt (asks for your password)?" y; then
                sudo apt-get install -y libgtk-3-0 libxkbcommon0 libgl1 || warn "apt could not install them; LimeCraft may not open"
            fi
        else
            echo "    ${DIMG}Install GTK 3, libxkbcommon and OpenGL with your package manager.${R}"
        fi
    fi

    if [ "$(df -Pk "$HOME" | awk 'NR==2 {print $4}')" -lt 2097152 ] 2>/dev/null; then
        warn "Less than 2 GB free in $HOME: Minecraft versions take 0.5–1 GB each"
    fi
}

copy_program() {
    mkdir -p "$LIB" "$BIN"
    install -m 755 "$SRC/limecraft-launcher" "$LIB/limecraft-launcher"
    install -m 755 "$SRC/install.sh" "$LIB/install.sh"
    printf '#!/bin/sh\nexec "$(dirname "$0")/install.sh" --uninstall "$@"\n' > "$LIB/uninstall.sh"
    chmod 755 "$LIB/uninstall.sh"
    printf '%s\n' "$NEW_VERSION" > "$LIB/VERSION"
    # older versions put the program right into ~/.local/bin
    rm -f "$BIN/limecraft-launcher"
    ln -s "$LIB/limecraft-launcher" "$BIN/limecraft-launcher"
}

write_desktop_file() {
    sed -e "s|^Exec=.*|Exec=$LIB/limecraft-launcher|" -e "s|^Icon=.*|Icon=$ICONS/limecraft.png|" \
        "$SRC/limecraft.desktop" > "$1"
    chmod 755 "$1"
}

add_menu_entry() {
    mkdir -p "$APPS" "$ICONS"
    install -m 644 "$SRC/limecraft.png" "$ICONS/limecraft.png"
    write_desktop_file "$APPS/limecraft.desktop"
    refresh_menus
}

add_desktop_shortcut() {
    local desk; desk="$(desktop_dir)"
    mkdir -p "$desk"
    write_desktop_file "$desk/limecraft.desktop"
    # GNOME/Cinnamon/Nemo: mark it trusted so it opens without the "untrusted launcher" prompt
    gio set "$desk/limecraft.desktop" metadata::trusted true >/dev/null 2>&1 || true
}

do_install() {
    NEW_VERSION="$(cat "$SRC/VERSION" 2>/dev/null || echo "")"
    banner "Installer${NEW_VERSION:+ · version $NEW_VERSION}"

    check_system

    section "Where it goes"
    echo "  ${DIMG}Program  ${R} $LIB"
    echo "  ${DIMG}Command  ${R} $BIN/limecraft-launcher"
    echo "  ${DIMG}Your data${R} $DATA"
    local old; old="$(installed_version)"
    if [ -n "$old" ]; then
        echo
        if [ "$old" = "$NEW_VERSION" ]; then
            info "LimeCraft $old is already installed: it will be reinstalled"
        else
            info "Update: ${B}$old${R} → ${B}${LIME}${NEW_VERSION:-new}${R} ${DIMG}(games and settings stay)${R}"
        fi
    fi

    section "Options"
    local -a labels=("Add to the applications menu" "Shortcut on the desktop" "Start LimeCraft when done")
    local -a states=(1 0 1)
    [ -e "$(desktop_dir)/limecraft.desktop" ] && states[1]=1
    if [ -z "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ] || [ "$INTERACTIVE" = 0 ]; then states[2]=0; fi
    if [ "$INTERACTIVE" = 1 ]; then
        checklist labels states
    else
        for i in "${!labels[@]}"; do
            [ "${states[i]}" = 1 ] && echo "  ${LIME}[✔]${R} ${labels[i]}"
        done
    fi

    echo
    ask "Install LimeCraft now?" y || { echo "  ${D}Nothing changed.${R}"; exit 0; }

    section "Installing"
    step "Copying the program" copy_program
    if [ "${states[0]}" = 1 ]; then step "Adding to the menu (Games)" add_menu_entry; fi
    if [ "${states[1]}" = 1 ]; then step "Creating the desktop shortcut" add_desktop_shortcut; fi

    echo
    local msg="LimeCraft${NEW_VERSION:+ $NEW_VERSION} is installed. Have fun!" line
    line="$(printf '─%.0s' $(seq 1 $((${#msg} + 7))))"
    echo "  ${LIME}╭${line}╮${R}"
    echo "  ${LIME}│${R}  ${OK} ${B}${msg}${R}   ${LIME}│${R}"
    echo "  ${LIME}╰${line}╯${R}"
    echo
    [ "${states[0]}" = 1 ] && info "Menu → Games → ${B}LimeCraft${R}"
    case ":$PATH:" in
        *":$BIN:"*) info "Terminal: ${B}limecraft-launcher${R}" ;;
        *) info "Terminal: ${B}$BIN/limecraft-launcher${R} ${DIMG}($BIN is not in PATH yet — log out and back in)${R}" ;;
    esac
    info "Uninstall: ${B}$LIB/uninstall.sh${R}"
    echo

    if [ "${states[2]}" = 1 ]; then
        nohup "$LIB/limecraft-launcher" >/dev/null 2>&1 &
        disown 2>/dev/null || true
        okay "Starting LimeCraft…"
        echo
    fi
}

if [ "$MODE" = uninstall ]; then do_uninstall; else do_install; fi
