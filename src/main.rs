#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use view3d::app;

/// winit implements file drag-and-drop on X11, Windows and macOS, but not on
/// Wayland, where the drop events simply never arrive. So on Linux we default
/// to the X11 backend (through XWayland in a Wayland session) to keep
/// drag-and-drop working, and let `VIEW3D_BACKEND=wayland` opt back into a
/// native Wayland window for anyone who prefers it.
#[cfg(all(unix, not(target_os = "macos")))]
fn select_linux_backend(options: &mut eframe::NativeOptions) {
    use winit::platform::wayland::EventLoopBuilderExtWayland as _;
    use winit::platform::x11::EventLoopBuilderExtX11 as _;

    let requested = std::env::var("VIEW3D_BACKEND").unwrap_or_default();
    let x11_available = std::env::var_os("DISPLAY").is_some();
    let use_x11 = if requested.eq_ignore_ascii_case("wayland") {
        false
    } else if requested.eq_ignore_ascii_case("x11") {
        true
    } else {
        x11_available
    };

    options.event_loop_builder = Some(Box::new(move |builder| {
        if use_x11 {
            builder.with_x11();
        } else {
            log::warn!("using the Wayland backend; winit does not deliver file drops there");
            builder.with_wayland();
        }
    }));
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
fn select_linux_backend(_options: &mut eframe::NativeOptions) {}

struct Args {
    model: Option<PathBuf>,
    /// Render the model, write this PNG and exit — for thumbnails and docs.
    screenshot_to: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        model: None,
        screenshot_to: None,
    };
    let mut it = std::env::args_os().skip(1);
    while let Some(arg) = it.next() {
        match arg.to_string_lossy().as_ref() {
            "--screenshot" => {
                args.screenshot_to = Some(PathBuf::from(
                    it.next().ok_or("--screenshot needs an output path")?,
                ));
            }
            "-h" | "--help" => {
                println!("usage: view3d [FILE] [--screenshot OUT.png]");
                std::process::exit(0);
            }
            other if other.starts_with('-') => return Err(format!("unknown option {other}")),
            _ if args.model.is_none() => args.model = Some(PathBuf::from(arg)),
            other => return Err(format!("unexpected argument {other}")),
        }
    }
    if args.screenshot_to.is_some() && args.model.is_none() {
        return Err("--screenshot needs a model file to render".to_owned());
    }
    Ok(args)
}

fn main() -> eframe::Result<()> {
    env_logger::init();

    let Args {
        model,
        screenshot_to,
    } = match parse_args() {
        Ok(args) => args,
        Err(msg) => {
            eprintln!("{msg}\n\nusage: view3d [FILE] [--screenshot OUT.png]");
            std::process::exit(2);
        }
    };

    // In screenshot mode ask for a fixed-size, non-resizable window: tiling
    // window managers float windows that carry fixed size hints, which keeps
    // the captured image the same shape every time.
    let viewport = egui::ViewportBuilder::default().with_title("view3d");
    let viewport = if screenshot_to.is_some() {
        viewport
            .with_inner_size([1200.0, 760.0])
            .with_resizable(false)
    } else {
        viewport
            .with_inner_size([1000.0, 700.0])
            .with_drag_and_drop(true)
    };

    let mut options = eframe::NativeOptions {
        // The scene callback draws into egui's own render pass, so that pass
        // needs a depth attachment.
        depth_buffer: 32,
        multisampling: 0,
        viewport,
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };

    select_linux_backend(&mut options);

    // `--screenshot` must fail loudly for scripts, so track it across the app.
    let screenshot_failed = Arc::new(AtomicBool::new(false));
    let failed = Arc::clone(&screenshot_failed);

    let result = eframe::run_native(
        "view3d",
        options,
        Box::new(|cc| {
            app::App::new(cc, model, screenshot_to, Arc::clone(&failed))
                .map(|a| Box::new(a) as Box<dyn eframe::App>)
                .ok_or_else(|| "wgpu backend unavailable".into())
        }),
    );

    if result.is_ok() && screenshot_failed.load(Ordering::Relaxed) {
        std::process::exit(1);
    }
    result
}
