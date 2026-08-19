"""Regenerates docs/screenshots/*.png using view3d's own --screenshot flag.

Usage:
    cargo build --release
    python3 docs/make-models.py /tmp/view3d-models
    python3 docs/make-screenshots.py /tmp/view3d-models docs/screenshots

The draw mode is set by editing the persisted settings file before each run,
since draw modes are a UI setting rather than a command-line option.
"""
import os, re, subprocess, sys

models, outdir = sys.argv[1], sys.argv[2]
cfg = os.path.expanduser("~/.local/share/view3d/app.ron")

SHOTS = [
    ("shaded.png",        "knot.stl",       "Shaded",       "false"),
    ("material.png",      "knot_color.3mf", "Material",     "true"),
    ("wireframe.png",     "knot.stl",       "Wireframe",    "false"),
    ("surface-angle.png", "knot.stl",       "SurfaceAngle", "false"),
    ("mesh-light.png",    "knot.stl",       "MeshLight",    "false"),
]

os.makedirs(outdir, exist_ok=True)
for name, model, mode, axes in SHOTS:
    settings = open(cfg).read()
    settings = re.sub(r"draw_mode:\w+", "draw_mode:" + mode, settings)
    settings = re.sub(r"draw_axes:\w+", "draw_axes:" + axes, settings)
    open(cfg, "w").write(settings)

    out_path = os.path.join(outdir, name)
    subprocess.run(["./target/release/view3d", os.path.join(models, model),
                    "--screenshot", out_path], check=True, timeout=90)
    subprocess.run(["magick", out_path, "-resize", "1100x", out_path], check=True)
    print("wrote", out_path)
