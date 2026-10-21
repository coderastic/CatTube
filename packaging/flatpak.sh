#!/usr/bin/env bash
# Build dist/CatTube-<version>-x86_64.flatpak from dist/stage (run stage.sh
# first), on top of the Freedesktop 25.08 runtime (no SDK needed: CatTube
# and mpv are built by stage.sh on Debian 13, whose glibc the runtime
# shares). Libraries the runtime already has are left out, except FFmpeg's:
# ours plays H.264, HEVC and the rest without extra Flatpak extensions.
#
# Needs flatpak and org.freedesktop.Platform//25.08 installed.
set -euo pipefail
cd "$(dirname "$0")/.."
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
ID=io.github.coderastic.CatTube
S=dist/stage
[[ -x "$S/bin/mpv" ]] || { echo "nothing staged: run packaging/stage.sh first" >&2; exit 1; }
RT="$(flatpak info --show-location org.freedesktop.Platform//25.08)/files"
B=dist/flatpak-build R=dist/flatpak-repo
rm -rf "$B" "$R"
flatpak build-init "$B" "$ID" org.freedesktop.Platform org.freedesktop.Platform 25.08 >/dev/null
F="$B/files"
mkdir -p "$F/bin" "$F/lib"
cp -a "$S/bin/." "$F/bin/"
kept=0
for l in "$S"/lib/*; do
  n="$(basename "$l")"
  case "$n" in
    libav*|libsw*|libpostproc*|libncurses*|libtinfo*) ;;
    *) [[ -e "$RT/lib/x86_64-linux-gnu/$n" || -e "$RT/lib/$n" ]] && continue ;;
  esac
  cp -a "$l" "$F/lib/"; kept=$((kept + 1))
done
install -Dm644 assets/cattube-osd.lua "$F/share/cattube/cattube-osd.lua"
install -Dm644 assets/cattube-input.conf "$F/share/cattube/cattube-input.conf"
install -Dm644 packaging/$ID.desktop "$F/share/applications/$ID.desktop"
install -Dm644 packaging/$ID.svg "$F/share/icons/hicolor/scalable/apps/$ID.svg"
install -Dm644 packaging/$ID.metainfo.xml "$F/share/metainfo/$ID.metainfo.xml"
flatpak build-finish "$B" --command=cattube \
  --share=network --share=ipc --socket=wayland --socket=fallback-x11 \
  --socket=pulseaudio --device=dri \
  --talk-name=org.freedesktop.ScreenSaver >/dev/null
# --disable-sandbox: the icon check runs without bwrap (build containers
# often cannot mount /proc).
flatpak build-export --disable-sandbox "$R" "$B" >/dev/null
OUT="dist/CatTube-$VERSION-x86_64.flatpak"
flatpak build-bundle --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo "$R" "$OUT" "$ID"
rm -rf "$B" "$R"
(cd dist && sha256sum "$(basename "$OUT")" > "$(basename "$OUT").sha256")
echo "$OUT ($kept libraries bundled)"
