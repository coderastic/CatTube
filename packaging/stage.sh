#!/usr/bin/env bash
# Collect what the AppImage and the Flatpak ship into dist/stage:
#   bin/cattube, bin/mpv and lib/ (mpv's libraries).
#
# Run it on Debian 13 (trixie), or in a Debian 13 chroot or container: its
# glibc (2.41) matches the Freedesktop 25.08 Flatpak runtime and is old
# enough for most current distributions, and its mpv (0.40) has everything
# CatTube's player controls use. Needs cargo, mpv and binutils.
#
# Libraries every system has and that must come from the host are left
# out: AppImage's excludelist (glibc, GL/Vulkan, X11/Wayland, ...) plus
# audio servers, hardware video decoding and D-Bus/systemd, which talk to
# drivers and daemons of the host's own version.
set -euo pipefail
cd "$(dirname "$0")/.."
EXCLUDE="${EXCLUDELIST:-packaging/excludelist}"
MPV="$(command -v mpv)" || { echo "mpv not installed" >&2; exit 1; }
cargo build --release --locked
S=dist/stage
rm -rf "$S" && mkdir -p "$S/bin" "$S/lib"
install -m755 "${CARGO_TARGET_DIR:-target}/release/cattube" "$S/bin/cattube"
install -m755 "$MPV" "$S/bin/mpv"
excluded() {
  grep -qxF "$1" <(grep -v '^#' "$EXCLUDE" | sed 's/[[:space:]]*#.*//; /^$/d') && return 0
  case "$1" in
    ld-linux*|libpulse*|libpipewire*|libjack*|libasound*|libva*|libvdpau*|libdrm*|libgbm*|libEGL*|libGL*|libvulkan*|libwayland-*|libdbus*|libsystemd*|libudev*|libcuda*|libnvidia*) return 0 ;;
  esac
  return 1
}
ldd "$MPV" | awk '/=> \//{print $1, $3}' | while read -r name path; do
  name="$(basename "$name")"
  excluded "$name" || cp -L "$path" "$S/lib/$name"
done
echo "staged cattube, mpv and $(ls "$S/lib" | wc -l) libraries; newest glibc symbol:"
objdump -T "$S"/bin/* "$S"/lib/* 2>/dev/null | grep -o 'GLIBC_[0-9.]*' | sort -Vu | tail -1
