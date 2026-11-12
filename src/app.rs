//! One-page UI. Left = photo tools (cards), centre = crop + result / sheet preview, right = print & export.
use crate::brand::{self, BLUE, BLUE_DK, BORDER, CARD, FIELD, GREEN, MUTED, ORANGE, RED, TEXT};
use crate::pipeline::*;
use crate::printer;
use crate::seg;
use crate::sheet::{self, Align, Border, SheetOpts};
use eframe::egui::{self, pos2, vec2, Color32, CornerRadius, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Ui, Vec2};
use image::{GrayImage, ImageDecoder, RgbImage, RgbaImage};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

#[path = "app_extra.rs"]
mod extra;
use extra::{AiState, DocState, ModelsUi};
#[path = "app_duet.rs"]
mod duet;
use duet::{CellDrag, Hist, Stash};
#[path = "app_batch.rs"]
mod bat;
use bat::BatchState;

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
struct Settings {
    model_key: String,
    auto_bg: bool,
    last_dir: String,
    sheet: SheetOpts,
    jpeg_q: u8,
    online_w: u32,
    online_h: u32,
    online_kb: u32,
    size_idx: usize,
    custom_w_mm: f32,
    custom_h_mm: f32,
    dpi: u32,
    bg: [u8; 3],
    duet: bool,
    duet_w_mm: f32,
    duet_h_mm: f32,
    duet_between_mm: f32,
    duet_stack: bool,
    brush_px: f32,
    brush_hard: f32,
    brush_strength: f32,
    cut_feather: f32,
    doc: crate::doc::DocOpts,
    page: crate::doc::PageOpts,
    doc_size_idx: usize,
    doc_w_mm: f32,
    doc_h_mm: f32,
    doc_portrait: bool,
    doc_keep_original: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Settings {
            model_key: seg::MODELS[0].key.into(),
            auto_bg: true,
            last_dir: String::new(),
            sheet: SheetOpts::default(),
            jpeg_q: 95,
            online_w: 354,
            online_h: 472,
            online_kb: 50,
            size_idx: 0,
            custom_w_mm: 35.0,
            custom_h_mm: 45.0,
            dpi: 300,
            bg: [255, 255, 255],
            duet: false,
            duet_w_mm: 48.26,
            duet_h_mm: 38.1,
            duet_between_mm: 0.0,
            duet_stack: false,
            brush_px: 30.0,
            brush_hard: 0.6,
            brush_strength: 1.0,
            cut_feather: 4.0,
            doc: Default::default(),
            page: Default::default(),
            doc_size_idx: 0,
            doc_w_mm: 85.6,
            doc_h_mm: 54.0,
            doc_portrait: false,
            doc_keep_original: false,
        }
    }
}
fn settings_path() -> PathBuf {
    seg::app_dir().join("settings.json")
}

enum Msg {
    Seg(Result<GrayImage, String>, bool, u64),
    Dl(u64, u64),
    DlDone(Result<(), String>),
    Rendered(u64, RgbImage),
    MDl(String, u64, u64),
    MDone(String, Result<(), String>),
    MOp(Result<String, String>),
    AiProg(f32, String),
    AiDone(u64, Result<extra::AiOut, String>),
    DocDetect(usize, Result<Option<([crate::doc::Pt; 4], f32)>, String>),
    DocAi(usize, String, Result<RgbImage, String>),
    Printed(Result<String, String>),
    BatchLoaded(u64, Result<RgbImage, String>),
    BatchSeg(u64, Result<GrayImage, String>),
}
struct Job {
    id: u64,
    src: Arc<RgbImage>,
    alpha: Option<Arc<GrayImage>>,
    edit: Edit,
    dress: Option<Arc<RgbaImage>>,
}

#[derive(PartialEq, Clone, Copy)]
enum View {
    Edit,
    Sheet,
    Doc,
    Batch,
}
#[derive(PartialEq, Clone, Copy)]
enum Tool {
    None,
    Heal,
    Restore,
    Erase,
    /// click points to draw a shape
    Pen,
    /// drag to draw a free shape
    Lasso,
}
#[derive(Clone, Copy)]
enum Drag {
    None,
    Move,
    Resize { start_dist: f32, cw0: f32 },
}
/// Brush stroke shown immediately on screen until the re-rendered picture arrives (photo-pixel coords).
struct Overlay {
    erase: bool,
    r: f32,
    pts: Vec<Pos2>,
}

pub struct App {
    settings: Settings,
    edit: Edit,
    // picture
    src: Option<Arc<RgbImage>>,
    src_path: Option<PathBuf>,
    src_tex: Option<TextureHandle>,
    alpha: Option<Arc<GrayImage>>,
    alpha_ver: u32,
    img_id: u64,
    // dress
    dresses: Vec<(String, PathBuf)>,
    dress_sel: Option<usize>,
    dress_img: Option<Arc<RgbaImage>>,
    // render
    job_tx: Sender<Job>,
    job_id: u64,
    shown_id: u64,
    last_key: Option<(Edit, u32)>,
    last_aspect: f32,
    photo: Option<RgbImage>,
    unit: Option<RgbImage>, // what is printed: the photo (+caption) or the duet frame
    unit_tex: Option<TextureHandle>,
    photo_ver: u32,
    // duet
    other: Option<Stash>,
    other_img: Option<RgbImage>,
    other_key: Option<(Edit, u32)>,
    cell_drag: Option<CellDrag>,
    hist: Hist,
    batch: BatchState,
    // sheet preview
    view: View,
    sheet_key: Option<(u32, SheetOpts, u32)>,
    sheet_tex: Option<TextureHandle>,
    sheet_info: String,
    // background tasks
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    seg_busy: bool,
    printing: bool,
    dl: Option<(u64, u64)>,
    cancel: Arc<AtomicBool>,
    // ui state
    status: String,
    error: Option<String>,
    show_guides: bool,
    tool: Tool,
    heal_r: f32,
    brush_px: f32,
    brush_hard: f32,
    brush_strength: f32,
    cut_feather: f32,
    cut_mode: CutMode,
    /// shape being drawn: (photo px, source px)
    pen: Vec<(Pos2, (f32, f32))>,
    painting: bool,
    overlay: Vec<Overlay>,
    overlay_clear_id: u64,
    rz: f32,
    rpan: Vec2,
    drag: Drag,
    logo: TextureHandle,
    online_msg: String,
    add_date: bool,
    ai: AiState,
    doc: DocState,
    mui: ModelsUi,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> App {
        brand::apply_theme(&cc.egui_ctx);
        let logo = cc.egui_ctx.load_texture("logo", brand::logo_image(), TextureOptions::LINEAR);
        let settings: Settings = std::fs::read_to_string(settings_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let (tx, rx) = channel();
        let (job_tx, job_rx) = channel::<Job>();
        {
            let tx = tx.clone();
            let ctx = cc.egui_ctx.clone();
            std::thread::spawn(move || {
                while let Ok(mut job) = job_rx.recv() {
                    while let Ok(j) = job_rx.try_recv() {
                        job = j;
                    }
                    let p = render_photo(&job.src, job.alpha.as_deref(), &job.edit, job.dress.as_deref());
                    let p = if job.edit.duet { p } else { add_caption(&p, &job.edit) };
                    let _ = tx.send(Msg::Rendered(job.id, p));
                    ctx.request_repaint();
                }
            });
        }
        let mut edit = Edit::default();
        edit.size_idx = settings.size_idx.min(PRESETS.len() - 1);
        edit.custom_w_mm = settings.custom_w_mm;
        edit.custom_h_mm = settings.custom_h_mm;
        edit.dpi = settings.dpi;
        edit.bg = settings.bg;
        edit.duet = settings.duet;
        edit.duet_w_mm = settings.duet_w_mm;
        edit.duet_h_mm = settings.duet_h_mm;
        edit.duet_between_mm = settings.duet_between_mm;
        edit.duet_stack = settings.duet_stack;
        let last_aspect = edit.aspect();
        let brush_px = settings.brush_px.clamp(6.0, 160.0);
        let (settings_hard, settings_strength, settings_feather) = (settings.brush_hard.clamp(0.0, 1.0), settings.brush_strength.clamp(0.05, 1.0), settings.cut_feather.clamp(0.0, 60.0));
        let mut app = App {
            settings,
            edit,
            src: None,
            src_path: None,
            src_tex: None,
            alpha: None,
            alpha_ver: 0,
            img_id: 0,
            dresses: find_dresses(),
            dress_sel: None,
            dress_img: None,
            job_tx,
            job_id: 0,
            shown_id: 0,
            last_key: None,
            last_aspect,
            photo: None,
            unit: None,
            unit_tex: None,
            photo_ver: 0,
            other: None,
            other_img: None,
            other_key: None,
            cell_drag: None,
            hist: Hist::default(),
            batch: BatchState::default(),
            view: View::Edit,
            sheet_key: None,
            sheet_tex: None,
            sheet_info: String::new(),
            tx,
            rx,
            seg_busy: false,
            printing: false,
            dl: None,
            cancel: Arc::new(AtomicBool::new(false)),
            status: "Open a photo to start  (or drag & drop it here)".into(),
            error: None,
            show_guides: true,
            tool: Tool::None,
            heal_r: 0.018,
            brush_px,
            brush_hard: settings_hard,
            brush_strength: settings_strength,
            cut_feather: settings_feather,
            cut_mode: CutMode::Erase,
            pen: Vec::new(),
            painting: false,
            overlay: Vec::new(),
            overlay_clear_id: 0,
            rz: 1.0,
            rpan: Vec2::ZERO,
            drag: Drag::None,
            logo,
            online_msg: String::new(),
            add_date: false,
            ai: AiState::new(),
            doc: DocState::new(),
            mui: ModelsUi::new(),
        };
        if !seg::MODELS.iter().any(|m| m.key == app.settings.model_key) {
            app.settings.model_key = seg::MODELS[0].key.into(); // e.g. the removed Silueta
        }
        seg::warm_up(&app.settings.model_key);
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.first().map(|a| a == "--batch").unwrap_or(false) {
            app.view = View::Batch;
            app.batch_import(&cc.egui_ctx, args.iter().skip(1).map(PathBuf::from).collect());
        } else if args.first().map(|a| a == "--doc").unwrap_or(false) {
            app.view = View::Doc;
            for (i, p) in args.iter().skip(1).take(2).enumerate() {
                app.doc_open(&cc.egui_ctx, i, PathBuf::from(p));
            }
            app.doc.active = 0;
        } else if let Some(p) = args.first() {
            app.open_path(&cc.egui_ctx, PathBuf::from(p));
        }
        app
    }

    fn save_settings(&mut self) {
        let e = &self.edit;
        let s = &mut self.settings;
        s.size_idx = e.size_idx;
        s.custom_w_mm = e.custom_w_mm;
        s.custom_h_mm = e.custom_h_mm;
        s.dpi = e.dpi;
        s.bg = e.bg;
        s.duet = e.duet;
        s.duet_w_mm = e.duet_w_mm;
        s.duet_h_mm = e.duet_h_mm;
        s.duet_between_mm = e.duet_between_mm;
        s.duet_stack = e.duet_stack;
        s.brush_px = self.brush_px;
        s.brush_hard = self.brush_hard;
        s.brush_strength = self.brush_strength;
        s.cut_feather = self.cut_feather;
        if let Ok(j) = serde_json::to_string_pretty(&self.settings) {
            let _ = std::fs::write(settings_path(), j);
        }
    }

    // ------------------------------------------------------------ picture loading
    fn open_path(&mut self, ctx: &egui::Context, path: PathBuf) {
        match load_image(&path) {
            Ok(img) => {
                if let Some(d) = path.parent() {
                    self.settings.last_dir = d.to_string_lossy().into();
                }
                let (w, h) = img.dimensions();
                let k = (1600.0 / w.max(h) as f32).min(1.0);
                let prev = if k < 1.0 {
                    image::imageops::resize(&img, (w as f32 * k) as u32, (h as f32 * k) as u32, image::imageops::FilterType::Triangle)
                } else {
                    img.clone()
                };
                let ci = egui::ColorImage::from_rgb([prev.width() as usize, prev.height() as usize], prev.as_raw());
                self.src_tex = Some(ctx.load_texture("src", ci, TextureOptions::LINEAR));
                self.src = Some(Arc::new(img));
                self.alpha = None;
                self.alpha_ver += 1;
                self.img_id += 1;
                self.edit.spots.clear();
                self.edit.strokes.clear();
                self.edit.cuts.clear();
                self.overlay.clear();
                self.tool = Tool::None;
                self.rz = 1.0;
                self.rpan = Vec2::ZERO;
                self.edit.reset_crop(w, h);
                self.hist_reset();
                self.status = format!("{}  -  {} x {} px", path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), w, h);
                self.src_path = Some(path);
                self.last_key = None;
                if self.settings.auto_bg {
                    if seg::is_installed(&self.settings.model_key) {
                        self.start_seg(ctx, true);
                    } else {
                        self.status.push_str("   |   Download the background AI model (left panel) to remove the background");
                    }
                }
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn pick_open(&mut self, ctx: &egui::Context) {
        let mut d = rfd::FileDialog::new().add_filter("Photos", &["jpg", "jpeg", "png", "bmp", "webp"]);
        if !self.settings.last_dir.is_empty() {
            d = d.set_directory(&self.settings.last_dir);
        }
        if let Some(p) = d.pick_file() {
            self.open_path(ctx, p);
        }
    }

    fn start_seg(&mut self, ctx: &egui::Context, auto_crop: bool) {
        let Some(src) = self.src.clone() else { return };
        if self.seg_busy {
            return;
        }
        self.seg_busy = true;
        self.status = "Removing background (AI)...".into();
        let key = self.settings.model_key.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let id = self.img_id;
        std::thread::spawn(move || {
            let r = std::panic::catch_unwind(|| seg::segment(&src, &key, 1536)).unwrap_or_else(|_| Err("AI engine crashed".into()));
            let _ = tx.send(Msg::Seg(r, auto_crop, id));
            ctx.request_repaint();
        });
    }

    fn start_download(&mut self, ctx: &egui::Context) {
        if self.dl.is_some() {
            return;
        }
        self.cancel.store(false, Ordering::Relaxed);
        self.dl = Some((0, 1));
        let key = self.settings.model_key.clone();
        let (tx, cancel, ctx) = (self.tx.clone(), self.cancel.clone(), ctx.clone());
        std::thread::spawn(move || {
            let (tx2, ctx2) = (tx.clone(), ctx.clone());
            let r = seg::download(&key, &move |d, t| {
                let _ = tx2.send(Msg::Dl(d, t));
                ctx2.request_repaint();
            }, &cancel);
            let _ = tx.send(Msg::DlDone(r));
            ctx.request_repaint();
        });
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                m @ (Msg::BatchLoaded(..) | Msg::BatchSeg(..)) => self.batch_msg(ctx, m),
                Msg::Seg(r, auto, id) => {
                    self.seg_busy = false;
                    if id != self.img_id {
                        continue;
                    }
                    match r {
                        Ok(a) => {
                            if auto {
                                if let Some(s) = &self.src {
                                    self.status = if self.edit.auto_crop(&a, s.width(), s.height()) {
                                        "Background removed + head placed. Fine-tune the frame, or fix edges with the brush.".into()
                                    } else {
                                        "Background removed, but no person was found - adjust the crop by hand.".into()
                                    };
                                }
                            } else {
                                self.status = "Background removed.".into();
                            }
                            self.alpha = Some(Arc::new(a));
                            self.alpha_ver += 1;
                        }
                        Err(e) => {
                            self.status = "Background removal failed".into();
                            self.error = Some(e);
                        }
                    }
                }
                Msg::Printed(r) => {
                    self.printing = false;
                    match r {
                        Ok(m) => self.status = m,
                        Err(e) => {
                            self.status = "Printing failed".into();
                            self.error = Some(e);
                        }
                    }
                }
                Msg::Dl(d, t) => self.dl = Some((d, t.max(1))),
                Msg::DlDone(r) => {
                    self.dl = None;
                    match r {
                        Ok(()) => {
                            self.status = "AI model downloaded.".into();
                            if self.src.is_some() && self.alpha.is_none() {
                                self.start_seg(ctx, true);
                            }
                        }
                        Err(e) if e == "Cancelled" => self.status = "Download cancelled.".into(),
                        Err(e) => self.error = Some(e),
                    }
                }
                Msg::Rendered(id, img) => {
                    if id >= self.shown_id {
                        self.shown_id = id;
                        self.photo = Some(img);
                        self.rebuild_unit(ctx);
                        if !self.painting && id >= self.overlay_clear_id {
                            self.overlay.clear();
                        }
                    }
                }
                other => self.extra_msg(ctx, other),
            }
        }
    }

    /// The printable unit = single photo (with caption) or the duet frame (A | B).
    fn rebuild_unit(&mut self, ctx: &egui::Context) {
        self.ensure_other();
        let Some(p) = &self.photo else { return };
        let u = if self.edit.duet {
            let (a, b) = self.duet_images(p, self.other_img.as_ref());
            compose_duet(a, b, &self.edit)
        } else {
            p.clone()
        };
        let ci = egui::ColorImage::from_rgb([u.width() as usize, u.height() as usize], u.as_raw());
        self.unit_tex = Some(ctx.load_texture("unit", ci, TextureOptions::LINEAR));
        self.unit = Some(u);
        self.photo_ver += 1;
    }

    fn submit_render(&mut self) {
        let Some(src) = &self.src else { return };
        let key = (self.edit.clone(), self.alpha_ver);
        if self.last_key.as_ref() == Some(&key) {
            return;
        }
        self.job_id += 1;
        let _ = self.job_tx.send(Job { id: self.job_id, src: src.clone(), alpha: self.alpha.clone(), edit: self.edit.clone(), dress: self.dress_img.clone() });
        self.last_key = Some(key);
    }

    fn final_photo(&self) -> Option<RgbImage> {
        let src = self.src.as_ref()?;
        let p = render_photo(src, self.alpha.as_deref(), &self.edit, self.dress_img.as_deref());
        Some(if self.edit.duet {
            let o = self.render_other_pub();
            let (a, b) = self.duet_images(&p, o.as_ref());
            compose_duet(a, b, &self.edit)
        } else {
            add_caption(&p, &self.edit)
        })
    }

    fn base_name(&self) -> String {
        let stem = self.src_path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "photo".into());
        if self.edit.duet {
            return format!("{}_duet", stem);
        }
        let (w, h) = self.edit.size_mm();
        format!("{}_{}x{}", stem, w.round(), h.round())
    }

    fn save_dialog(&mut self, name: &str, filter: (&str, &[&str])) -> Option<PathBuf> {
        let mut d = rfd::FileDialog::new().set_file_name(name).add_filter(filter.0, filter.1);
        if let Some(dir) = self.src_path.as_ref().and_then(|p| p.parent()) {
            d = d.set_directory(dir);
        }
        d.save_file()
    }

    // ------------------------------------------------------------ exports
    fn save_photo(&mut self, png: bool) {
        let Some(img) = self.final_photo() else { return };
        let ext = if png { "png" } else { "jpg" };
        let name = format!("{}.{}", self.base_name(), ext);
        if let Some(p) = self.save_dialog(&name, if png { ("PNG", &["png"]) } else { ("JPEG", &["jpg", "jpeg"]) }) {
            let bytes = if png { sheet::encode_png(&img, self.edit.dpi) } else { sheet::encode_jpeg(&img, self.settings.jpeg_q, self.edit.dpi) };
            self.status = match std::fs::write(&p, bytes) {
                Ok(()) => format!("Saved {}", p.display()),
                Err(e) => format!("Could not save: {e}"),
            };
        }
    }

    fn sheets(&mut self) -> Option<Vec<RgbImage>> {
        let img = self.final_photo()?;
        match sheet::make_sheets(&img, &self.settings.sheet, self.edit.dpi) {
            Ok(p) => Some(p),
            Err(e) => {
                self.error = Some(e);
                None
            }
        }
    }

    fn save_sheet(&mut self, kind: &str) {
        let name = format!("{}_sheet.{}", self.base_name(), kind);
        let filter: (&str, &[&str]) = match kind {
            "pdf" => ("PDF", &["pdf"]),
            "png" => ("PNG", &["png"]),
            _ => ("JPEG", &["jpg", "jpeg"]),
        };
        let Some(path) = self.save_dialog(&name, filter) else { return };
        let Some(pages) = self.sheets() else { return };
        let dpi = self.edit.dpi;
        let q = self.settings.jpeg_q;
        let res: std::io::Result<()> = (|| {
            match kind {
                "pdf" => std::fs::write(&path, sheet::encode_pdf(&pages, dpi, q))?,
                _ => {
                    for (i, pg) in pages.iter().enumerate() {
                        let mut p = path.clone();
                        if pages.len() > 1 {
                            let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
                            p.set_file_name(format!("{}_{}.{}", stem, i + 1, kind));
                        }
                        let b = if kind == "png" { sheet::encode_png(pg, dpi) } else { sheet::encode_jpeg(pg, q, dpi) };
                        std::fs::write(p, b)?;
                    }
                }
            }
            Ok(())
        })();
        self.status = match res {
            Ok(()) => format!("Saved {} sheet page(s) -> {}", pages.len(), path.display()),
            Err(e) => format!("Could not save: {e}"),
        };
    }

    fn print_sheet(&mut self, ctx: &egui::Context) {
        let Some(pages) = self.sheets() else { return };
        let (dpi, paper) = (self.edit.dpi, self.settings.sheet.paper_mm());
        self.print_now(ctx, pages, dpi, paper);
    }

    /// Print straight to the default printer in the background (no PDF viewer). Non-Windows: opens a PDF.
    fn print_now(&mut self, ctx: &egui::Context, pages: Vec<RgbImage>, dpi: u32, paper: (f32, f32)) {
        if self.printing {
            self.status = "A print job is already being sent - please wait".into();
            return;
        }
        if !cfg!(windows) {
            let path = std::env::temp_dir().join("ripon_print.pdf");
            match std::fs::write(&path, sheet::encode_pdf(&pages, dpi, 95)).map_err(|e| e.to_string()).and_then(|_| open::that(&path).map_err(|e| e.to_string())) {
                Ok(()) => self.status = "Opened as PDF (direct printing works on Windows).".into(),
                Err(e) => self.error = Some(format!("Could not open the PDF: {e}")),
            }
            return;
        }
        self.printing = true;
        self.status = "Print window opened - choose the printer and press Print".into();
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = std::panic::catch_unwind(|| printer::print_pages(&pages, dpi, paper)).unwrap_or_else(|_| Err("Printing crashed unexpectedly".into()));
            let _ = tx.send(Msg::Printed(r));
            ctx.request_repaint();
        });
    }

    fn save_online(&mut self) {
        let Some(img) = self.final_photo() else { return };
        let name = format!("{}_online.jpg", self.base_name());
        let Some(path) = self.save_dialog(&name, ("JPEG", &["jpg", "jpeg"])) else { return };
        let s = &self.settings;
        let (bytes, q, fits) = sheet::jpeg_under_kb(&img, Some((s.online_w, s.online_h)), s.online_kb, 72);
        let kb = bytes.len() as f32 / 1024.0;
        match std::fs::write(&path, &bytes) {
            Ok(()) => {
                self.online_msg = if fits { format!("Saved {:.1} KB (quality {}) - under {} KB", kb, q, s.online_kb) } else { format!("Saved, but still {:.1} KB - make the pixel size smaller", kb) };
                self.status = format!("Saved {}", path.display());
            }
            Err(e) => self.status = format!("Could not save: {e}"),
        }
    }

    fn choose_dress(&mut self, sel: Option<usize>) {
        self.dress_sel = sel;
        self.edit.dress_id += 1;
        self.dress_img = sel.and_then(|i| image::open(&self.dresses[i].1).ok()).map(|im| Arc::new(im.to_rgba8()));
        if sel.is_some() && self.dress_img.is_none() {
            self.error = Some("Could not read that dress PNG.".into());
            self.dress_sel = None;
        }
    }
}

// ================================================================== helpers
fn load_image(path: &Path) -> Result<RgbImage, String> {
    let reader = image::ImageReader::open(path).map_err(|e| format!("Cannot open file: {e}"))?.with_guessed_format().map_err(|e| e.to_string())?;
    let mut dec = reader.into_decoder().map_err(|e| format!("Not a supported picture: {e}"))?;
    let orient = dec.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = image::DynamicImage::from_decoder(dec).map_err(|e| format!("Cannot read picture: {e}"))?;
    img.apply_orientation(orient);
    let rgb = img.to_rgb8();
    let m = rgb.width().max(rgb.height());
    if m > 6000 {
        let k = 6000.0 / m as f32;
        return Ok(image::imageops::resize(&rgb, (rgb.width() as f32 * k) as u32, (rgb.height() as f32 * k) as u32, image::imageops::FilterType::Triangle));
    }
    Ok(rgb)
}

fn dress_dirs() -> Vec<PathBuf> {
    let mut v = vec![seg::app_dir().join("dress")];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            v.push(d.join("dress"));
        }
    }
    v.push(PathBuf::from("dress"));
    v
}
fn find_dresses() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for d in dress_dirs() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let mut files = Vec::new();
            if p.is_dir() {
                if let Ok(rd2) = std::fs::read_dir(&p) {
                    files.extend(rd2.flatten().map(|e| e.path()));
                }
            } else {
                files.push(p.clone());
            }
            for f in files {
                if f.extension().map(|x| x.eq_ignore_ascii_case("png")).unwrap_or(false) {
                    let cat = f.parent().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    let nm = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    let label = if f.parent() == Some(d.as_path()) { nm } else { format!("{cat} / {nm}") };
                    out.push((label, f));
                }
            }
        }
    }
    out.sort();
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

fn fit_rect(area: Rect, w: f32, h: f32, pad: f32) -> Rect {
    let a = area.shrink(pad);
    let s = (a.width() / w).min(a.height() / h).max(0.01);
    Rect::from_center_size(a.center(), vec2(w * s, h * s))
}

fn cr(r: u8) -> CornerRadius {
    CornerRadius::same(r)
}

// ------------------------------------------------------------------ small widgets
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Primary,
    Accent,
    Ghost,
    Danger,
}
fn button(ui: &mut Ui, text: &str, kind: Kind, size: Vec2) -> egui::Response {
    let (fill, col, stroke) = match kind {
        Kind::Primary => (BLUE_DK, Color32::WHITE, Stroke::new(1.0_f32, BLUE)),
        Kind::Accent => (ORANGE, Color32::from_rgb(0x1a, 0x10, 0x00), Stroke::NONE),
        Kind::Ghost => (FIELD, TEXT, Stroke::new(1.0_f32, Color32::from_rgb(0x2a, 0x35, 0x4c))),
        Kind::Danger => (Color32::from_rgb(0x3a, 0x18, 0x20), RED, Stroke::new(1.0_f32, Color32::from_rgb(0x6a, 0x2a, 0x38))),
    };
    let b = egui::Button::new(RichText::new(text).font(brand::medium(13.5)).color(col)).fill(fill).stroke(stroke).corner_radius(cr(9)).min_size(size);
    ui.add(b).on_hover_cursor(egui::CursorIcon::PointingHand)
}
fn full_button(ui: &mut Ui, text: &str, kind: Kind, h: f32) -> egui::Response {
    let w = ui.available_width();
    button(ui, text, kind, vec2(w, h))
}
fn chip(ui: &mut Ui, text: &str, on: bool) -> egui::Response {
    let (fill, col, stroke) = if on { (BLUE_DK, Color32::WHITE, Stroke::new(1.0_f32, BLUE)) } else { (FIELD, MUTED, Stroke::new(1.0_f32, Color32::from_rgb(0x2a, 0x35, 0x4c))) };
    let b = egui::Button::new(RichText::new(text).font(brand::medium(12.5)).color(col)).fill(fill).stroke(stroke).corner_radius(cr(16)).min_size(vec2(0.0, 26.0));
    ui.add(b).on_hover_cursor(egui::CursorIcon::PointingHand)
}
fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(11.5).color(MUTED));
}
fn sub(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).font(brand::medium(12.0)).color(MUTED));
}

fn slider_row(ui: &mut Ui, label: &str, v: &mut f32, lo: f32, hi: f32, scale: f32, suffix: &str) -> egui::Response {
    ui.horizontal(|ui| {
        ui.add_sized([96.0, 20.0], egui::Label::new(RichText::new(label).size(12.5).color(MUTED)));
        let w = (ui.available_width() - 48.0).max(60.0);
        ui.spacing_mut().slider_width = w;
        let r = ui.add(egui::Slider::new(v, lo..=hi).show_value(false));
        ui.add_sized([42.0, 20.0], egui::Label::new(RichText::new(format!("{:.0}{}", *v * scale, suffix)).font(brand::medium(12.5)).color(BLUE)));
        r
    })
    .inner
}

fn card(ui: &mut Ui, num: &str, title: &str, default_open: bool, add: impl FnOnce(&mut Ui)) {
    let id = egui::Id::new(("card", title));
    let mut open: bool = ui.ctx().data_mut(|d| *d.get_temp_mut_or(id, default_open));
    egui::Frame::new().fill(CARD).stroke(Stroke::new(1.0_f32, BORDER)).corner_radius(cr(14)).inner_margin(egui::Margin::same(13)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let head = ui.horizontal(|ui| {
            if !num.is_empty() {
                let (r, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 11.0, Color32::from_rgb(0x1a, 0x2a, 0x44));
                ui.painter().circle_stroke(r.center(), 11.0, Stroke::new(1.0_f32, BLUE));
                ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, num, brand::semibold(11.5), BLUE);
            }
            ui.label(RichText::new(title).font(brand::semibold(13.5)).color(TEXT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                let c = r.center();
                let pts = if open { vec![c + vec2(-5.0, -2.5), c + vec2(5.0, -2.5), c + vec2(0.0, 3.5)] } else { vec![c + vec2(-2.5, -5.0), c + vec2(-2.5, 5.0), c + vec2(3.5, 0.0)] };
                ui.painter().add(egui::Shape::convex_polygon(pts, MUTED, Stroke::NONE));
            });
        });
        let click = ui.interact(head.response.rect, id.with("hdr"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        if click.clicked() {
            open = !open;
            ui.ctx().data_mut(|d| d.insert_temp(id, open));
        }
        if open {
            ui.add_space(8.0);
            add(ui);
        }
    });
    ui.add_space(10.0);
}

fn gradient_bar(painter: &egui::Painter, rect: Rect, c0: Color32, c1: Color32) {
    let mut m = egui::Mesh::default();
    m.colored_vertex(rect.left_top(), c0);
    m.colored_vertex(rect.right_top(), c1);
    m.colored_vertex(rect.right_bottom(), c1);
    m.colored_vertex(rect.left_bottom(), c0);
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(m));
}

// ================================================================== UI
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll(ctx);
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if self.view == View::Batch && !dropped.is_empty() {
            self.batch_import(ctx, dropped);
        } else if let Some(p) = dropped.into_iter().next() {
            if self.view == View::Doc {
                self.doc_drop(ctx, p);
            } else {
                self.open_path(ctx, p);
            }
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::O)) {
            self.pick_open(ctx);
        }
        if !ctx.wants_keyboard_input() {
            if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Z)) {
                if ctx.input(|i| i.modifiers.shift) { self.redo(); } else { self.undo(); }
            }
            if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Y)) {
                self.redo();
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.tool = Tool::None;
                self.pen.clear();
            }
        }
        // the crop frame follows the aspect ratio of the chosen size / duet
        let asp = self.edit.aspect();
        if (asp - self.last_aspect).abs() > 2e-3 {
            if self.edit.duet && self.edit.duet_free {
                // photo resized by hand: keep what is visible vertically, change the width
                let ch = self.edit.cw / self.last_aspect.max(0.01);
                self.edit.cw = ch * asp;
            } else {
                self.after_size_change();
            }
            self.last_aspect = asp;
        }
        self.hist_tick(ctx);
        self.submit_render();
        self.batch_tick(ctx);
        if self.view == View::Doc {
            self.doc_refresh(ctx);
        }

        egui::TopBottomPanel::top("brand").exact_height(74.0).frame(egui::Frame::default().fill(brand::BG).inner_margin(egui::Margin::symmetric(18, 10))).show(ctx, |ui| self.header(ui, ctx));
        egui::TopBottomPanel::bottom("status").exact_height(30.0).frame(egui::Frame::default().fill(brand::BG).inner_margin(egui::Margin::symmetric(18, 5))).show(ctx, |ui| {
            ui.horizontal(|ui| {
                if self.seg_busy || self.dl.is_some() {
                    ui.spinner();
                } else {
                    let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 3.5, GREEN);
                }
                ui.label(RichText::new(&self.status).size(12.0).color(MUTED));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("{}   v{}", brand::TITLE, brand::VERSION)).size(11.0).color(MUTED));
                });
            });
        });
        egui::SidePanel::left("tools").exact_width(372.0).resizable(false).frame(egui::Frame::default().fill(brand::PANEL).inner_margin(egui::Margin { left: 12, right: 6, top: 12, bottom: 0 })).show(ctx, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.set_max_width(ui.available_width() - 8.0);
                match self.view { View::Doc => self.doc_left(ui, ctx), View::Batch => self.batch_left(ui, ctx), _ => self.left_panel(ui, ctx) }
            });
        });
        egui::SidePanel::right("output").exact_width(318.0).resizable(false).frame(egui::Frame::default().fill(brand::PANEL).inner_margin(egui::Margin { left: 6, right: 12, top: 12, bottom: 0 })).show(ctx, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.set_max_width(ui.available_width() - 8.0);
                match self.view { View::Doc => self.doc_right(ui, ctx), View::Batch => self.batch_right(ui, ctx), _ => self.right_panel(ui, ctx) }
            });
        });
        egui::CentralPanel::default().frame(egui::Frame::default().fill(brand::BG).inner_margin(egui::Margin::same(12))).show(ctx, |ui| {
            ui.horizontal(|ui| {
                egui::Frame::new().fill(Color32::from_rgb(0x10, 0x16, 0x23)).corner_radius(cr(12)).inner_margin(egui::Margin::same(3)).stroke(Stroke::new(1.0_f32, BORDER)).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for (v, name) in [(View::Edit, "Edit photo"), (View::Sheet, "Print sheet"), (View::Doc, "Document / NID"), (View::Batch, "Batch print")] {
                        let on = self.view == v;
                        let b = egui::Button::new(RichText::new(name).font(brand::medium(13.5)).color(if on { Color32::WHITE } else { MUTED })).fill(if on { BLUE_DK } else { Color32::TRANSPARENT }).stroke(Stroke::NONE).corner_radius(cr(9)).min_size(vec2(118.0, 28.0));
                        if ui.add(b).clicked() {
                            if self.batch.editing.is_some() && v != View::Edit {
                                self.batch_commit_edit();
                            }
                            self.view = v;
                        }
                    }
                });
                ui.add_space(8.0);
                if self.batch.editing.is_some() && self.view == View::Edit && button(ui, "Done - back to batch", Kind::Accent, vec2(170.0, 30.0)).clicked() {
                    self.batch_commit_edit();
                }
                if self.tool != Tool::None && self.view == View::Edit {
                    let (txt, col) = match self.tool {
                        Tool::Restore => ("RESTORE brush ON", RED),
                        Tool::Erase => ("ERASE brush ON", BLUE),
                        Tool::Pen => ("PEN tool ON", GREEN),
                        Tool::Lasso => ("LASSO tool ON", GREEN),
                        _ => ("SPOT HEAL ON", ORANGE),
                    };
                    ui.label(RichText::new(txt).font(brand::semibold(12.5)).color(col));
                }
            });
            ui.add_space(8.0);
            match self.view {
                View::Edit => self.edit_canvas(ui, ctx),
                View::Sheet => self.sheet_canvas(ui, ctx),
                View::Doc => self.doc_canvas(ui, ctx),
                View::Batch => self.batch_canvas(ui, ctx),
            }
        });

        self.models_window(ctx);
        if let Some(e) = self.error.clone() {
            egui::Window::new(RichText::new("Message").font(brand::semibold(15.0)).color(ORANGE)).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                ui.set_max_width(430.0);
                ui.label(e);
                ui.add_space(8.0);
                if button(ui, "OK", Kind::Primary, vec2(90.0, 30.0)).clicked() {
                    self.error = None;
                }
            });
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            self.save_settings();
        }
    }
}

impl App {
    fn header(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let full = ui.max_rect();
        ui.horizontal(|ui| {
            ui.image((self.logo.id(), vec2(54.0, 54.0)));
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.add_space(4.0);
                ui.label(brand::gradient_text(brand::NAME, 25.0));
                ui.label(RichText::new(brand::TAGLINE).font(brand::medium(11.0)).color(MUTED));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if button(ui, "Open photo   Ctrl+O", Kind::Accent, vec2(180.0, 38.0)).clicked() {
                    self.pick_open(ctx);
                }
                if button(ui, "AI Models", Kind::Primary, vec2(110.0, 38.0)).on_hover_text("Download, edit links, backup and restore AI models").clicked() {
                    self.mui.open = true;
                    self.refresh_models();
                }
            });
        });
        let bar = Rect::from_min_max(pos2(full.min.x - 18.0, full.max.y + 8.0), pos2(full.max.x + 18.0, full.max.y + 10.0));
        gradient_bar(ui.painter(), bar, BLUE, ORANGE);
    }

    fn after_size_change(&mut self) {
        if let Some(s) = &self.src {
            let (w, h) = s.dimensions();
            if let Some(a) = self.alpha.clone() {
                self.edit.auto_crop(&a, w, h);
            } else {
                self.edit.reset_crop(w, h);
            }
        }
    }

    fn auto_crop_now(&mut self, ctx: &egui::Context) {
        let Some(s) = self.src.clone() else { return };
        match self.alpha.clone() {
            Some(a) => {
                if !self.edit.auto_crop(&a, s.width(), s.height()) {
                    self.error = Some("No person found in the picture. Move the frame by hand.".into());
                }
            }
            None => {
                if seg::is_installed(&self.settings.model_key) {
                    self.start_seg(ctx, true);
                } else {
                    self.error = Some("Auto head crop needs the AI model (section 2). Download it once, then it works offline.".into());
                }
            }
        }
    }

    fn left_panel(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let has = self.src.is_some();

        card(ui, "1", "Size & crop", true, |ui| {
            ui.horizontal(|ui| {
                if chip(ui, "Single photo", !self.edit.duet).clicked() {
                    self.edit.duet = false;
                }
                if chip(ui, "Duet  (2 photos in one frame)", self.edit.duet).clicked() {
                    self.edit.duet = true;
                    self.view = View::Edit;
                }
            });
            ui.add_space(2.0);
            if !self.edit.duet {
                egui::ComboBox::from_id_salt("size").width(ui.available_width()).selected_text(PRESETS[self.edit.size_idx].name).show_ui(ui, |ui| {
                    for (i, p) in PRESETS.iter().enumerate() {
                        ui.selectable_value(&mut self.edit.size_idx, i, p.name);
                    }
                });
                if self.edit.size_idx == CUSTOM_IDX {
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut self.edit.custom_w_mm).range(10.0..=300.0).suffix(" mm").speed(0.5));
                        ui.label("x");
                        ui.add(egui::DragValue::new(&mut self.edit.custom_h_mm).range(10.0..=300.0).suffix(" mm").speed(0.5));
                    });
                }
            } else {
                sub(ui, "Whole duet frame (both photos together)");
                ui.horizontal(|ui| {
                    let (mut wi, mut hi) = (self.edit.duet_w_mm / 25.4, self.edit.duet_h_mm / 25.4);
                    let a = ui.add(egui::DragValue::new(&mut wi).range(0.6..=12.0).speed(0.01).fixed_decimals(2).suffix(" in"));
                    ui.label("x");
                    let b = ui.add(egui::DragValue::new(&mut hi).range(0.6..=12.0).speed(0.01).fixed_decimals(2).suffix(" in"));
                    if a.changed() {
                        self.edit.duet_w_mm = wi * 25.4;
                    }
                    if b.changed() {
                        self.edit.duet_h_mm = hi * 25.4;
                    }
                    if ui.small_button("Bank 1.9 x 1.5").clicked() {
                        self.edit.duet_w_mm = 48.26;
                        self.edit.duet_h_mm = 38.1;
                    }
                });
                ui.horizontal(|ui| {
                    sub(ui, "Gap between photos");
                    if ui.add(egui::DragValue::new(&mut self.edit.duet_between_mm).range(0.0..=20.0).speed(0.1).fixed_decimals(1).suffix(" mm")).changed() && self.edit.duet_free {
                        self.duet_tidy();
                    }
                });
                ui.horizontal(|ui| {
                    sub(ui, "Arrange");
                    if chip(ui, "Side by side", !self.edit.duet_stack).clicked() {
                        self.edit.duet_stack = false;
                        self.edit.duet_free = false;
                    }
                    if chip(ui, "Top / bottom", self.edit.duet_stack).clicked() {
                        self.edit.duet_stack = true;
                        self.edit.duet_free = false;
                    }
                });
                let (pw, ph) = self.edit.photo_px();
                let (fw, fh) = self.edit.duet_px();
                hint(ui, &format!("Frame {} x {} px  -  each photo {} x {} px", fw, fh, pw, ph));
                ui.add_space(2.0);
                self.duet_panel(ui, ctx);
            }
            ui.horizontal(|ui| {
                sub(ui, "Quality");
                for d in [200u32, 300, 600] {
                    if chip(ui, &format!("{d} dpi"), self.edit.dpi == d).clicked() {
                        self.edit.dpi = d;
                    }
                }
            });
            if !self.edit.duet {
                let (pw, ph) = self.edit.photo_px();
                hint(ui, &format!("Output  {} x {} px", pw, ph));
            }
            ui.add_enabled_ui(has, |ui| {
                if full_button(ui, "Auto head crop", Kind::Primary, 34.0).on_hover_text("Places the head at the right size and position").clicked() {
                    self.auto_crop_now(ctx);
                }
                ui.horizontal(|ui| {
                    if button(ui, "Reset", Kind::Ghost, vec2(60.0, 26.0)).clicked() {
                        if let Some(s) = &self.src {
                            let (w, h) = s.dimensions();
                            self.edit.reset_crop(w, h);
                        }
                    }
                    sub(ui, "Zoom");
                    if button(ui, "-", Kind::Ghost, vec2(34.0, 26.0)).clicked() {
                        self.edit.cw *= 1.05;
                    }
                    if button(ui, "+", Kind::Ghost, vec2(34.0, 26.0)).clicked() {
                        self.edit.cw /= 1.05;
                    }
                });
                ui.horizontal(|ui| {
                    let step = self.edit.cw * 0.01;
                    sub(ui, "Move");
                    if button(ui, "Left", Kind::Ghost, vec2(0.0, 26.0)).clicked() { self.edit.cx -= step; }
                    if button(ui, "Right", Kind::Ghost, vec2(0.0, 26.0)).clicked() { self.edit.cx += step; }
                    if button(ui, "Up", Kind::Ghost, vec2(0.0, 26.0)).clicked() { self.edit.cy -= step; }
                    if button(ui, "Down", Kind::Ghost, vec2(0.0, 26.0)).clicked() { self.edit.cy += step; }
                });
                ui.horizontal_wrapped(|ui| {
                    sub(ui, "Rotate");
                    if button(ui, "Left 90\u{00B0}", Kind::Ghost, vec2(0.0, 26.0)).clicked() { self.edit.rotate_quarter(-1); }
                    if button(ui, "Right 90\u{00B0}", Kind::Ghost, vec2(0.0, 26.0)).clicked() { self.edit.rotate_quarter(1); }
                });
                ui.horizontal_wrapped(|ui| {
                    sub(ui, "Flip");
                    if chip(ui, "Left-right", self.edit.flip_h).clicked() { self.edit.flip_h ^= true; }
                    if chip(ui, "Up-down", self.edit.flip_v).clicked() { self.edit.flip_v ^= true; }
                });
                // free rotate: whole degrees; slider, - / + buttons, typing, mouse wheel or arrow keys (click the slider first)
                let mut focused = false;
                let row = ui.horizontal(|ui| {
                    ui.add_sized([70.0, 20.0], egui::Label::new(RichText::new("Free rotate").size(12.5).color(MUTED)));
                    if ui.small_button("-").on_hover_text("1 degree left").clicked() {
                        self.edit.angle -= 1.0;
                    }
                    ui.spacing_mut().slider_width = (ui.available_width() - 105.0).max(50.0);
                    let sl = ui.add(egui::Slider::new(&mut self.edit.angle, -180.0..=180.0).step_by(1.0).show_value(false));
                    focused = sl.has_focus();
                    if ui.small_button("+").on_hover_text("1 degree right").clicked() {
                        self.edit.angle += 1.0;
                    }
                    ui.add(egui::DragValue::new(&mut self.edit.angle).range(-180.0..=180.0).speed(0.2).fixed_decimals(0).suffix("\u{00B0}"));
                });
                if row.response.hovered() || focused {
                    let dir = if focused { 0 } else { ui.input(|i| (i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::ArrowUp)) as i32 - (i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::ArrowDown)) as i32) };
                    self.edit.angle += dir as f32;
                    let sd = ui.input(|i| i.raw_scroll_delta.y);
                    if sd != 0.0 {
                        self.edit.angle += sd.signum();
                    }
                }
                self.edit.angle = self.edit.angle.round().clamp(-180.0, 180.0);
            });
            ui.checkbox(&mut self.show_guides, "Show head guides");
            hint(ui, "Drag the orange frame to move, drag a corner to resize, mouse wheel = zoom.");
        });

        card(ui, "2", "Background", true, |ui| {
            let key = self.settings.model_key.clone();
            let installed = seg::is_installed(&key);
            egui::ComboBox::from_id_salt("model").width(ui.available_width()).selected_text(seg::model_by_key(&key).label).show_ui(ui, |ui| {
                for m in seg::MODELS {
                    ui.selectable_value(&mut self.settings.model_key, m.key.to_string(), m.label);
                }
            });
            if self.settings.model_key != key {
                seg::warm_up(&self.settings.model_key);
            }
            if let Some((d, t)) = self.dl {
                ui.add(egui::ProgressBar::new(d as f32 / t as f32).corner_radius(cr(8)).text(format!("Downloading AI model  {:.0} / {:.0} MB", d as f32 / 1e6, t as f32 / 1e6)));
                if button(ui, "Cancel", Kind::Danger, vec2(80.0, 26.0)).clicked() {
                    self.cancel.store(true, Ordering::Relaxed);
                }
            } else if !installed {
                hint(ui, "The AI model is not on this PC yet. One-time download, then it works offline.");
                if full_button(ui, &format!("Download AI model  ({} MB)", seg::model_by_key(&key).size_mb), Kind::Accent, 36.0).clicked() {
                    self.start_download(ctx);
                }
            } else {
                ui.add_enabled_ui(has && !self.seg_busy, |ui| {
                    ui.horizontal(|ui| {
                        if button(ui, "Remove background", Kind::Primary, vec2(190.0, 34.0)).clicked() {
                            self.start_seg(ctx, false);
                        }
                        ui.add_enabled_ui(self.alpha.is_some(), |ui| {
                            if button(ui, "Keep original", Kind::Ghost, vec2(0.0, 34.0)).clicked() {
                                self.alpha = None;
                                self.alpha_ver += 1;
                                self.edit.strokes.clear();
                                self.edit.cuts.clear();
                            }
                        });
                    });
                });
            }
            ui.checkbox(&mut self.settings.auto_bg, "Auto remove background + head crop on open");
            ui.add_space(2.0);
            sub(ui, "Background colour");
            ui.horizontal_wrapped(|ui| {
                for (name, c) in [("White", [255u8, 255, 255]), ("Light blue", [173, 216, 255]), ("Sky blue", [102, 178, 255]), ("Red", [200, 30, 40]), ("Grey", [200, 200, 200]), ("Cream", [250, 243, 224])] {
                    let col = Color32::from_rgb(c[0], c[1], c[2]);
                    let sel = self.edit.bg == c;
                    let (r, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
                    ui.painter().circle_filled(r.center(), 12.0, col);
                    ui.painter().circle_stroke(r.center(), if sel { 13.5 } else { 12.0 }, Stroke::new(if sel { 2.5_f32 } else { 1.0_f32 }, if sel { ORANGE } else { Color32::from_gray(80) }));
                    if resp.on_hover_text(name).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        self.edit.bg = c;
                    }
                }
                ui.color_edit_button_srgb(&mut self.edit.bg);
            });
            slider_row(ui, "Edge smooth", &mut self.edit.edge_refine, 0.0, 1.0, 100.0, "");
            slider_row(ui, "Edge cut", &mut self.edit.edge_cut, 0.0, 0.4, 100.0, "");

            ui.add_space(4.0);
            ui.separator();
            ui.add_space(2.0);
            sub(ui, "Repair brush  (paint on the RESULT picture)");
            ui.horizontal(|ui| {
                let on_r = self.tool == Tool::Restore;
                let on_e = self.tool == Tool::Erase;
                ui.add_enabled_ui(has, |ui| {
                    let r = egui::Button::new(RichText::new("Restore  (add person)").font(brand::medium(12.5)).color(if on_r { Color32::WHITE } else { RED })).fill(if on_r { Color32::from_rgb(0xa8, 0x2c, 0x3a) } else { Color32::from_rgb(0x3a, 0x18, 0x20) }).stroke(Stroke::new(1.0_f32, RED)).corner_radius(cr(9)).min_size(vec2(0.0, 30.0));
                    if ui.add(r).clicked() {
                        self.tool = if on_r { Tool::None } else { Tool::Restore };
                        self.view = View::Edit;
                    }
                    let e = egui::Button::new(RichText::new("Erase  (remove bg)").font(brand::medium(12.5)).color(if on_e { Color32::WHITE } else { BLUE })).fill(if on_e { BLUE_DK } else { Color32::from_rgb(0x12, 0x23, 0x3a) }).stroke(Stroke::new(1.0_f32, BLUE)).corner_radius(cr(9)).min_size(vec2(0.0, 30.0));
                    if ui.add(e).clicked() {
                        self.tool = if on_e { Tool::None } else { Tool::Erase };
                        self.view = View::Edit;
                    }
                });
            });
            slider_row(ui, "Brush size", &mut self.brush_px, 8.0, 140.0, 1.0, "");
            let mut soft = 1.0 - self.brush_hard;
            if slider_row(ui, "Softness", &mut soft, 0.0, 1.0, 100.0, "").on_hover_text("Left = hard, sharp edge.  Right = soft, feathered edge (the thin inner ring shows the full-strength part).").changed() {
                self.brush_hard = 1.0 - soft;
            }
            slider_row(ui, "Strength", &mut self.brush_strength, 0.05, 1.0, 100.0, "").on_hover_text("Low strength removes / restores only a little with each stroke - go over the same place again for more.");
            ui.horizontal_wrapped(|ui| {
                sub(ui, "Presets");
                if ui.small_button("Hard edge").clicked() {
                    self.brush_hard = 1.0;
                    self.brush_strength = 1.0;
                }
                if ui.small_button("Soft edge").clicked() {
                    self.brush_hard = 0.3;
                    self.brush_strength = 1.0;
                }
                if ui.small_button("Gentle").clicked() {
                    self.brush_hard = 0.2;
                    self.brush_strength = 0.35;
                }
            });
            ui.horizontal(|ui| {
                if ui.add_enabled(!self.edit.strokes.is_empty(), egui::Button::new("Undo stroke")).clicked() {
                    self.edit.strokes.pop();
                }
                if ui.add_enabled(!self.edit.strokes.is_empty(), egui::Button::new(format!("Clear brush ({})", self.edit.strokes.len()))).clicked() {
                    self.edit.strokes.clear();
                }
            });
            hint(ui, "Works even without the AI model: Erase cuts the background by hand.");

            ui.add_space(4.0);
            ui.separator();
            ui.add_space(2.0);
            sub(ui, "Cut shapes  (Pen = click points, Lasso = drag)");
            ui.horizontal(|ui| {
                ui.add_enabled_ui(has, |ui| {
                    let on_p = self.tool == Tool::Pen;
                    let on_l = self.tool == Tool::Lasso;
                    if ui.add(egui::Button::new(RichText::new("Pen tool").color(if on_p { Color32::WHITE } else { GREEN })).fill(if on_p { Color32::from_rgb(0x1f, 0x6b, 0x3c) } else { Color32::from_rgb(0x10, 0x2a, 0x1c) }).stroke(Stroke::new(1.0_f32, GREEN)).corner_radius(cr(9)).min_size(vec2(0.0, 30.0))).on_hover_text("Click around the area one point at a time. Click the first (orange) point, double-click or press Enter to close. Backspace = remove last point.").clicked() {
                        self.tool = if on_p { Tool::None } else { Tool::Pen };
                        self.pen.clear();
                        self.view = View::Edit;
                    }
                    if ui.add(egui::Button::new(RichText::new("Lasso").color(if on_l { Color32::WHITE } else { GREEN })).fill(if on_l { Color32::from_rgb(0x1f, 0x6b, 0x3c) } else { Color32::from_rgb(0x10, 0x2a, 0x1c) }).stroke(Stroke::new(1.0_f32, GREEN)).corner_radius(cr(9)).min_size(vec2(0.0, 30.0))).on_hover_text("Hold the mouse button and draw around the area, release to close the shape.").clicked() {
                        self.tool = if on_l { Tool::None } else { Tool::Lasso };
                        self.pen.clear();
                        self.view = View::Edit;
                    }
                });
            });
            ui.horizontal_wrapped(|ui| {
                if chip(ui, "Remove inside", self.cut_mode == CutMode::Erase).clicked() {
                    self.cut_mode = CutMode::Erase;
                }
                if chip(ui, "Restore inside", self.cut_mode == CutMode::Restore).clicked() {
                    self.cut_mode = CutMode::Restore;
                }
                if chip(ui, "Keep only inside", self.cut_mode == CutMode::KeepInside).clicked() {
                    self.cut_mode = CutMode::KeepInside;
                }
            });
            slider_row(ui, "Edge feather", &mut self.cut_feather, 0.0, 40.0, 1.0, "").on_hover_text("0 = sharp cut. Higher = soft, blended edge.");
            ui.horizontal(|ui| {
                if ui.add_enabled(self.pen.len() >= 3 && self.tool == Tool::Pen, egui::Button::new("Finish shape")).clicked() {
                    let pw = self.edit.photo_px().0 as f32;
                    self.finish_cut(pw);
                }
                if ui.add_enabled(!self.pen.is_empty(), egui::Button::new("Cancel shape")).clicked() {
                    self.pen.clear();
                }
            });
            ui.horizontal(|ui| {
                if ui.add_enabled(!self.edit.cuts.is_empty(), egui::Button::new("Undo shape")).clicked() {
                    self.edit.cuts.pop();
                }
                if ui.add_enabled(!self.edit.cuts.is_empty(), egui::Button::new(format!("Clear shapes ({})", self.edit.cuts.len()))).clicked() {
                    self.edit.cuts.clear();
                }
            });
        });

        card(ui, "3", "Enhance", true, |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.edit.auto_enhance, "Auto colour & light");
            });
            ui.add_enabled_ui(self.edit.auto_enhance, |ui| {
                slider_row(ui, "Strength", &mut self.edit.enhance_strength, 0.0, 1.0, 100.0, "");
            });
            slider_row(ui, "Brightness", &mut self.edit.brightness, -1.0, 1.0, 100.0, "");
            slider_row(ui, "Contrast", &mut self.edit.contrast, -0.6, 0.8, 100.0, "");
            slider_row(ui, "Colour", &mut self.edit.saturation, -1.0, 1.0, 100.0, "");
            slider_row(ui, "Lift shadows", &mut self.edit.shadows, 0.0, 1.0, 100.0, "");
            slider_row(ui, "Sharpen", &mut self.edit.sharpen, 0.0, 1.0, 100.0, "");
            slider_row(ui, "Reduce noise", &mut self.edit.denoise, 0.0, 1.0, 100.0, "");
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.edit.bw, "Black & white");
                if button(ui, "Reset adjustments", Kind::Ghost, vec2(0.0, 26.0)).clicked() {
                    let d = Edit::default();
                    self.edit.auto_enhance = d.auto_enhance;
                    self.edit.enhance_strength = d.enhance_strength;
                    self.edit.brightness = 0.0;
                    self.edit.contrast = 0.0;
                    self.edit.saturation = 0.0;
                    self.edit.shadows = 0.0;
                    self.edit.sharpen = d.sharpen;
                    self.edit.denoise = 0.0;
                    self.edit.bw = false;
                }
            });
        });

        self.ai_card(ui, ctx);

        card(ui, "4", "Retouch", true, |ui| {
            slider_row(ui, "Skin smooth", &mut self.edit.skin, 0.0, 1.0, 100.0, "");
            ui.horizontal(|ui| {
                let on = self.tool == Tool::Heal;
                if chip(ui, if on { "Spot heal: ON" } else { "Spot heal tool" }, on).clicked() {
                    self.tool = if on { Tool::None } else { Tool::Heal };
                    self.view = View::Edit;
                }
                if ui.add_enabled(!self.edit.spots.is_empty(), egui::Button::new("Undo")).clicked() {
                    self.edit.spots.pop();
                }
                if ui.add_enabled(!self.edit.spots.is_empty(), egui::Button::new(format!("Clear ({})", self.edit.spots.len()))).clicked() {
                    self.edit.spots.clear();
                }
            });
            if self.tool == Tool::Heal {
                slider_row(ui, "Brush size", &mut self.heal_r, 0.006, 0.06, 100.0, "");
            }
        });

        card(ui, "5", "Dress / suit", false, |ui| {
            if self.dresses.is_empty() {
                hint(ui, "No dress PNGs yet. Put transparent PNG files (shirt / suit / tie) in the 'dress' folder, then press Refresh.");
            } else {
                let cur = self.dress_sel.map(|i| self.dresses[i].0.clone()).unwrap_or_else(|| "None".into());
                let mut pick = self.dress_sel;
                egui::ComboBox::from_id_salt("dress").width(ui.available_width()).selected_text(cur).show_ui(ui, |ui| {
                    ui.selectable_value(&mut pick, None, "None");
                    for (i, (n, _)) in self.dresses.iter().enumerate() {
                        ui.selectable_value(&mut pick, Some(i), n);
                    }
                });
                if pick != self.dress_sel {
                    self.choose_dress(pick);
                }
                if self.dress_sel.is_some() {
                    slider_row(ui, "Size", &mut self.edit.dress_scale, 0.4, 2.0, 100.0, "");
                    slider_row(ui, "Up / down", &mut self.edit.dress_y, -0.5, 0.5, 100.0, "");
                    slider_row(ui, "Left / right", &mut self.edit.dress_x, -0.5, 0.5, 100.0, "");
                }
            }
            ui.horizontal(|ui| {
                if button(ui, "Refresh", Kind::Ghost, vec2(0.0, 26.0)).clicked() {
                    self.dresses = find_dresses();
                    self.choose_dress(None);
                }
                if button(ui, "Open dress folder", Kind::Ghost, vec2(0.0, 26.0)).clicked() {
                    let d = seg::app_dir().join("dress");
                    let _ = std::fs::create_dir_all(&d);
                    let _ = open::that(d);
                }
            });
        });

        card(ui, "6", "Name & date under photo", false, |ui| {
            if self.edit.duet {
                hint(ui, "Not used in Duet mode.");
            }
            ui.add_enabled_ui(!self.edit.duet, |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.edit.caption).hint_text("Customer name").desired_width(ui.available_width()));
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut self.add_date, "Date").changed() {
                        self.edit.caption_date = if self.add_date { chrono::Local::now().format("%d-%m-%Y").to_string() } else { String::new() };
                    }
                    if self.add_date {
                        ui.add(egui::TextEdit::singleline(&mut self.edit.caption_date).desired_width(120.0));
                    }
                });
            });
        });
        ui.add_space(30.0);
    }

    fn right_panel(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let has = self.src.is_some();
        card(ui, "", "Print sheet", true, |ui| {
            let s = &mut self.settings.sheet;
            egui::ComboBox::from_id_salt("paper").width(ui.available_width()).selected_text(sheet::PAPERS[s.paper_idx].name).show_ui(ui, |ui| {
                for (i, p) in sheet::PAPERS.iter().enumerate() {
                    ui.selectable_value(&mut s.paper_idx, i, p.name);
                }
            });
            if s.paper_idx == sheet::CUSTOM_PAPER {
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut s.custom_w_mm).range(40.0..=1000.0).suffix(" mm"));
                    ui.label("x");
                    ui.add(egui::DragValue::new(&mut s.custom_h_mm).range(40.0..=1000.0).suffix(" mm"));
                });
            }
            let unit_name = if self.edit.duet { "Duets" } else { "Copies" };
            ui.horizontal(|ui| {
                sub(ui, unit_name);
                ui.add_enabled(!s.fill_page, egui::DragValue::new(&mut s.copies).range(1..=500));
                ui.checkbox(&mut s.fill_page, "Fill the page");
            });
            ui.horizontal(|ui| {
                sub(ui, "Gap");
                ui.add(egui::DragValue::new(&mut s.gap_mm).range(0.0..=20.0).speed(0.2).suffix(" mm"));
                sub(ui, "Margin");
                ui.add(egui::DragValue::new(&mut s.margin_mm).range(0.0..=30.0).speed(0.2).suffix(" mm"));
            });
            ui.horizontal(|ui| {
                sub(ui, "Lines");
                egui::ComboBox::from_id_salt("border").width(100.0).selected_text(match s.border { Border::None => "None", Border::Thin => "Thin border", Border::CutMarks => "Cut marks" }).show_ui(ui, |ui| {
                    ui.selectable_value(&mut s.border, Border::None, "None");
                    ui.selectable_value(&mut s.border, Border::Thin, "Thin border");
                    ui.selectable_value(&mut s.border, Border::CutMarks, "Cut marks");
                });
                egui::ComboBox::from_id_salt("align").width(86.0).selected_text(match s.align { Align::TopLeft => "Top-left", Align::Center => "Centre" }).show_ui(ui, |ui| {
                    ui.selectable_value(&mut s.align, Align::TopLeft, "Top-left");
                    ui.selectable_value(&mut s.align, Align::Center, "Centre");
                });
            });
            ui.checkbox(&mut s.auto_rotate, "Rotate to fit more");
            ui.checkbox(&mut s.studio_footer, "Studio name at sheet bottom");
            if has {
                let (pw, ph) = self.unit.as_ref().map(|p| p.dimensions()).unwrap_or_else(|| self.edit.photo_px());
                let (l, copies, pages) = sheet::plan(pw, ph, &self.settings.sheet, self.edit.dpi);
                let t = if l.per_page() == 0 { "Too big for this paper".to_string() } else { format!("{} x {} = {} per sheet   -   {} on {} sheet(s)", l.cols, l.rows, l.per_page(), copies, pages) };
                ui.add_space(2.0);
                egui::Frame::new().fill(Color32::from_rgb(0x2a, 0x1d, 0x08)).corner_radius(cr(8)).inner_margin(egui::Margin::symmetric(9, 5)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(t).font(brand::medium(12.0)).color(ORANGE));
                });
            }
        });

        ui.add_enabled_ui(has, |ui| {
            card(ui, "", "Save & print", true, |ui| {
                if full_button(ui, "Print sheet", Kind::Accent, 40.0).on_hover_text("Opens the Windows print window (choose printer / copies), prints at real size").clicked() {
                    self.print_sheet(ctx);
                }
                sub(ui, "Save the whole sheet as");
                let w = (ui.available_width() - 16.0) / 3.0;
                ui.horizontal(|ui| {
                    if button(ui, "PDF", Kind::Primary, vec2(w, 32.0)).clicked() { self.save_sheet("pdf"); }
                    if button(ui, "JPG", Kind::Primary, vec2(w, 32.0)).clicked() { self.save_sheet("jpg"); }
                    if button(ui, "PNG", Kind::Primary, vec2(w, 32.0)).clicked() { self.save_sheet("png"); }
                });
                sub(ui, "Save one photo");
                let w2 = (ui.available_width() - 8.0) / 2.0;
                ui.horizontal(|ui| {
                    let nm = if self.edit.duet { "Duet" } else { "Photo" };
                    if button(ui, &format!("{nm} JPG"), Kind::Ghost, vec2(w2 - 2.0, 32.0)).clicked() { self.save_photo(false); }
                    if button(ui, &format!("{nm} PNG"), Kind::Ghost, vec2(w2 - 2.0, 32.0)).clicked() { self.save_photo(true); }
                });
                let mut q = self.settings.jpeg_q as f32;
                if slider_row(ui, "JPG quality", &mut q, 60.0, 100.0, 1.0, "").changed() {
                    self.settings.jpeg_q = q as u8;
                }
            });
            card(ui, "", "For online forms  (KB limit)", true, |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut self.settings.online_w).range(50..=4000).suffix(" px"));
                    ui.label("x");
                    ui.add(egui::DragValue::new(&mut self.settings.online_h).range(50..=4000).suffix(" px"));
                });
                ui.horizontal(|ui| {
                    sub(ui, "Max size");
                    ui.add(egui::DragValue::new(&mut self.settings.online_kb).range(5..=2000).suffix(" KB"));
                });
                if full_button(ui, "Save JPG under limit", Kind::Primary, 34.0).clicked() {
                    self.save_online();
                }
                if !self.online_msg.is_empty() {
                    ui.label(RichText::new(&self.online_msg).size(11.5).color(GREEN));
                }
            });
        });
        ui.add_space(20.0);
    }

    // ------------------------------------------------------------ canvases
    fn panel_bg(&self, painter: &egui::Painter, area: Rect, title: &str) {
        painter.rect_filled(area, cr(14), Color32::from_rgb(0x0b, 0x0f, 0x18));
        painter.rect_stroke(area, cr(14), Stroke::new(1.0_f32, BORDER), StrokeKind::Inside);
        painter.text(area.min + vec2(14.0, 11.0), egui::Align2::LEFT_TOP, title, brand::semibold(11.5), MUTED);
    }

    fn edit_canvas(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let area = ui.available_rect_before_wrap();
        if self.src.is_none() {
            self.welcome(ui, area, ctx);
            return;
        }
        let split = area.min.x + area.width() * 0.50;
        let left = Rect::from_min_max(area.min, pos2(split - 5.0, area.max.y));
        let right = Rect::from_min_max(pos2(split + 5.0, area.min.y), area.max);
        self.source_view(ui, left);
        self.result_view(ui, right);
    }

    fn welcome(&mut self, ui: &mut Ui, area: Rect, ctx: &egui::Context) {
        let painter = ui.painter_at(area);
        painter.rect_filled(area, cr(16), Color32::from_rgb(0x0b, 0x0f, 0x18));
        painter.rect_stroke(area, cr(16), Stroke::new(1.0_f32, BORDER), StrokeKind::Inside);
        let c = area.center();
        let r = Rect::from_center_size(c - vec2(0.0, 70.0), vec2(250.0, 250.0));
        painter.image(self.logo.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        painter.text(c + vec2(0.0, 82.0), egui::Align2::CENTER_CENTER, "Drop a photo here", brand::semibold(24.0), TEXT);
        painter.text(c + vec2(0.0, 112.0), egui::Align2::CENTER_CENTER, "JPG, PNG, WEBP, BMP  -  or press Ctrl+O", egui::FontId::proportional(13.0), MUTED);
        let b = Rect::from_center_size(c + vec2(0.0, 160.0), vec2(220.0, 42.0));
        if ui.put(b, egui::Button::new(RichText::new("Open photo").font(brand::semibold(15.0)).color(Color32::from_rgb(0x1a, 0x10, 0x00))).fill(ORANGE).corner_radius(cr(12))).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            self.pick_open(ctx);
        }
        for (i, t) in ["AI background", "Auto head crop", "Repair brush", "Duet photos", "Print sheets"].iter().enumerate() {
            let x = c.x + (i as f32 - 2.0) * 128.0;
            let rr = Rect::from_center_size(pos2(x, c.y + 225.0), vec2(118.0, 26.0));
            painter.rect_filled(rr, cr(13), Color32::from_rgb(0x13, 0x1c, 0x2e));
            painter.rect_stroke(rr, cr(13), Stroke::new(1.0_f32, BORDER), StrokeKind::Inside);
            painter.text(rr.center(), egui::Align2::CENTER_CENTER, t, brand::medium(11.5), MUTED);
        }
    }

    fn source_view(&mut self, ui: &mut Ui, area: Rect) {
        let (Some(src), Some(tex)) = (self.src.clone(), self.src_tex.clone()) else { return };
        let (iw, ih) = (src.width() as f32, src.height() as f32);
        let painter = ui.painter_at(area);
        self.panel_bg(&painter, area, "ORIGINAL  -  CROP FRAME");
        let draw = fit_rect(Rect::from_min_max(area.min + vec2(0.0, 30.0), area.max), iw, ih, 12.0);
        painter.image(tex.id(), draw, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        let scale = draw.width() / iw;
        let e = &self.edit;
        let (cx, cy, cw, ch) = (e.cx, e.cy, e.cw, e.crop_h());
        let (sn, cs) = e.angle.to_radians().sin_cos();
        let to_screen = |u: f32, v: f32| -> Pos2 {
            let (lx, ly) = (u * cw, v * ch);
            let sx = cx + lx * cs - ly * sn;
            let sy = cy + lx * sn + ly * cs;
            draw.min + vec2(sx * scale, sy * scale)
        };
        let corners = [to_screen(-0.5, -0.5), to_screen(0.5, -0.5), to_screen(0.5, 0.5), to_screen(-0.5, 0.5)];
        if e.angle.abs() < 0.01 {
            let f = Rect::from_min_max(corners[0], corners[2]).intersect(draw);
            let dim = Color32::from_black_alpha(140);
            painter.rect_filled(Rect::from_min_max(draw.min, pos2(draw.max.x, f.min.y)), 0.0, dim);
            painter.rect_filled(Rect::from_min_max(pos2(draw.min.x, f.max.y), draw.max), 0.0, dim);
            painter.rect_filled(Rect::from_min_max(pos2(draw.min.x, f.min.y), pos2(f.min.x, f.max.y)), 0.0, dim);
            painter.rect_filled(Rect::from_min_max(pos2(f.max.x, f.min.y), pos2(draw.max.x, f.max.y)), 0.0, dim);
        }
        painter.add(egui::Shape::closed_line(corners.to_vec(), Stroke::new(2.0_f32, ORANGE)));
        if self.show_guides {
            let (head_ratio, top) = e.head_params();
            let chin = top + head_ratio;
            let g = Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(80, 200, 255, 190));
            painter.line_segment([to_screen(0.0, -0.5), to_screen(0.0, 0.5)], g);
            for fy in [top, chin] {
                painter.line_segment([to_screen(-0.5, fy - 0.5), to_screen(0.5, fy - 0.5)], g);
            }
            let half_u = (0.70 * head_ratio / 2.0) / e.aspect();
            let pts: Vec<Pos2> = (0..48)
                .map(|i| {
                    let a = i as f32 / 48.0 * std::f32::consts::TAU;
                    to_screen(a.cos() * half_u, (top + chin) / 2.0 - 0.5 + a.sin() * head_ratio / 2.0)
                })
                .collect();
            painter.add(egui::Shape::closed_line(pts, Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(80, 200, 255, 110))));
        }
        for c in corners {
            painter.circle_filled(c, 6.0, ORANGE);
            painter.circle_stroke(c, 6.0, Stroke::new(1.5_f32, Color32::from_rgb(0x1a, 0x10, 0x00)));
        }
        let resp = ui.interact(draw, egui::Id::new("src_view"), Sense::click_and_drag());
        let center_s = to_screen(0.0, 0.0);
        if resp.drag_started() {
            if let Some(p) = resp.interact_pointer_pos() {
                let near = corners.iter().any(|c| c.distance(p) < 20.0);
                self.drag = if near { Drag::Resize { start_dist: p.distance(center_s).max(1.0), cw0: self.edit.cw } } else { Drag::Move };
            }
        }
        if resp.dragged() {
            match self.drag {
                Drag::Move => {
                    let d = resp.drag_delta() / scale;
                    self.edit.cx += d.x;
                    self.edit.cy += d.y;
                }
                Drag::Resize { start_dist, cw0 } => {
                    if let Some(p) = resp.interact_pointer_pos() {
                        self.edit.cw = (cw0 * p.distance(center_s) / start_dist).clamp(32.0, 4.0 * iw.max(ih));
                    }
                }
                Drag::None => {}
            }
        }
        if resp.drag_stopped() {
            self.drag = Drag::None;
        }
        if resp.hovered() {
            let near = ui.input(|i| i.pointer.hover_pos()).map(|p| corners.iter().any(|c| c.distance(p) < 20.0)).unwrap_or(false);
            ui.ctx().set_cursor_icon(if near { egui::CursorIcon::ResizeNwSe } else { egui::CursorIcon::Grab });
            let sd = ui.input(|i| i.smooth_scroll_delta.y);
            if sd.abs() > 0.1 {
                self.edit.cw = (self.edit.cw * (1.0 - sd * 0.0015)).clamp(32.0, 4.0 * iw.max(ih));
            }
        }
    }

    /// offset (px) of Photo A inside the printable unit (non-zero only for duets)
    fn photo_a_offset(&self) -> (f32, f32) {
        if !self.edit.duet {
            return (0.0, 0.0);
        }
        let c = self.edit.duet_cells()[self.edit.duet_active as usize];
        (c.0 as f32, c.1 as f32)
    }

    fn result_view(&mut self, ui: &mut Ui, area: Rect) {
        let painter = ui.painter_at(area);
        self.panel_bg(&painter, area, if self.edit.duet { "RESULT  -  DUET" } else { "RESULT" });
        let Some(tex) = self.unit_tex.clone() else {
            painter.text(area.center(), egui::Align2::CENTER_CENTER, "Rendering...", egui::FontId::proportional(16.0), MUTED);
            return;
        };
        let (tw, th) = (tex.size()[0] as f32, tex.size()[1] as f32);
        let inner = Rect::from_min_max(area.min + vec2(0.0, 30.0), area.max - vec2(0.0, 28.0));
        let base = fit_rect(inner, tw, th, 12.0);
        let draw = Rect::from_center_size(base.center() + self.rpan, base.size() * self.rz);
        let scale = draw.width() / tw; // screen px per photo px
        painter.rect_filled(draw.expand(3.0).intersect(inner), 3.0, Color32::from_gray(55));
        painter.with_clip_rect(inner).image(tex.id(), draw, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        let (mmw, mmh) = if self.edit.duet { (self.edit.duet_w_mm, self.edit.duet_h_mm) } else { self.edit.size_mm() };
        let (ppw, pph) = if self.edit.duet { self.edit.duet_px() } else { self.edit.photo_px() };
        painter.text(pos2(area.center().x, area.max.y - 14.0), egui::Align2::CENTER_CENTER, format!("{:.1} x {:.1} mm   |   {} x {} px   |   {} DPI", mmw, mmh, ppw, pph, self.edit.dpi), egui::FontId::proportional(11.5), MUTED);

        let resp = ui.interact(inner, egui::Id::new("res_view"), Sense::click_and_drag());
        let hover = resp.hover_pos();
        // zoom (scroll) around the cursor, pan (right / middle drag, or primary drag when no tool)
        if resp.hovered() {
            let sd = ui.input(|i| i.smooth_scroll_delta.y);
            if sd.abs() > 0.1 {
                if let Some(p) = hover {
                    let nz = (self.rz * (1.0 + sd * 0.0025)).clamp(1.0, 10.0);
                    let rel = p - draw.center();
                    let f = nz / self.rz;
                    let nc = p - rel * f;
                    self.rpan = nc - base.center();
                    self.rz = nz;
                    if self.rz <= 1.001 {
                        self.rz = 1.0;
                        self.rpan = Vec2::ZERO;
                    }
                }
            }
        }
        let cell_busy = if self.edit.duet && self.tool == Tool::None { let pl = painter.clone(); self.duet_canvas(ui, &pl, &resp, draw, scale, inner) } else { false };
        let panning = !cell_busy && ui.input(|i| i.pointer.secondary_down() || i.pointer.middle_down()) || (self.tool == Tool::None && resp.dragged_by(egui::PointerButton::Primary) && self.rz > 1.0);
        if resp.hovered() && panning {
            let d = ui.input(|i| i.pointer.delta());
            self.rpan += d;
        }
        if self.rz > 1.0 {
            let lim = (draw.size() * 0.5 + inner.size() * 0.3).max(Vec2::ZERO);
            self.rpan = self.rpan.clamp(-lim, lim);
        }
        if self.tool != Tool::None {
            painter.text(pos2(inner.center().x, inner.max.y - 8.0), egui::Align2::CENTER_BOTTOM, "scroll = zoom   |   right-drag = move   |   Ctrl+Z = undo   |   Esc = stop", egui::FontId::proportional(11.0), MUTED);
        }
        // little zoom badge
        if self.rz > 1.0 {
            let b = Rect::from_min_size(pos2(inner.max.x - 118.0, inner.min.y + 6.0), vec2(108.0, 24.0));
            if ui.put(b, egui::Button::new(RichText::new(format!("{:.0}%   Fit", self.rz * 100.0)).size(12.0)).corner_radius(cr(12))).clicked() {
                self.rz = 1.0;
                self.rpan = Vec2::ZERO;
            }
        }

        // ---- photo-local mapping (A photo inside the unit)
        let (ox, oy) = self.photo_a_offset();
        let (pw, ph) = self.edit.photo_px();
        let to_photo = |p: Pos2| -> (f32, f32) { ((p.x - draw.min.x) / scale - ox, (p.y - draw.min.y) / scale - oy) };
        let to_screen_photo = |x: f32, y: f32| -> Pos2 { draw.min + vec2((x + ox) * scale, (y + oy) * scale) };

        // ---- overlays of strokes that are still being re-rendered
        for ov in &self.overlay {
            let col = if ov.erase { Color32::from_rgba_unmultiplied(51, 161, 255, 110) } else { Color32::from_rgba_unmultiplied(255, 92, 108, 110) };
            let pts: Vec<Pos2> = ov.pts.iter().map(|p| to_screen_photo(p.x, p.y)).collect();
            let w = ov.r * 2.0 * scale;
            let pl = painter.with_clip_rect(inner);
            if pts.len() > 1 {
                pl.add(egui::Shape::line(pts.clone(), Stroke::new(w, col)));
            }
            for p in pts {
                pl.circle_filled(p, ov.r * scale, col);
            }
        }

        match self.tool {
            Tool::Heal => {
                if let Some(p) = hover {
                    painter.circle_stroke(p, self.heal_r * pw as f32 * scale, Stroke::new(1.5_f32, ORANGE));
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                }
                if resp.clicked() {
                    if let Some(p) = resp.interact_pointer_pos() {
                        let (x, y) = to_photo(p);
                        if x >= 0.0 && y >= 0.0 && x <= pw as f32 && y <= ph as f32 {
                            self.edit.spots.push(Spot { x: x / pw as f32, y: y / ph as f32, r: self.heal_r });
                        }
                    }
                }
            }
            Tool::Restore | Tool::Erase => {
                let erase = self.tool == Tool::Erase;
                let ring = if erase { BLUE } else { RED };
                if let Some(p) = hover {
                    painter.circle_stroke(p, self.brush_px / 2.0, Stroke::new(1.5_f32, ring));
                    painter.circle_stroke(p, self.brush_px / 2.0 + 1.5, Stroke::new(1.0_f32, Color32::from_black_alpha(160)));
                    if self.brush_hard < 0.98 {
                        // inner ring = where the brush works at full strength, between the rings it fades out
                        painter.circle_stroke(p, self.brush_px / 2.0 * self.brush_hard, Stroke::new(1.0_f32, ring.gamma_multiply(0.6)));
                    }
                    ui.ctx().set_cursor_icon(egui::CursorIcon::None);
                }
                let down = ui.input(|i| i.pointer.primary_down()) && resp.is_pointer_button_down_on() && !panning;
                if down {
                    if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
                        let (x, y) = to_photo(p);
                        let r_photo = self.brush_px / 2.0 / scale;
                        let (sn, cs) = self.edit.angle.to_radians().sin_cos();
                        let (u, v) = (x / pw as f32, y / ph as f32);
                        let (lx, ly) = ((u - 0.5) * self.edit.cw, (v - 0.5) * self.edit.crop_h());
                        let sxy = (self.edit.cx + lx * cs - ly * sn, self.edit.cy + lx * sn + ly * cs);
                        let r_src = r_photo * self.edit.cw / pw as f32;
                        if !self.painting {
                            self.painting = true;
                            self.overlay_clear_id = u64::MAX;
                            self.edit.strokes.push(BrushStroke { erase, r: r_src, pts: vec![sxy], hard: self.brush_hard, strength: self.brush_strength });
                            self.overlay.push(Overlay { erase, r: r_photo, pts: vec![pos2(x, y)] });
                        } else if let (Some(s), Some(o)) = (self.edit.strokes.last_mut(), self.overlay.last_mut()) {
                            let last = *s.pts.last().unwrap();
                            if ((sxy.0 - last.0).powi(2) + (sxy.1 - last.1).powi(2)).sqrt() > r_src * 0.3 {
                                s.pts.push(sxy);
                                o.pts.push(pos2(x, y));
                            }
                        }
                    }
                } else if self.painting {
                    self.painting = false;
                    self.overlay_clear_id = self.job_id + 1;
                    self.last_key = None; // force one final render that contains the whole stroke
                }
            }
            Tool::Pen | Tool::Lasso => {
                let lasso = self.tool == Tool::Lasso;
                let col = match self.cut_mode {
                    CutMode::Erase => BLUE,
                    CutMode::Restore => RED,
                    CutMode::KeepInside => GREEN,
                };
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                let (sn, cs) = self.edit.angle.to_radians().sin_cos();
                let (cw, chh, ecx, ecy) = (self.edit.cw, self.edit.crop_h(), self.edit.cx, self.edit.cy);
                let to_src = |x: f32, y: f32| -> (f32, f32) {
                    let (lx, ly) = ((x / pw as f32 - 0.5) * cw, (y / ph as f32 - 0.5) * chh);
                    (ecx + lx * cs - ly * sn, ecy + lx * sn + ly * cs)
                };
                let pl = painter.with_clip_rect(inner);
                let spts: Vec<Pos2> = self.pen.iter().map(|(p, _)| to_screen_photo(p.x, p.y)).collect();
                if spts.len() > 1 {
                    pl.add(egui::Shape::line(spts.clone(), Stroke::new(2.0_f32, col)));
                }
                if let (Some(h), Some(last), false) = (hover, spts.last(), lasso) {
                    pl.line_segment([*last, h], Stroke::new(1.5_f32, col.gamma_multiply(0.6)));
                }
                if spts.len() > 2 {
                    pl.line_segment([spts[spts.len() - 1], spts[0]], Stroke::new(1.0_f32, col.gamma_multiply(0.4)));
                }
                for (i, p) in spts.iter().enumerate() {
                    pl.circle_filled(*p, if i == 0 { 6.0 } else { 3.5 }, if i == 0 { ORANGE } else { col });
                }
                let mut finish = false;
                if lasso {
                    let down = ui.input(|i| i.pointer.primary_down()) && resp.is_pointer_button_down_on() && !panning;
                    if down {
                        if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
                            let far = spts.last().map(|l| l.distance(p) > 3.0).unwrap_or(true);
                            if far {
                                let (x, y) = to_photo(p);
                                self.pen.push((pos2(x, y), to_src(x, y)));
                            }
                        }
                    } else if !self.pen.is_empty() {
                        finish = true;
                    }
                } else {
                    if resp.clicked() && !panning {
                        if let Some(p) = resp.interact_pointer_pos() {
                            if self.pen.len() >= 3 && spts.first().map(|f| f.distance(p) < 10.0).unwrap_or(false) {
                                finish = true; // clicked the first point = close the shape
                            } else if spts.last().map(|l| l.distance(p) > 3.0).unwrap_or(true) {
                                let (x, y) = to_photo(p);
                                self.pen.push((pos2(x, y), to_src(x, y)));
                            }
                        }
                    }
                    if resp.double_clicked() && self.pen.len() >= 3 {
                        finish = true;
                    }
                    if ui.input(|i| i.key_pressed(egui::Key::Enter)) && self.pen.len() >= 3 {
                        finish = true;
                    }
                    if ui.input(|i| i.key_pressed(egui::Key::Backspace)) {
                        self.pen.pop();
                    }
                }
                if finish {
                    self.finish_cut(pw as f32);
                }
            }
            Tool::None => {}
        }
        if self.tool != Tool::Pen && self.tool != Tool::Lasso && !self.pen.is_empty() {
            self.pen.clear();
        }
        if self.tool == Tool::None && self.painting {
            self.painting = false;
        }
    }

    /// close the shape that is being drawn with Pen / Lasso and put it into the edit
    fn finish_cut(&mut self, photo_w: f32) {
        if self.pen.len() >= 3 {
            let pts: Vec<(f32, f32)> = self.pen.iter().map(|(_, s)| *s).collect();
            let feather = self.cut_feather * self.edit.cw / photo_w.max(1.0);
            self.edit.cuts.push(PolyCut { mode: self.cut_mode, pts, feather });
            self.last_key = None;
        }
        self.pen.clear();
    }

    fn sheet_canvas(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let area = ui.available_rect_before_wrap();
        let painter = ui.painter_at(area);
        self.panel_bg(&painter, area, "PRINT SHEET PREVIEW");
        if self.unit.is_none() {
            painter.text(area.center(), egui::Align2::CENTER_CENTER, "Open a photo to see the print sheet", egui::FontId::proportional(16.0), MUTED);
            return;
        }
        let key = (self.photo_ver, self.settings.sheet.clone(), self.edit.dpi);
        if self.sheet_key.as_ref() != Some(&key) {
            let photo = self.unit.clone().unwrap();
            match sheet::make_sheets(&photo, &self.settings.sheet, self.edit.dpi) {
                Ok(pages) => {
                    let pg = &pages[0];
                    let k = (1100.0 / pg.height() as f32).min(1.0);
                    let small = image::imageops::resize(pg, (pg.width() as f32 * k) as u32, (pg.height() as f32 * k) as u32, image::imageops::FilterType::Triangle);
                    let ci = egui::ColorImage::from_rgb([small.width() as usize, small.height() as usize], small.as_raw());
                    self.sheet_tex = Some(ctx.load_texture("sheet", ci, TextureOptions::LINEAR));
                    self.sheet_info = format!("Page 1 of {}  -  real size {} x {} px @ {} DPI", pages.len(), pg.width(), pg.height(), self.edit.dpi);
                }
                Err(e) => {
                    self.sheet_tex = None;
                    self.sheet_info = e;
                }
            }
            self.sheet_key = Some(key);
        }
        match &self.sheet_tex {
            Some(t) => {
                let r = fit_rect(Rect::from_min_max(area.min + vec2(0.0, 30.0), area.max - vec2(0.0, 24.0)), t.size()[0] as f32, t.size()[1] as f32, 10.0);
                painter.rect_filled(r.expand(3.0), 3.0, Color32::from_gray(60));
                painter.image(t.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                painter.text(pos2(area.center().x, area.max.y - 13.0), egui::Align2::CENTER_CENTER, &self.sheet_info, egui::FontId::proportional(11.5), MUTED);
            }
            None => {
                painter.text(area.center(), egui::Align2::CENTER_CENTER, &self.sheet_info, egui::FontId::proportional(16.0), ORANGE);
            }
        }
    }
}
