//! Persisted user settings, mirroring fstl's `QSettings` keys.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::camera::{P_ORTHOGRAPHIC, P_PERSPECTIVE};

pub const MAX_RECENT: usize = 10;

#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Projection {
    Perspective,
    Orthographic,
}

impl Projection {
    pub fn value(self) -> f32 {
        match self {
            Self::Perspective => P_PERSPECTIVE,
            Self::Orthographic => P_ORTHOGRAPHIC,
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum DrawMode {
    Shaded,
    Wireframe,
    SurfaceAngle,
    MeshLight,
    /// Colors carried by the file itself (3MF materials, OBJ/MTL diffuse).
    Material,
}

impl DrawMode {
    /// Modes drawn as filled triangles, in pipeline order.
    pub const FILLED: [Self; 4] = [
        Self::Shaded,
        Self::SurfaceAngle,
        Self::MeshLight,
        Self::Material,
    ];

    pub const ALL: [Self; 5] = [
        Self::Shaded,
        Self::Wireframe,
        Self::SurfaceAngle,
        Self::MeshLight,
        Self::Material,
    ];

    pub fn fill_index(self) -> Option<usize> {
        Self::FILLED.iter().position(|m| *m == self)
    }

    pub fn entry_point(self) -> &'static str {
        match self {
            Self::Shaded => "fs_shaded",
            Self::Wireframe => "fs_wireframe",
            Self::SurfaceAngle => "fs_surface_angle",
            Self::MeshLight => "fs_mesh_light",
            Self::Material => "fs_material",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Shaded => "Shaded",
            Self::Wireframe => "Wireframe",
            Self::SurfaceAngle => "Surface Angle",
            Self::MeshLight => "Mesh Light",
            Self::Material => "Material Color",
        }
    }
}

/// The 26 light directions fstl offers: every combination of -1/0/1 except zero.
pub fn light_directions() -> Vec<([f32; 3], String)> {
    let xname = ["right ", " ", "left "];
    let yname = ["top ", " ", "bottom "];
    let zname = ["rear ", " ", "front "];
    let mut out = Vec::with_capacity(26);
    for i in -1..2i32 {
        for j in -1..2i32 {
            for k in -1..2i32 {
                if i == 0 && j == 0 && k == 0 {
                    continue;
                }
                let name = format!(
                    "{}{}{}",
                    xname[(i + 1) as usize],
                    yname[(j + 1) as usize],
                    zname[(k + 1) as usize]
                );
                let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
                out.push(([i as f32, j as f32, k as f32], name));
            }
        }
    }
    out
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub projection: Projection,
    pub draw_mode: DrawMode,
    pub draw_axes: bool,
    pub invert_zoom: bool,
    pub autoreload: bool,
    pub reset_transform_on_load: bool,
    pub hide_menu_bar: bool,
    pub obj_y_up: bool,
    pub ambient_color: [f32; 3],
    pub ambient_factor: f32,
    pub directive_color: [f32; 3],
    pub directive_factor: f32,
    pub light_direction: usize,
    pub recent_files: Vec<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            projection: Projection::Perspective,
            draw_mode: DrawMode::Shaded,
            draw_axes: false,
            invert_zoom: false,
            autoreload: true,
            reset_transform_on_load: true,
            hide_menu_bar: false,
            obj_y_up: true,
            ambient_color: [0.22, 0.8, 1.0],
            ambient_factor: 0.67,
            directive_color: [1.0, 1.0, 1.0],
            directive_factor: 0.5,
            light_direction: 1,
            recent_files: Vec::new(),
        }
    }
}

impl Settings {
    pub fn push_recent(&mut self, path: &std::path::Path) {
        self.recent_files.retain(|p| p != path);
        self.recent_files.insert(0, path.to_path_buf());
        self.recent_files.truncate(MAX_RECENT);
    }
}
