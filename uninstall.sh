#!/usr/bin/env bash
set -euo pipefail

PURGE_DATA=0
for arg in "$@"; do
    case "$arg" in
        --purge-data)
            PURGE_DATA=1
            ;;
        -h|--help)
            cat <<'EOF'
Usage: ./uninstall.sh [--purge-data]

Removes Velox WM binaries, LightDM Wayland session file, and desktop entries
installed by install.sh. User configuration files are preserved by default.

Options:
  --purge-data   Also remove ~/.config/velox-wm, ~/.local/state/velox-wm,
                 ~/.local/share/velox-wm, and ~/.cache/velox-wm
EOF
            exit 0
            ;;
        *)
            echo "Unknown option: $arg" >&2
            exit 1
            ;;
    esac
done

SUDO="sudo"
TARGET_HOME="$HOME"
if [ "$(id -u)" -eq 0 ]; then
    SUDO=""
    if [ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != "root" ]; then
        TARGET_HOME="$(getent passwd "$SUDO_USER" | cut -d: -f6 || echo "/home/$SUDO_USER")"
    fi
fi

echo "==> Removing Velox WM installed binaries and desktop entries..."
$SUDO rm -f \
    /usr/local/bin/velox-wm \
    /usr/local/bin/velox-settings \
    /usr/local/bin/velox-session \
    /usr/share/wayland-sessions/velox-wm.desktop \
    /usr/share/applications/velox-settings.desktop

if [ "$PURGE_DATA" -eq 1 ]; then
    echo "==> Purging user configuration, cache, logs, and history under $TARGET_HOME..."
    rm -rf \
        "$TARGET_HOME/.config/velox-wm" \
        "$TARGET_HOME/.local/state/velox-wm" \
        "$TARGET_HOME/.local/share/velox-wm" \
        "$TARGET_HOME/.cache/velox-wm"
else
    echo "==> Kept user config/data (pass --purge-data to remove ~/.config/velox-wm and state dirs)."
fi

echo "==> Velox WM uninstalled cleanly."
