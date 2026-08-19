#!/bin/sh
# Builds and installs the pacman package from this working tree.
# pacman will ask for your password at the install step.
set -eu
cd "$(dirname "$0")"
makepkg --syncdeps --install --force "$@"
cat <<'MSG'

Installed. `view3d` is on your PATH, and it is registered as a handler for
.stl / .3mf / .obj files. To make it the default for a type:

    xdg-mime default view3d.desktop model/stl
    xdg-mime default view3d.desktop model/3mf
    xdg-mime default view3d.desktop model/obj

Remove it again with: sudo pacman -R view3d
MSG
