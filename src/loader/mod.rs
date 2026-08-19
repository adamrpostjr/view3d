//! File-format dispatch and background loading.

pub mod obj;
pub mod stl;
pub mod threemf;

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

use crate::mesh::Mesh;

/// Extensions this viewer will open, used for dialogs and folder cycling.
pub const EXTENSIONS: [&str; 3] = ["stl", "3mf", "obj"];

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Format {
    Stl,
    ThreeMf,
    Obj,
}

pub fn detect(path: &Path) -> Option<Format> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "stl" => Some(Format::Stl),
        "3mf" => Some(Format::ThreeMf),
        "obj" => Some(Format::Obj),
        _ => None,
    }
}

pub struct Loaded {
    pub path: PathBuf,
    pub mesh: Mesh,
    pub warning: Option<String>,
    pub elapsed: std::time::Duration,
    /// True when this was a reload of the file already on screen.
    pub is_reload: bool,
}

pub fn load(path: &Path, obj_y_up: bool) -> Result<(Mesh, Option<String>)> {
    match detect(path) {
        Some(Format::Stl) => Ok((stl::load(path)?, None)),
        Some(Format::ThreeMf) => Ok((threemf::load(path)?, None)),
        Some(Format::Obj) => obj::load(path, obj_y_up),
        None => bail!(
            "{} has an unsupported extension (expected .stl, .3mf or .obj)",
            path.display()
        ),
    }
}

/// Loads on a worker thread so the UI keeps painting on huge files.
pub fn load_async(
    path: PathBuf,
    obj_y_up: bool,
    is_reload: bool,
) -> Receiver<Result<Loaded, (PathBuf, String)>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let start = std::time::Instant::now();
        let msg = match load(&path, obj_y_up) {
            Ok((mesh, warning)) => Ok(Loaded {
                path,
                mesh,
                warning,
                elapsed: start.elapsed(),
                is_reload,
            }),
            Err(e) => Err((path, format!("{e:#}"))),
        };
        let _ = tx.send(msg);
    });
    rx
}
