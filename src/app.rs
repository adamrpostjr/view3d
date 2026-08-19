//! Application state, menus, input handling and file plumbing.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui_wgpu;
use egui::{Key, KeyboardShortcut, Modifiers, PointerButton};
use glam::{Mat4, Vec3};

use crate::camera::{Camera, ViewPoint};
use crate::loader::{self, Loaded, EXTENSIONS};
use crate::mesh::Mesh;
use crate::render::{Scene, SceneCallback, SceneParams};
use crate::settings::{light_directions, DrawMode, Projection, Settings};
use crate::watch::FileWatcher;

const SETTINGS_KEY: &str = "view3d.settings";
const STATUS_TIMEOUT: Duration = Duration::from_secs(4);
/// Quiet period after a filesystem event before reloading, so we do not race a
/// slicer or CAD tool that is still writing the file.
const RELOAD_DEBOUNCE: Duration = Duration::from_millis(200);
/// `--screenshot` retries this often, and gives up after the timeout.
const AUTO_SCREENSHOT_RETRY: Duration = Duration::from_millis(300);
const AUTO_SCREENSHOT_SETTLE: Duration = Duration::from_millis(700);
const AUTO_SCREENSHOT_TIMEOUT: Duration = Duration::from_secs(20);

pub struct App {
    settings: Settings,
    camera: Camera,
    render_state: egui_wgpu::RenderState,

    mesh: Option<Arc<Mesh>>,
    mesh_info: String,
    path: Option<PathBuf>,

    pending: Option<Receiver<Result<Loaded, (PathBuf, String)>>>,
    watcher: Option<FileWatcher>,
    reload_at: Option<Instant>,

    status: String,
    status_at: Instant,

    last_pointer: Option<egui::Pos2>,
    show_light_prefs: bool,
    show_about: bool,
    fullscreen: bool,
    screenshot_pending: bool,
    /// Set when `--screenshot` could not produce a file, so main can exit(1).
    screenshot_failed: Arc<AtomicBool>,
    /// `--screenshot`: save one frame here, then quit.
    screenshot_to: Option<PathBuf>,
    /// When to re-ask for the automatic screenshot, and when to give up.
    screenshot_retry_at: Option<Instant>,
    screenshot_deadline: Option<Instant>,
    /// Window size and when it last changed, so `--screenshot` waits for the
    /// window manager to finish placing the window before capturing.
    last_size: egui::Vec2,
    size_changed_at: Instant,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        initial: Option<PathBuf>,
        screenshot_to: Option<PathBuf>,
        screenshot_failed: Arc<AtomicBool>,
    ) -> Option<Self> {
        let render_state = cc.wgpu_render_state.as_ref()?.clone();
        let scene = Scene::new(&render_state.device, render_state.target_format);
        render_state
            .renderer
            .write()
            .callback_resources
            .insert(scene);

        let settings: Settings = cc
            .storage
            .and_then(|s| eframe::get_value(s, SETTINGS_KEY))
            .unwrap_or_default();

        let mut camera = Camera::default();
        camera.perspective = settings.projection.value();

        let mut app = Self {
            camera,
            settings,
            render_state,
            mesh: None,
            mesh_info: String::new(),
            path: None,
            pending: None,
            watcher: None,
            reload_at: None,
            status: String::new(),
            status_at: Instant::now(),
            last_pointer: None,
            show_light_prefs: false,
            show_about: false,
            fullscreen: false,
            screenshot_pending: false,
            screenshot_failed,
            screenshot_to,
            screenshot_retry_at: None,
            screenshot_deadline: None,
            last_size: egui::Vec2::ZERO,
            size_changed_at: Instant::now(),
        };
        if let Some(path) = initial {
            app.open(path, false);
        }
        Some(app)
    }

    fn set_status(&mut self, text: impl Into<String>) {
        self.status = text.into();
        self.status_at = Instant::now();
    }

    fn open(&mut self, path: PathBuf, is_reload: bool) {
        if !is_reload {
            self.set_status(format!("Loading {}…", file_name(&path)));
        }
        self.pending = Some(loader::load_async(path, self.settings.obj_y_up, is_reload));
    }

    fn reload(&mut self) {
        if let Some(path) = self.path.clone() {
            self.open(path, true);
        }
    }

    fn open_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("3D models", &EXTENSIONS)
            .add_filter("All files", &["*"])
            .pick_file()
        {
            self.open(path, false);
        }
    }

    /// Opens the current file in the system's default application.
    fn open_with(&mut self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        #[cfg(target_os = "macos")]
        let cmd = "open";
        #[cfg(target_os = "windows")]
        let cmd = "explorer";
        #[cfg(all(unix, not(target_os = "macos")))]
        let cmd = "xdg-open";

        match std::process::Command::new(cmd).arg(&path).spawn() {
            Ok(_) => self.set_status(format!("Opened {} externally", file_name(&path))),
            Err(e) => self.set_status(format!("Could not launch {cmd}: {e}")),
        }
    }

    /// Steps to the previous/next loadable file in the current directory.
    fn cycle_file(&mut self, forward: bool) {
        let Some(current) = self.path.clone() else {
            return;
        };
        let Some(dir) = current.parent() else {
            return;
        };
        let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
            Ok(entries) => entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file() && loader::detect(p).is_some())
                .collect(),
            Err(e) => {
                self.set_status(format!("Cannot list {}: {e}", dir.display()));
                return;
            }
        };
        if files.len() < 2 {
            return;
        }
        files.sort();
        let idx = files.iter().position(|p| *p == current).unwrap_or(0);
        let next = if forward {
            (idx + 1) % files.len()
        } else {
            (idx + files.len() - 1) % files.len()
        };
        self.open(files[next].clone(), false);
    }

    fn finish_load(&mut self, loaded: Loaded) {
        let mesh = Arc::new(loaded.mesh);
        {
            let mut renderer = self.render_state.renderer.write();
            if let Some(scene) = renderer.callback_resources.get_mut::<Scene>() {
                scene.upload_mesh(&self.render_state.device, &self.render_state.queue, &mesh);
                if self.settings.draw_mode == DrawMode::Wireframe {
                    scene.ensure_edges(&self.render_state.device, &mesh);
                }
            }
        }

        self.camera.fit(
            mesh.bounds.min,
            mesh.bounds.max,
            loaded.is_reload,
            self.settings.reset_transform_on_load,
        );

        let b = &mesh.bounds;
        self.mesh_info = format!(
            "Triangles: {}\nX: [{:.3}, {:.3}]\nY: [{:.3}, {:.3}]\nZ: [{:.3}, {:.3}]",
            mesh.tri_count(),
            b.min.x,
            b.max.x,
            b.min.y,
            b.max.y,
            b.min.z,
            b.max.z
        );

        let name = file_name(&loaded.path);
        let mut status = format!(
            "Loaded {name} — {} triangles in {} ms",
            mesh.tri_count(),
            loaded.elapsed.as_millis()
        );
        if let Some(w) = &loaded.warning {
            status.push_str(&format!(" ({w})"));
        }
        self.set_status(status);

        self.settings.push_recent(&loaded.path);
        if self.watcher.as_ref().map(|w| w.path()) != Some(loaded.path.as_path()) {
            self.watcher = FileWatcher::new(&loaded.path);
        }
        self.path = Some(loaded.path);
        self.mesh = Some(mesh);
    }

    /// `--screenshot`: once the mesh is on screen, ask for a frame and quit
    /// when it arrives. A capture only happens if the request is in hand when
    /// the window actually paints, so re-ask until the image comes back rather
    /// than waiting forever on a request that was dropped.
    fn drive_auto_screenshot(&mut self, ctx: &egui::Context) {
        if self.screenshot_to.is_none() || self.mesh.is_none() {
            return;
        }
        let now = Instant::now();
        let deadline = *self
            .screenshot_deadline
            .get_or_insert(now + AUTO_SCREENSHOT_TIMEOUT);

        // Capturing mid-resize grabs a surface that does not match the visible
        // window, so wait until the size has held still for a moment. Tiling
        // window managers in particular resize us well after the first frame.
        let size = ctx.viewport_rect().size();
        if size != self.last_size {
            self.last_size = size;
            self.size_changed_at = now;
        }
        if now.duration_since(self.size_changed_at) < AUTO_SCREENSHOT_SETTLE {
            ctx.request_repaint_after(AUTO_SCREENSHOT_SETTLE);
            return;
        }
        if now >= deadline {
            log::error!("timed out waiting for the window to produce a screenshot");
            self.screenshot_failed.store(true, Ordering::Relaxed);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if self.screenshot_retry_at.is_none_or(|at| now >= at) {
            self.screenshot_pending = true;
            self.screenshot_retry_at = Some(now + AUTO_SCREENSHOT_RETRY);
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
        ctx.request_repaint();
    }

    fn poll_load(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.pending else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(loaded)) => {
                self.pending = None;
                self.finish_load(loaded);
            }
            Ok(Err((path, err))) => {
                self.pending = None;
                self.set_status(format!("Failed to load {}: {err}", file_name(&path)));
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                // Keep animating the "Loading…" status while we wait.
                ctx.request_repaint_after(Duration::from_millis(30));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.pending = None;
                self.set_status("Loader thread stopped unexpectedly");
            }
        }
    }

    fn poll_watcher(&mut self, ctx: &egui::Context) {
        if !self.settings.autoreload {
            return;
        }
        if let Some(w) = &self.watcher {
            if w.take_change() {
                self.reload_at = Some(Instant::now() + RELOAD_DEBOUNCE);
            }
        }
        if let Some(at) = self.reload_at {
            if Instant::now() >= at {
                self.reload_at = None;
                self.reload();
            } else {
                ctx.request_repaint_after(RELOAD_DEBOUNCE);
            }
        }
    }

    fn set_draw_mode(&mut self, mode: DrawMode) {
        self.settings.draw_mode = mode;
        if mode == DrawMode::Wireframe {
            if let Some(mesh) = self.mesh.clone() {
                let mut renderer = self.render_state.renderer.write();
                if let Some(scene) = renderer.callback_resources.get_mut::<Scene>() {
                    scene.ensure_edges(&self.render_state.device, &mesh);
                }
            }
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Open…\tCtrl+O").clicked() {
                    ui.close();
                    self.open_dialog();
                }
                if ui.button("Open with…\tAlt+S").clicked() {
                    ui.close();
                    self.open_with();
                }
                ui.menu_button("Open recent", |ui| {
                    let recents = self.settings.recent_files.clone();
                    if recents.is_empty() {
                        ui.label("(nothing yet)");
                    }
                    for path in recents {
                        if ui.button(path.display().to_string()).clicked() {
                            ui.close();
                            self.open(path, false);
                        }
                    }
                    ui.separator();
                    if ui.button("Clear").clicked() {
                        ui.close();
                        self.settings.recent_files.clear();
                    }
                });
                ui.separator();
                if ui.button("Reload\tF5").clicked() {
                    ui.close();
                    self.reload();
                }
                ui.checkbox(&mut self.settings.autoreload, "Autoreload");
                if ui.button("Save Screenshot…").clicked() {
                    ui.close();
                    self.screenshot_pending = true;
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Screenshot(
                            egui::UserData::default(),
                        ));
                }
                ui.separator();
                if ui.button("Quit\tCtrl+Q").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });

            ui.menu_button("View", |ui| {
                ui.menu_button("Projection", |ui| {
                    for p in [Projection::Perspective, Projection::Orthographic] {
                        let label = format!("{p:?}");
                        if ui.radio(self.settings.projection == p, label).clicked() {
                            self.settings.projection = p;
                            self.camera.perspective = p.value();
                            ui.close();
                        }
                    }
                });
                ui.menu_button("Draw Mode", |ui| {
                    for mode in DrawMode::ALL {
                        if ui
                            .radio(self.settings.draw_mode == mode, mode.label())
                            .clicked()
                        {
                            self.set_draw_mode(mode);
                            ui.close();
                        }
                    }
                });
                if ui.button("Draw Mode Settings…").clicked() {
                    ui.close();
                    self.show_light_prefs = true;
                }
                ui.separator();
                ui.menu_button("Viewpoint", |ui| {
                    for (key, v) in viewpoint_keys() {
                        if ui.button(format!("{v:?}\t{key}")).clicked() {
                            ui.close();
                            self.camera.set_viewpoint(v);
                        }
                    }
                });
                ui.separator();
                ui.checkbox(&mut self.settings.draw_axes, "Draw Axes");
                ui.checkbox(&mut self.settings.invert_zoom, "Invert Zoom");
                ui.checkbox(
                    &mut self.settings.reset_transform_on_load,
                    "Reset rotation on load",
                );
                ui.checkbox(&mut self.settings.obj_y_up, "Rotate Y-up OBJ files to Z-up");
                ui.separator();
                if ui.button("Hide Menu Bar\tCtrl+Shift+C").clicked() {
                    ui.close();
                    self.settings.hide_menu_bar = true;
                }
                if ui.button("Toggle Fullscreen\tF11").clicked() {
                    ui.close();
                    self.toggle_fullscreen(ui.ctx());
                }
            });

            ui.menu_button("Help", |ui| {
                if ui.button("About").clicked() {
                    ui.close();
                    self.show_about = true;
                }
            });
        });
    }

    fn toggle_fullscreen(&mut self, ctx: &egui::Context) {
        self.fullscreen = !self.fullscreen;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let consume = |mods: Modifiers, key: Key| {
            ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(mods, key)))
        };

        if consume(Modifiers::COMMAND, Key::O) {
            self.open_dialog();
        }
        if consume(Modifiers::COMMAND, Key::Q) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if consume(Modifiers::ALT, Key::S) {
            self.open_with();
        }
        if consume(Modifiers::NONE, Key::F5) {
            self.reload();
        }
        if consume(Modifiers::NONE, Key::F11) {
            self.toggle_fullscreen(ctx);
        }
        if consume(Modifiers::COMMAND | Modifiers::SHIFT, Key::C) {
            self.settings.hide_menu_bar = !self.settings.hide_menu_bar;
        }
        if consume(Modifiers::NONE, Key::ArrowRight) || consume(Modifiers::NONE, Key::ArrowDown) {
            self.cycle_file(true);
        }
        if consume(Modifiers::NONE, Key::ArrowLeft) || consume(Modifiers::NONE, Key::ArrowUp) {
            self.cycle_file(false);
        }
        for (key, v) in viewpoint_keys() {
            let k = match key {
                '0' => Key::Num0,
                '1' => Key::Num1,
                '2' => Key::Num2,
                '3' => Key::Num3,
                '4' => Key::Num4,
                '5' => Key::Num5,
                '6' => Key::Num6,
                _ => Key::Num9,
            };
            if consume(Modifiers::NONE, k) {
                self.camera.set_viewpoint(v);
            }
        }
    }

    /// True while the pointer is dragging files over the window.
    fn files_hovering(ctx: &egui::Context) -> bool {
        ctx.input(|i| !i.raw.hovered_files.is_empty())
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if dropped.is_empty() {
            return;
        }
        match dropped.iter().find(|p| loader::detect(p).is_some()) {
            Some(path) => self.open(path.clone(), false),
            None => {
                let names: Vec<String> = dropped.iter().map(|p| file_name(p)).collect();
                self.set_status(format!(
                    "Cannot open {} — only .stl, .3mf and .obj files are supported",
                    names.join(", ")
                ));
            }
        }
    }

    fn handle_screenshot(&mut self, ctx: &egui::Context) {
        if !self.screenshot_pending {
            return;
        }
        let image = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        let Some(image) = image else {
            return;
        };
        self.screenshot_pending = false;

        if let Some(target) = self.screenshot_to.clone() {
            match save_screenshot(&image, &target) {
                Ok(()) => log::info!("wrote {}", target.display()),
                Err(e) => {
                    log::error!("could not write {}: {e}", target.display());
                    self.screenshot_failed.store(true, Ordering::Relaxed);
                }
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        let default_name = self
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| format!("{}.png", s.to_string_lossy()))
            .unwrap_or_else(|| "screenshot.png".to_owned());
        let Some(target) = rfd::FileDialog::new()
            .add_filter("PNG image", &["png"])
            .set_file_name(default_name)
            .save_file()
        else {
            return;
        };

        match save_screenshot(&image, &target) {
            Ok(()) => self.set_status(format!("Saved {}", file_name(&target))),
            Err(e) => self.set_status(format!("Could not save screenshot: {e}")),
        }
    }

    /// Mouse look: arcball on the left button, pan on the right, zoom on wheel.
    fn handle_camera_input(&mut self, ui: &egui::Ui, response: &egui::Response, rect: egui::Rect) {
        let (w, h) = (rect.width(), rect.height());
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let local = |p: egui::Pos2| [p.x - rect.left(), p.y - rect.top()];

        let pointer = response.interact_pointer_pos().or(response.hover_pos());
        if response.dragged_by(PointerButton::Primary) {
            if let (Some(prev), Some(now)) = (self.last_pointer, pointer) {
                self.camera.rotate(local(prev), local(now), w, h);
            }
        } else if response.dragged_by(PointerButton::Secondary) {
            let d = response.drag_delta();
            if d != egui::Vec2::ZERO {
                self.camera.pan([d.x, d.y], w, h);
            }
        }
        self.last_pointer = pointer;

        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let cursor = pointer.map(local).unwrap_or([w / 2.0, h / 2.0]);
                self.camera
                    .zoom_at(cursor, scroll, self.settings.invert_zoom, w, h);
            }
        }
    }

    fn scene_params(&self, rect: egui::Rect) -> SceneParams {
        let (w, h) = (rect.width().max(1.0), rect.height().max(1.0));
        let dirs = light_directions();
        let light = dirs
            .get(self.settings.light_direction)
            .map(|(d, _)| *d)
            .unwrap_or([0.0, 0.0, 1.0]);

        let aspect = self.camera.aspect_matrix(w, h);
        let ar = w / h;
        let hud_size = 0.2;
        let hud = Mat4::from_translation(if ar > 1.0 {
            Vec3::new(ar - 2.0 * hud_size, -1.0 + 2.0 * hud_size, 0.0)
        } else {
            Vec3::new(1.0 - 2.0 * hud_size, -1.0 / ar + 2.0 * hud_size, 0.0)
        }) * Mat4::from_scale(Vec3::new(hud_size, hud_size, 1.0));
        let hud_view = aspect * hud;

        let mut line_mvps = [Mat4::IDENTITY; 5];
        line_mvps[0] = self.camera.mvp(w, h);
        line_mvps[1] = Camera::to_wgpu_clip(hud_view * self.camera.orient);
        for axis in 0..3 {
            let mut v = Vec3::ZERO;
            v[axis] = 1.25;
            let label = Mat4::from_translation(self.camera.orient.transform_point3(v));
            line_mvps[2 + axis] = Camera::to_wgpu_clip(hud_view * label);
        }

        let a = self.settings.ambient_color;
        let d = self.settings.directive_color;
        SceneParams {
            mvp: self.camera.mvp(w, h),
            ambient: [a[0], a[1], a[2], self.settings.ambient_factor],
            directive: [d[0], d[1], d[2], self.settings.directive_factor],
            light_dir: light,
            zoom_inv: 1.0 / self.camera.zoom,
            has_colors: self.mesh.as_ref().is_some_and(|m| m.has_colors),
            draw_mode: self.settings.draw_mode,
            draw_axes: self.settings.draw_axes && self.mesh.is_some(),
            line_mvps,
        }
    }

    fn overlays(&self, ui: &egui::Ui, rect: egui::Rect) {
        let painter = ui.painter_at(rect);
        let font = egui::FontId::proportional(13.0);
        if !self.mesh_info.is_empty() {
            painter.text(
                rect.left_top() + egui::vec2(10.0, 8.0),
                egui::Align2::LEFT_TOP,
                &self.mesh_info,
                font.clone(),
                egui::Color32::from_gray(220),
            );
        }
        if !self.status.is_empty() && self.status_at.elapsed() < STATUS_TIMEOUT {
            painter.text(
                rect.left_bottom() + egui::vec2(10.0, -8.0),
                egui::Align2::LEFT_BOTTOM,
                &self.status,
                font,
                egui::Color32::from_gray(220),
            );
        }
    }

    fn light_prefs_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_light_prefs;
        egui::Window::new("Draw Mode Settings")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                egui::Grid::new("light prefs")
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label("Ambient color");
                        ui.color_edit_button_rgb(&mut self.settings.ambient_color);
                        ui.end_row();
                        ui.label("Ambient factor");
                        ui.add(egui::Slider::new(
                            &mut self.settings.ambient_factor,
                            0.0..=2.0,
                        ));
                        ui.end_row();
                        ui.label("Directive color");
                        ui.color_edit_button_rgb(&mut self.settings.directive_color);
                        ui.end_row();
                        ui.label("Directive factor");
                        ui.add(egui::Slider::new(
                            &mut self.settings.directive_factor,
                            0.0..=2.0,
                        ));
                        ui.end_row();
                        ui.label("Light direction");
                        let dirs = light_directions();
                        let current = dirs
                            .get(self.settings.light_direction)
                            .map(|(_, n)| n.clone())
                            .unwrap_or_default();
                        egui::ComboBox::from_id_salt("light dir")
                            .selected_text(current)
                            .show_ui(ui, |ui| {
                                for (i, (_, name)) in dirs.iter().enumerate() {
                                    ui.selectable_value(
                                        &mut self.settings.light_direction,
                                        i,
                                        name,
                                    );
                                }
                            });
                        ui.end_row();
                    });
                ui.separator();
                if ui.button("Reset to defaults").clicked() {
                    let d = Settings::default();
                    self.settings.ambient_color = d.ambient_color;
                    self.settings.ambient_factor = d.ambient_factor;
                    self.settings.directive_color = d.directive_color;
                    self.settings.directive_factor = d.directive_factor;
                    self.settings.light_direction = d.light_direction;
                }
                ui.label("These settings apply to the Mesh Light draw mode.");
            });
        self.show_light_prefs = open;
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_about;
        egui::Window::new("About view3d")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(format!("view3d {}", env!("CARGO_PKG_VERSION")));
                ui.label("A fast STL / 3MF / OBJ viewer, written in Rust.");
                ui.separator();
                ui.label("Modelled on fstl by Matt Keeter (MIT licensed),");
                ui.label("rendered with wgpu and egui.");
            });
        self.show_about = open;
    }
}

fn viewpoint_keys() -> [(char, ViewPoint); 8] {
    [
        ('0', ViewPoint::Iso),
        ('1', ViewPoint::Top),
        ('2', ViewPoint::Bottom),
        ('3', ViewPoint::Front),
        ('4', ViewPoint::Back),
        ('5', ViewPoint::Left),
        ('6', ViewPoint::Right),
        ('9', ViewPoint::Center),
    ]
}

fn save_screenshot(image: &egui::ColorImage, target: &Path) -> Result<(), String> {
    let (w, h) = (image.width() as u32, image.height() as u32);
    let pixels: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
    let img = image::RgbaImage::from_raw(w, h, pixels)
        .ok_or_else(|| "screenshot buffer had the wrong size".to_owned())?;
    img.save(target).map_err(|e| e.to_string())
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

impl eframe::App for App {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, SETTINGS_KEY, &self.settings);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 1.0]
    }

    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &root.ctx().clone();
        self.poll_load(ctx);
        self.poll_watcher(ctx);
        self.shortcuts(ctx);
        self.handle_drops(ctx);
        self.handle_screenshot(ctx);

        let title = match &self.path {
            Some(p) => format!("{} — view3d", file_name(p)),
            None => "view3d".to_owned(),
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));

        if !self.settings.hide_menu_bar {
            egui::Panel::top("menu").show(root, |ui| self.menu_bar(ui));
        }

        self.light_prefs_window(ctx);
        self.about_window(ctx);

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(root, |ui| {
                let rect = ui.available_rect_before_wrap();
                let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
                self.handle_camera_input(ui, &response, rect);

                ui.painter().add(egui_wgpu::Callback::new_paint_callback(
                    rect,
                    SceneCallback {
                        params: self.scene_params(rect),
                    },
                ));

                if Self::files_hovering(ui.ctx()) {
                    let painter = ui.painter_at(rect);
                    painter.rect_filled(rect, 0.0, egui::Color32::from_black_alpha(120));
                    painter.rect_stroke(
                        rect.shrink(12.0),
                        8.0,
                        egui::Stroke::new(2.0, egui::Color32::from_gray(220)),
                        egui::StrokeKind::Inside,
                    );
                    painter.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "Drop to open",
                        egui::FontId::proportional(22.0),
                        egui::Color32::from_gray(240),
                    );
                } else if self.mesh.is_none() && self.pending.is_none() {
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "Open a .stl, .3mf or .obj file  (Ctrl+O, or drop one here)",
                        egui::FontId::proportional(16.0),
                        egui::Color32::from_gray(200),
                    );
                }
                self.overlays(ui, rect);
            });

        self.drive_auto_screenshot(ctx);

        if self.status_at.elapsed() < STATUS_TIMEOUT {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }
}
