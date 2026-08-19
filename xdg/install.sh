#!/bin/sh
# Installs view3d for the current user: binary, icons, desktop entry and the
# 3MF MIME type, then offers it as the handler for .stl / .3mf / .obj.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
prefix="${PREFIX:-$HOME/.local}"
bin="$here/../target/release/view3d"

if [ ! -x "$bin" ]; then
    echo "Build it first: cargo build --release" >&2
    exit 1
fi

install -Dm755 "$bin" "$prefix/bin/view3d"
install -Dm644 "$here/view3d.svg" "$prefix/share/icons/hicolor/scalable/apps/view3d.svg"
for png in "$here"/icons/view3d_*.png; do
    size=$(basename "$png" .png | sed 's/view3d_//')
    install -Dm644 "$png" "$prefix/share/icons/hicolor/$size/apps/view3d.png"
done
install -Dm644 "$here/view3d.desktop" "$prefix/share/applications/view3d.desktop"
install -Dm644 "$here/view3d-mime.xml" "$prefix/share/mime/packages/view3d.xml"

update-mime-database "$prefix/share/mime" 2>/dev/null || true
update-desktop-database "$prefix/share/applications" 2>/dev/null || true
gtk-update-icon-cache -f -t "$prefix/share/icons/hicolor" 2>/dev/null || true

for type in model/stl model/3mf model/obj application/vnd.ms-package.3dmanufacturing-3dmodel+xml; do
    xdg-mime default view3d.desktop "$type" 2>/dev/null || true
done

echo "Installed to $prefix. Ensure $prefix/bin is on your PATH."
