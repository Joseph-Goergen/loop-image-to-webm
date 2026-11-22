// No console window for the GUI on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use eframe::egui::{self, Color32, RichText, Sense, Stroke, TextureHandle, TextureOptions, Vec2};
use image::RgbaImage;
use loopwebm_core::{self as core, Axis, Direction, FramePlan, LoopOptions, RenderSummary};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::Instant;

/// Longest edge of the preview texture.
const PREVIEW_MAX: u32 = 1600;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("LoopWebM")
            .with_inner_size([1100.0, 680.0])
            .with_min_inner_size([720.0, 460.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native("LoopWebM", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

struct Loaded {
    path: PathBuf,
    full: Arc<RgbaImage>,
    preview: RgbaImage,
}

struct Job {
    done: Arc<AtomicU32>,
    total: u32,
    cancel: Arc<AtomicBool>,
    rx: Receiver<Result<RenderSummary, core::Error>>,
    started: Instant,
}

enum Status {
    Idle,
    Info(String),
    Error(String),
}

struct App {
    opts: LoopOptions,
    image: Option<Loaded>,
    loading: Option<(PathBuf, Receiver<Result<Loaded, core::Error>>)>,
    texture: Option<TextureHandle>,
    /// Options the current texture was built with (blur / background).
    texture_key: Option<(bool, u32, [u8; 3], Axis)>,
    anim_start: Instant,
    playing: bool,
    job: Option<Job>,
    status: Status,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let mut app = Self {
            opts: LoopOptions::default(),
            image: None,
            loading: None,
            texture: None,
            texture_key: None,
            anim_start: Instant::now(),
            playing: true,
            job: None,
            status: Status::Idle,
        };
        // Allow `loopwebm-gui path/to/image.png`.
        if let Some(p) = std::env::args_os().nth(1) {
            app.load(PathBuf::from(p));
        }
        app
    }

    fn load(&mut self, path: PathBuf) {
        let (tx, rx) = mpsc::channel();
        let p = path.clone();
        std::thread::spawn(move || {
            let r = core::load_image(&p).map(|full| {
                let preview = downscale(&full, PREVIEW_MAX);
                Loaded { path: p, full: Arc::new(full), preview }
            });
            let _ = tx.send(r);
        });
        self.loading = Some((path, rx));
        self.status = Status::Idle;
    }

    fn poll_background(&mut self) {
        if let Some((path, rx)) = &self.loading {
            if let Ok(r) = rx.try_recv() {
                match r {
                    Ok(l) => {
                        self.image = Some(l);
                        self.texture = None;
                        self.texture_key = None;
                        self.anim_start = Instant::now();
                        self.status = Status::Idle;
                    }
                    Err(e) => self.status = Status::Error(format!("{}: {e}", path.display())),
                }
                self.loading = None;
            }
        }
        if let Some(job) = &self.job {
            if let Ok(r) = job.rx.try_recv() {
                self.status = match r {
                    Ok(s) => Status::Info(format!(
                        "Saved {} — {} frames, {:.2}s, {:.2} MB in {:.1}s",
                        s.output.display(),
                        s.frames,
                        s.duration_secs,
                        s.bytes as f64 / 1_048_576.0,
                        job.started.elapsed().as_secs_f64()
                    )),
                    Err(core::Error::Cancelled) => Status::Info("Export cancelled".into()),
                    Err(e) => Status::Error(format!("Export failed: {e}")),
                };
                self.job = None;
            }
        }
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if let Some(p) = dropped.into_iter().next() {
            if self.job.is_none() {
                self.load(p);
            }
        }
    }

    fn ensure_texture(&mut self, ctx: &egui::Context) {
        let Some(img) = &self.image else { return };
        let key = (self.opts.seam_blur, self.opts.blur_width, self.opts.background, self.opts.direction.axis());
        if self.texture.is_some() && self.texture_key == Some(key) {
            return;
        }
        // Scale the blur band to the preview resolution so it looks the same as the export.
        let scale = img.preview.width() as f64 / img.full.width() as f64;
        let mut o = self.opts.clone();
        o.blur_width = ((o.blur_width as f64 * scale).round() as u32).max(1);
        let src = core::prepare_source(&img.preview, &o);
        let color = egui::ColorImage::from_rgba_unmultiplied(
            [src.width as usize, src.height as usize],
            &src.to_rgba(),
        );
        self.texture = Some(ctx.load_texture("preview", color, TextureOptions::LINEAR));
        self.texture_key = Some(key);
    }

    fn start_export(&mut self) {
        let Some(img) = &self.image else { return };
        let default_name = img
            .path
            .file_stem()
            .map(|s| format!("{}.webm", s.to_string_lossy()))
            .unwrap_or_else(|| "loop.webm".into());
        let mut dialog = rfd::FileDialog::new().add_filter("WebM video", &["webm"]).set_file_name(&default_name);
        if let Some(dir) = img.path.parent() {
            dialog = dialog.set_directory(dir);
        }
        let Some(mut out) = dialog.save_file() else { return };
        if out.extension().is_none() {
            out.set_extension("webm");
        }

        let done = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let total = self.opts.frame_count(img.full.width(), img.full.height());
        let (tx, rx) = mpsc::channel();
        let (full, opts, d, c) = (img.full.clone(), self.opts.clone(), done.clone(), cancel.clone());
        std::thread::spawn(move || {
            let r = core::render(&full, &opts, &out, |n, _| d.store(n, Ordering::Relaxed), &c);
            let _ = tx.send(r);
        });
        self.job = Some(Job { done, total, cancel, rx, started: Instant::now() });
        self.status = Status::Idle;
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        let busy = self.job.is_some();
        ui.add_space(6.0);
        ui.heading("LoopWebM");
        ui.label(RichText::new("Seamless scrolling WebM loops").weak());
        ui.add_space(8.0);

        ui.add_enabled_ui(!busy, |ui| {
            if ui.button("Open image…").clicked() {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("Images", core::SUPPORTED_EXTENSIONS)
                    .pick_file()
                {
                    self.load(p);
                }
            }
            ui.separator();

            ui.label(RichText::new("Target FPS").strong());
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(&mut self.opts.fps, 1..=120).show_value(false));
                ui.add(egui::DragValue::new(&mut self.opts.fps).range(1..=core::options::MAX_FPS).suffix(" fps"));
            });
            ui.add_space(4.0);

            ui.label(RichText::new("Pixels per frame").strong());
            ui.horizontal(|ui| {
                ui.add(
                    egui::Slider::new(&mut self.opts.px_per_frame, 0.1..=32.0)
                        .logarithmic(true)
                        .show_value(false),
                );
                ui.add(
                    egui::DragValue::new(&mut self.opts.px_per_frame)
                        .range(0.1..=200.0)
                        .speed(0.05)
                        .max_decimals(2)
                        .suffix(" px"),
                );
            });
            ui.label(RichText::new("Scroll speed = FPS × pixels per frame").weak().small());
            ui.add_space(8.0);

            ui.label(RichText::new("Direction").strong());
            egui::Grid::new("direction").num_columns(2).spacing([6.0, 6.0]).show(ui, |ui| {
                for row in Direction::ALL.chunks(2) {
                    for &d in row {
                        let selected = self.opts.direction == d;
                        let btn = egui::Button::selectable(selected, d.label()).min_size(Vec2::new(124.0, 26.0));
                        if ui.add(btn).clicked() {
                            self.opts.direction = d;
                        }
                    }
                    ui.end_row();
                }
            });
            ui.label(
                RichText::new(match self.opts.direction.axis() {
                    Axis::Horizontal => "Left and right edges are stitched together",
                    Axis::Vertical => "Top and bottom edges are stitched together",
                })
                .weak()
                .small(),
            );
            ui.add_space(8.0);

            ui.label(RichText::new("Seam").strong());
            toggle_row(ui, &mut self.opts.seam_blur, "Blur the seam");
            if self.opts.seam_blur {
                ui.add(egui::Slider::new(&mut self.opts.blur_width, 1..=256).text("px each side").logarithmic(true));
            }
            ui.add_space(8.0);

            egui::CollapsingHeader::new("Advanced").default_open(false).show(ui, |ui| {
                egui::Grid::new("adv").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                    ui.label("Quality (CRF)");
                    ui.add(egui::Slider::new(&mut self.opts.crf, 0..=63))
                        .on_hover_text("Lower = better quality, larger file");
                    ui.end_row();
                    ui.label("Encoder speed");
                    ui.add(egui::Slider::new(&mut self.opts.cpu_used, 0..=8))
                        .on_hover_text("Higher = faster encode, slightly lower quality");
                    ui.end_row();
                    ui.label("Background");
                    ui.color_edit_button_srgb(&mut self.opts.background)
                        .on_hover_text("Used behind transparent pixels");
                    ui.end_row();
                });
                if ui.small_button("Reset to defaults").clicked() {
                    self.opts = LoopOptions::default();
                }
            });
        });

        ui.separator();
        if let Some(img) = &self.image {
            let (w, h) = img.full.dimensions();
            let n = self.opts.frame_count(w, h);
            let secs = self.opts.duration_secs(w, h);
            let axis_len = self.opts.direction.axis_len(w, h);
            egui::Grid::new("info").num_columns(2).show(ui, |ui| {
                ui.label("Image");
                ui.label(format!("{w} × {h}"));
                ui.end_row();
                ui.label("Frames");
                ui.label(n.to_string());
                ui.end_row();
                ui.label("Loop length");
                ui.label(format!("{secs:.2} s"));
                ui.end_row();
                ui.label("Speed");
                ui.label(format!("{:.0} px/s", axis_len as f64 / secs));
                ui.end_row();
            });
            ui.add_space(8.0);
        }

        if let Some(job) = &self.job {
            let done = job.done.load(Ordering::Relaxed);
            let frac = done as f32 / job.total.max(1) as f32;
            ui.add(egui::ProgressBar::new(frac).show_percentage().animate(true));
            ui.label(format!("Encoding frame {done} / {}", job.total));
            if ui.button("Cancel").clicked() {
                job.cancel.store(true, Ordering::Relaxed);
            }
        } else {
            let enabled = self.image.is_some();
            let btn = egui::Button::new(RichText::new("Export WebM…").strong()).min_size(Vec2::new(180.0, 32.0));
            if ui.add_enabled(enabled, btn).clicked() {
                self.start_export();
            }
        }
    }

    fn preview(&mut self, ui: &mut egui::Ui) {
        let avail = ui.available_rect_before_wrap();
        let hovering = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
        let (rect, resp) = ui.allocate_exact_size(avail.size(), Sense::click());
        let painter = ui.painter_at(rect);
        let visuals = ui.visuals().clone();

        match (&self.image, &self.texture) {
            (Some(img), Some(tex)) => {
                let (w, h) = (img.preview.width() as f32, img.preview.height() as f32);
                let s = (rect.width() / w).min(rect.height() / h);
                let size = Vec2::new(w * s, h * s);
                let area = egui::Rect::from_center_size(rect.center(), size);

                let (full_w, full_h) = img.full.dimensions();
                let n = self.opts.frame_count(full_w, full_h);
                let plan = FramePlan::for_image(full_w, full_h, n, self.opts.direction);
                let t = self.anim_start.elapsed().as_secs_f64();
                let k = if self.playing { (t * self.opts.fps as f64) as u64 % n as u64 } else { 0 } as u32;
                // Fraction of the scroll axis to shift (positive = sample further right / down).
                let frac = (plan.shift(k) / plan.len as f64).rem_euclid(1.0) as f32;
                // Draw the image twice, end to end along the axis, offset by the shift.
                let step = match plan.axis() {
                    Axis::Horizontal => Vec2::new(size.x, 0.0),
                    Axis::Vertical => Vec2::new(0.0, size.y),
                };

                let clip = painter.with_clip_rect(area);
                let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                for i in 0..2 {
                    let r = egui::Rect::from_min_size(area.min + step * (i as f32 - frac), size);
                    clip.image(tex.id(), r, uv, Color32::WHITE);
                }
                painter.rect_stroke(area, 2.0, Stroke::new(1.0, visuals.weak_text_color()), egui::StrokeKind::Outside);

                let label = if self.playing { "click to pause" } else { "click to play" };
                painter.text(
                    area.left_bottom() + Vec2::new(8.0, -8.0),
                    egui::Align2::LEFT_BOTTOM,
                    format!("{}   {}", file_name(&img.path), label),
                    egui::FontId::proportional(13.0),
                    Color32::from_white_alpha(200),
                );
                if resp.clicked() {
                    self.playing = !self.playing;
                    self.anim_start = Instant::now();
                }
                if self.playing {
                    ui.ctx().request_repaint();
                }
            }
            _ => {
                let msg = if self.loading.is_some() {
                    "Loading…"
                } else {
                    "Drop an image here\nor click to choose one"
                };
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    msg,
                    egui::FontId::proportional(22.0),
                    visuals.weak_text_color(),
                );
                if resp.clicked() && self.job.is_none() {
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter("Images", core::SUPPORTED_EXTENSIONS)
                        .pick_file()
                    {
                        self.load(p);
                    }
                }
            }
        }

        if hovering {
            painter.rect_filled(rect.shrink(4.0), 8.0, Color32::from_rgba_unmultiplied(60, 120, 255, 40));
            painter.rect_stroke(rect.shrink(4.0), 8.0, Stroke::new(2.0, Color32::from_rgb(90, 150, 255)), egui::StrokeKind::Inside);
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Drop to load",
                egui::FontId::proportional(26.0),
                Color32::WHITE,
            );
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        self.poll_background();
        self.handle_drops(ctx);
        self.ensure_texture(ctx);
        if self.job.is_some() || self.loading.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| match &self.status {
                Status::Idle => {
                    ui.label(RichText::new(format!("libvpx {}", core::encode::vpx_version())).weak().small());
                }
                Status::Info(s) => {
                    ui.label(RichText::new(s).color(Color32::from_rgb(120, 200, 120)));
                }
                Status::Error(s) => {
                    ui.label(RichText::new(s).color(Color32::from_rgb(240, 110, 110)));
                }
            });
        });
        egui::Panel::left("controls").resizable(false).exact_size(290.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.controls(ui));
        });
        egui::CentralPanel::default().show(ui, |ui| self.preview(ui));
    }
}

fn toggle_row(ui: &mut egui::Ui, on: &mut bool, label: &str) {
    ui.horizontal(|ui| {
        toggle(ui, on);
        if ui.add(egui::Label::new(label).sense(Sense::click())).clicked() {
            *on = !*on;
        }
    });
}

/// iOS-style switch.
fn toggle(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let size = ui.spacing().interact_size.y * egui::vec2(2.0, 1.0);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(response.id, *on);
        let visuals = ui.style().interact_selectable(&response, *on);
        let rect = rect.expand(visuals.expansion);
        let radius = 0.5 * rect.height();
        ui.painter().rect(rect, radius, visuals.bg_fill, visuals.bg_stroke, egui::StrokeKind::Inside);
        let cx = egui::lerp((rect.left() + radius)..=(rect.right() - radius), t);
        ui.painter().circle(egui::pos2(cx, rect.center().y), 0.75 * radius, visuals.fg_stroke.color, visuals.fg_stroke);
    }
    response
}

fn downscale(img: &RgbaImage, max: u32) -> RgbaImage {
    let (w, h) = img.dimensions();
    if w <= max && h <= max {
        return img.clone();
    }
    let s = max as f64 / w.max(h) as f64;
    let nw = ((w as f64 * s).round() as u32).max(1);
    let nh = ((h as f64 * s).round() as u32).max(1);
    image::imageops::resize(img, nw, nh, image::imageops::FilterType::Triangle)
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}
