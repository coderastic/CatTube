#!/usr/bin/env bash
# Build dist/cattube-<version>-x86_64.tar.gz: the binary, the player files,
# the desktop entry and icon, and install.sh. Needs mpv and curl on the
# system it runs on. Uses the binary staged by stage.sh.
set -euo pipefail
cd "$(dirname "$0")/.."
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
NAME="cattube-$VERSION-x86_64"
[[ -x dist/stage/bin/cattube ]] || { echo "nothing staged: run packaging/stage.sh first" >&2; exit 1; }
rm -rf "dist/$NAME" && mkdir -p "dist/$NAME"
D="dist/$NAME"
install -Dm755 dist/stage/bin/cattube "$D/bin/cattube"
install -Dm644 assets/cattube-osd.lua "$D/share/cattube/cattube-osd.lua"
install -Dm644 assets/cattube-input.conf "$D/share/cattube/cattube-input.conf"
install -Dm644 packaging/io.github.coderastic.CatTube.desktop "$D/share/applications/io.github.coderastic.CatTube.desktop"
install -Dm644 packaging/io.github.coderastic.CatTube.svg "$D/share/icons/hicolor/scalable/apps/io.github.coderastic.CatTube.svg"
install -Dm644 packaging/io.github.coderastic.CatTube.metainfo.xml "$D/share/metainfo/io.github.coderastic.CatTube.metainfo.xml"
install -Dm755 packaging/install.sh "$D/install.sh"
install -Dm644 README.md "$D/README.md"
install -Dm644 LICENSE "$D/LICENSE"
install -Dm644 assets/fonts/LICENSE-Inter.txt "$D/LICENSE-Inter.txt"
tar -C dist -czf "dist/$NAME.tar.gz" "$NAME"
rm -rf "dist/$NAME"
(cd dist && sha256sum "$NAME.tar.gz" > "$NAME.tar.gz.sha256")
echo "dist/$NAME.tar.gz"
