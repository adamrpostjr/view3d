//! Headless loader check: `cargo run --example meshinfo -- file.3mf [...]`
//! Prints what the viewer would show for each file, without opening a window.

fn main() {
    let mut failures = 0;
    for arg in std::env::args().skip(1) {
        let path = std::path::PathBuf::from(&arg);
        let start = std::time::Instant::now();
        match view3d::loader::load(&path, true) {
            Ok((mesh, warning)) => {
                let b = &mesh.bounds;
                println!(
                    "{arg}\n  {} triangles, {} vertices in {:?}\n  X: [{:.3}, {:.3}]  Y: [{:.3}, {:.3}]  Z: [{:.3}, {:.3}]\n  colors: {}{}",
                    mesh.tri_count(),
                    mesh.verts.len(),
                    start.elapsed(),
                    b.min.x, b.max.x, b.min.y, b.max.y, b.min.z, b.max.z,
                    mesh.has_colors,
                    warning.map(|w| format!("\n  warning: {w}")).unwrap_or_default(),
                );
            }
            Err(e) => {
                failures += 1;
                eprintln!("{arg}\n  FAILED: {e:#}");
            }
        }
    }
    if failures > 0 {
        std::process::exit(1);
    }
}
