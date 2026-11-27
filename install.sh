#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

BUILD_USER=""
TARGET_UID="$(id -u)"
TARGET_HOME="$HOME"
SUDO="sudo"

if [ "$(id -u)" -eq 0 ]; then
    SUDO=""
    if [ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != "root" ]; then
        BUILD_USER="$SUDO_USER"
        TARGET_UID="$(id -u "$SUDO_USER")"
        TARGET_HOME="$(getent passwd "$SUDO_USER" | cut -d: -f6 || echo "/home/$SUDO_USER")"
    fi
fi

CARGO_BIN="$(command -v cargo || true)"
if [ -z "$CARGO_BIN" ] && [ -x "$TARGET_HOME/.cargo/bin/cargo" ]; then
    CARGO_BIN="$TARGET_HOME/.cargo/bin/cargo"
fi
if [ -z "$CARGO_BIN" ]; then
    echo "error: cargo not found in PATH or $TARGET_HOME/.cargo/bin" >&2
    exit 1
fi

CARGO_CACHE_DIR="${CARGO_HOME:-$TARGET_HOME/.cargo}"
RUSTUP_CACHE_DIR="${RUSTUP_HOME:-$TARGET_HOME/.rustup}"

run_cargo() {
    if [ -n "$BUILD_USER" ]; then
        sudo -u "$BUILD_USER" env \
            HOME="$TARGET_HOME" \
            CARGO_HOME="$CARGO_CACHE_DIR" \
            RUSTUP_HOME="$RUSTUP_CACHE_DIR" \
            PATH="$TARGET_HOME/.cargo/bin:$PATH" \
            "$CARGO_BIN" "$@"
    else
        env CARGO_HOME="$CARGO_CACHE_DIR" "$CARGO_BIN" "$@"
    fi
}

echo "==> Building velox-wm and velox-settings (release)..."
if ! run_cargo build --release --offline; then
    echo "==> Offline crate cache miss; retrying release build online..."
    run_cargo build --release
fi

echo "==> Installing binaries, desktop entries, and LightDM Wayland session (requires sudo)..."
$SUDO install -Dm755 target/release/velox-wm /usr/local/bin/velox-wm
$SUDO install -Dm755 target/release/velox-settings /usr/local/bin/velox-settings
$SUDO install -Dm755 assets/velox-session /usr/local/bin/velox-session
$SUDO install -Dm644 assets/velox-wm.desktop /usr/share/wayland-sessions/velox-wm.desktop
$SUDO install -Dm644 assets/velox-settings.desktop /usr/share/applications/velox-settings.desktop

if pgrep -x velox-wm >/dev/null 2>&1; then
    RUNTIME_DIR=""
    if [ -z "$BUILD_USER" ] && [ -n "${XDG_RUNTIME_DIR:-}" ]; then
        RUNTIME_DIR="$XDG_RUNTIME_DIR"
    elif [ -d "/run/user/$TARGET_UID" ]; then
        RUNTIME_DIR="/run/user/$TARGET_UID"
    fi
    if [ -n "$RUNTIME_DIR" ]; then
        CMD_FILE="$RUNTIME_DIR/velox-wm-cmd"
    else
        CMD_FILE="/tmp/velox-wm-cmd-$TARGET_UID"
    fi

    write_cmd() {
        if [ -n "$BUILD_USER" ]; then
            sudo -u "$BUILD_USER" tee "$CMD_FILE" >/dev/null <<<"$1"
        else
            printf '%s\n' "$1" > "$CMD_FILE"
        fi
    }

    if [ "${1:-}" = "--reload-now" ]; then
        write_cmd "reload-now"
        echo "==> Immediate seamless reload signal sent to running Velox WM!"
    elif [ "${1:-}" = "--reload" ]; then
        write_cmd "confirm-reload"
        echo "==> Opened reload confirmation dialog in Velox WM (review snapshot details & press Confirm/Enter)!"
    else
        echo "==> Update installed! Velox WM detected the new build — click 'Reload WM Now' in Notifications or run '$0 --reload'."
    fi
else
    echo "==> Done! Log out and select 'Velox WM' in LightDM, or launch 'Velox Settings' anytime (Win+I)."
fi
