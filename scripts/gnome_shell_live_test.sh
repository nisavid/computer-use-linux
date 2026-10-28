#!/usr/bin/env bash
set -euo pipefail

# Usage: scripts/gnome_shell_live_test.sh [repository-root]
# For Ubuntu 24.04 GNOME 46, in an isolated hosted runner or disposable container.
# Packages: gnome-shell dbus-x11 xvfb xauth xterm xdotool scrot tesseract-ocr
#           python3-gi gir1.2-gtk-3.0 libgl1-mesa-dri
# Run only in a disposable VM/container; use private XDG state, bus and display.
repo_root="${1:-$(pwd)}"
repo_root="$(realpath "$repo_root")"
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
task_root="$(mktemp -d)"
export GNOME_LIVE_ARTIFACTS="${GNOME_LIVE_ARTIFACTS:-$repo_root/gnome-live-artifacts}"
mkdir -p "$GNOME_LIVE_ARTIFACTS" "$task_root/runtime"
chmod 700 "$task_root/runtime"
trap 'rm -rf -- "$task_root"' EXIT
export XDG_RUNTIME_DIR="$task_root/runtime"
export XDG_CONFIG_HOME="$task_root/config"
export XDG_DATA_HOME="$task_root/data"
export XDG_CACHE_HOME="$task_root/cache"
export LC_ALL=C.UTF-8
export LIBGL_ALWAYS_SOFTWARE=1
export XDG_SESSION_TYPE=x11
export GNOME_SHELL_SESSION_MODE=user
export GSETTINGS_BACKEND=keyfile
uuid='computer-use-linux@avifenesh.dev'
mkdir -p "$XDG_DATA_HOME/gnome-shell/extensions/$uuid"
cp -a "$repo_root/gnome-shell-extension/$uuid/." \
    "$XDG_DATA_HOME/gnome-shell/extensions/$uuid/"
sha256sum "$repo_root/gnome-shell-extension/$uuid/extension.js" \
    "$XDG_DATA_HOME/gnome-shell/extensions/$uuid/extension.js" \
    > "$GNOME_LIVE_ARTIFACTS/extension-sha256.txt"
gnome-shell --version > "$GNOME_LIVE_ARTIFACTS/gnome-version.txt"

export GNOME_LIVE_SCRIPT_DIR="$script_dir"
xvfb-run -a -s '-screen 0 1280x900x24 -nolisten tcp' \
    dbus-run-session -- bash -euo pipefail <<'SESSION'
gsettings set org.gnome.shell enabled-extensions "['computer-use-linux@avifenesh.dev']"
gsettings set org.gnome.shell disable-user-extensions false
gsettings set org.gnome.desktop.session idle-delay 0
gsettings set org.gnome.desktop.screensaver lock-enabled false
gnome-shell --x11 --sm-disable --replace > "$GNOME_LIVE_ARTIFACTS/shell.log" 2>&1 &
shell_pid=$!
trap 'kill "$shell_pid" 2>/dev/null || true' EXIT
timeout 180 /usr/bin/python3 "$GNOME_LIVE_SCRIPT_DIR/gnome_shell_live_test.py" \
    2>&1 | tee "$GNOME_LIVE_ARTIFACTS/assertions.log"
SESSION
