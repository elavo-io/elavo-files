#!/bin/bash
# Run Elavo Files in a separate, headless labwc, so the real session and its
# file manager are left alone.
#
#   scripts/nested-session.sh start [WxH]   headless labwc on a new socket (1366x768)
#   scripts/nested-session.sh run [ARGS]    start Files in it (a folder or URI)
#   scripts/nested-session.sh keys ARGS     wtype into it (e.g. keys -M ctrl -k plus -m ctrl)
#   scripts/nested-session.sh shot NAME     screenshot to $SHOTS/NAME.png
#   scripts/nested-session.sh stop          end it all
#
# Files runs inside the elavo-dev toolbox on its own D-Bus session bus, so
# it can't reach an instance in the real session, with settings in
# $T/config (dark mode, the Elavo theme and icons, Elavo's window button
# layout), not yours. Its home is $T/home/$USER and its data folder is
# $T/data, so Move to Trash goes to $T/data/Trash and never to your Trash. The toolbox has no GVfs, so
# Recents and Network don't open here and Trash is read from that home
# trash directly; all three use GVfs in a real Elavo session. Nothing
# here needs root. ELAVO_SHARED_APPLICATIONS, if set, is passed on (the
# toolbox sees /Applications as /run/host/Applications).

set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
T=${ELAVO_FILES_TEST:-/tmp/elavo-files-test}
SHOTS=${SHOTS:-$HOME/GitHub/.lens-preview}
BOX=${TOOLBOX:-elavo-dev}
mkdir -p "$T"

socket() {
    [ -s "$T/socket" ] || { echo "no nested session; run: $0 start" >&2; exit 1; }
    cat "$T/socket"
}

in_box() {
    toolbox run -c "$BOX" env -u GTK_THEME -u GSETTINGS_SCHEMA_DIR \
        WAYLAND_DISPLAY="$(socket)" GSK_RENDERER="${GSK_RENDERER:-cairo}" "$@"
}

case "${1:-}" in
start)
    mode=${2:-1366x768}
    mkdir -p "$T/labwc" "$T/config"
    : >"$T/labwc/autostart"
    cat >"$T/labwc/rc.xml" <<'XML'
<?xml version="1.0"?>
<labwc_config>
  <core><decoration>client</decoration></core>
  <placement><policy>center</policy></placement>
</labwc_config>
XML
    rm -f "$T/socket"
    env -u WAYLAND_DISPLAY -u DISPLAY WLR_BACKENDS=headless WLR_RENDERER=pixman \
        WLR_HEADLESS_OUTPUTS=1 XDG_CONFIG_HOME="$T/config" \
        setsid labwc -C "$T/labwc" -s "sh -c 'echo \$WAYLAND_DISPLAY >$T/socket'" \
        >"$T/labwc.log" 2>&1 &
    echo $! >"$T/labwc.pid"
    for _ in $(seq 50); do [ -s "$T/socket" ] && break; sleep 0.1; done
    WAYLAND_DISPLAY=$(socket) wlr-randr --output HEADLESS-1 --custom-mode "$mode" >/dev/null
    echo "nested labwc on $(socket) at $mode (pid $(cat "$T/labwc.pid"))"
    ;;
run)
    shift
    # Your folder names (Desktop, Documents...) and dark mode, in the
    # private settings directory.
    mkdir -p "$T/config/gtk-4.0"
    [ -f "$HOME/.config/user-dirs.dirs" ] && cp "$HOME/.config/user-dirs.dirs" "$T/config/"
    printf '[Settings]\ngtk-theme-name=%s\ngtk-icon-theme-name=%s\ngtk-interface-color-scheme=dark\ngtk-application-prefer-dark-theme=true\ngtk-decoration-layout=close,minimize,maximize:\n' \
        "${GTK_THEME_NAME:-Elavo}" "${ICON_THEME_NAME:-Elavo}" >"$T/config/gtk-4.0/settings.ini"
    # A private data folder (and so a private Trash), seeing your themes,
    # icons and apps read-only through links.
    mkdir -p "$T/data"
    for d in themes icons applications; do
        if [ -e "$HOME/.local/share/$d" ]; then ln -sfn "$HOME/.local/share/$d" "$T/data/$d"; fi
    done
    # A private home on the same tmpfs as /tmp, so Move to Trash works for
    # scratch folders under /tmp (GLib only trashes files that are on the
    # home's filesystem, or on a removable one).
    H="$T/home/$USER"
    for d in Desktop Documents Downloads Music Pictures Videos; do mkdir -p "$H/$d"; done
    # A private bus and private settings. Portals are off so nothing on the
    # private bus starts the real session's portal services.
    in_box env -u DBUS_SESSION_BUS_ADDRESS GDK_DEBUG=no-portals GTK_USE_PORTAL=0 \
        GTK_A11Y=none ELAVO_FILES_LOG=1 ${ELAVO_SHARED_APPLICATIONS:+ELAVO_SHARED_APPLICATIONS="$ELAVO_SHARED_APPLICATIONS"} HOME="$H" XDG_CONFIG_HOME="$T/config" XDG_DATA_HOME="$T/data" \
        dbus-run-session -- sh -c '
            gsettings set org.gnome.desktop.interface color-scheme prefer-dark
            gsettings set org.gnome.desktop.interface icon-theme Elavo
            gsettings set org.gnome.desktop.wm.preferences button-layout "close,minimize,maximize:"
            exec "$0" "$@"' "$ROOT/target/debug/elavo-files" "$@" >"$T/files.log" 2>&1 &
    sleep 3
    ;;
keys)
    shift
    in_box wtype "$@"
    ;;
shot)
    mkdir -p "$SHOTS"
    out="$SHOTS/${2:-files}.png"
    WAYLAND_DISPLAY=$(socket) grim -o HEADLESS-1 "$out"
    echo "$out"
    ;;
stop)
    pkill -f "$ROOT/target/debug/elavo-files" || true
    [ -s "$T/labwc.pid" ] && kill "$(cat "$T/labwc.pid")" 2>/dev/null || true
    rm -f "$T/socket" "$T/labwc.pid"
    ;;
*)
    sed -n '2,21p' "$0"
    exit 1
    ;;
esac
