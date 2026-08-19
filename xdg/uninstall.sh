#!/bin/sh
set -eu
prefix="${PREFIX:-$HOME/.local}"
rm -f "$prefix/bin/view3d" \
      "$prefix/share/applications/view3d.desktop" \
      "$prefix/share/mime/packages/view3d.xml" \
      "$prefix/share/icons/hicolor/scalable/apps/view3d.svg"
rm -f "$prefix"/share/icons/hicolor/*/apps/view3d.png
update-mime-database "$prefix/share/mime" 2>/dev/null || true
update-desktop-database "$prefix/share/applications" 2>/dev/null || true
echo "Removed view3d from $prefix."
