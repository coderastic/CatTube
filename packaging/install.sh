#!/bin/sh
# Install CatTube from this folder.
#   ./install.sh              into ~/.local (no root needed)
#   sudo ./install.sh /usr/local
#   ./install.sh --uninstall [PREFIX]
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
uninstall=0
if [ "${1:-}" = "--uninstall" ]; then uninstall=1; shift; fi
prefix="${1:-$HOME/.local}"
files="bin/cattube share/cattube/cattube-osd.lua share/cattube/cattube-input.conf
share/applications/io.github.coderastic.CatTube.desktop
share/icons/hicolor/scalable/apps/io.github.coderastic.CatTube.svg
share/metainfo/io.github.coderastic.CatTube.metainfo.xml"
if [ "$uninstall" = 1 ]; then
  for f in $files; do rm -f "$prefix/$f"; done
  rmdir "$prefix/share/cattube" 2>/dev/null || true
  echo "CatTube removed from $prefix (your accounts stay in ~/.config/cattube)"
  exit 0
fi
for f in $files; do
  mkdir -p "$(dirname "$prefix/$f")"
  cp "$here/$f" "$prefix/$f"
done
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$prefix/share/applications" || true
command -v mpv >/dev/null 2>&1 || echo "Note: CatTube plays videos with mpv. Install it with your package manager."
command -v curl >/dev/null 2>&1 || echo "Note: CatTube needs curl. Install it with your package manager."
command -v python3 >/dev/null 2>&1 || echo "Note: CatTube needs Python 3 (for yt-dlp). Install it with your package manager."
echo "CatTube installed in $prefix (run: cattube)"
