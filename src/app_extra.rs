//! AI Models window, AI Enhance card and the Document / NID tool (UI + glue).
use super::*;
use crate::doc::{self, DocOpts, Pt};
use crate::{enhance, head, models};
use image::imageops::FilterType;

// ================================================================== state
pub(super) struct AiOut {
    pub img: RgbImage,
    pub alpha: Option<GrayImage>,
    pub crop_cw: Option<f32>,
    pub region: bool,
}
pub(super) struct AiUndo {
    src: Arc<RgbImage>,
    alpha: Option<Arc<GrayImage>>,
    edit: Edit,
}
pub(super) struct AiState {
    up: bool,
    up_model: String,
    face: bool,
    face_model: String,
    strength: f32,
    busy: bool,
    prog: (f32, String),
    cancel: Arc<AtomicBool>,
    pub(super) undo: Option<AiUndo>,
}
impl AiState {
    pub fn new() -> Self {
        AiState { up: true, up_model: "span_x4".into(), face: false, face_model: "gfpgan_1.4".into(), strength: 0.8, busy: false, prog: (0.0, String::new()), cancel: Arc::new(AtomicBool::new(false)), undo: None }
    }
}

pub(super) struct ModelsUi {
    pub open: bool,
    reg: Vec<models::Entry>,
    links: Vec<String>,
    dl: Option<(String, u64, u64)>,
    busy: bool,
    msg: String,
    cancel: Arc<AtomicBool>,
}
impl ModelsUi {
    pub fn new() -> Self {
        ModelsUi { open: false, reg: vec![], links: vec![], dl: None, busy: false, msg: String::new(), cancel: Arc::new(AtomicBool::new(false)) }
    }
}

#[derive(Default)]
pub(super) struct DocSlot {
    img: Option<Arc<RgbImage>>,
    small: Option<(Arc<RgbImage>, f32)>,
    tex: Option<TextureHandle>,
    name: String,
    path: Option<PathBuf>,
    quad: [Pt; 4],
    rot: u8,
    flip_h: bool,
    flip_v: bool,
    /// clean-up settings of THIS picture only
    opts: DocOpts,
    compare: bool,
    ai: Option<(String, RgbImage)>,
    res_tex: Option<TextureHandle>,
    res_key: String,
    info: String,
    busy: bool,
    /// the user moved the corners by hand: a late AI answer must not override them
    touched: bool,
}
#[derive(Clone)]
pub(super) struct PageDrag {
    item: usize,
    /// None = moving, Some(0..8) = resize handle (TL,T,TR,R,BR,B,BL,L)
    handle: Option<usize>,
    start: Pos2,
    orig: doc::PageItem,
    resize: bool,
}
pub(super) struct DocState {
    slots: [DocSlot; 2],
    pub(super) active: usize,
    drag: Option<usize>,
    sel: Option<usize>,
    page_drag: Option<PageDrag>,
    item_tex: Vec<Option<(String, TextureHandle)>>,
    page_info: String,
    ai_model: String,
    last_slot: usize,
    erase: bool,
    erase_r: f32,
    /// zoom / pan of the 4 picture views (front photo, front result, back photo, back result)
    zoom: [f32; 4],
    pan: [Vec2; 4],
}
impl DocState {
    pub fn new() -> Self {
        DocState { slots: [DocSlot::default(), DocSlot::default()], active: 0, drag: None, sel: None, page_drag: None, item_tex: vec![], page_info: String::new(), ai_model: "span_x4".into(), last_slot: 0, erase: false, erase_r: 0.02, zoom: [1.0; 4], pan: [Vec2::ZERO; 4] }
    }
}

fn model_label(id: &str) -> String {
    models::entry(id).map(|e| e.label).unwrap_or_else(|| id.to_string())
}

pub(super) fn tex_from(ctx: &egui::Context, name: &str, img: &RgbImage) -> TextureHandle {
    let ci = egui::ColorImage::from_rgb([img.width() as usize, img.height() as usize], img.as_raw());
    ctx.load_texture(name, ci, TextureOptions::LINEAR)
}

// ================================================================== messages
impl App {
    pub(super) fn extra_msg(&mut self, ctx: &egui::Context, m: Msg) {
        match m {
            Msg::MDl(id, d, t) => self.mui.dl = Some((id, d, t.max(1))),
            Msg::MDone(id, r) => {
                self.mui.dl = None;
                match r {
                    Ok(()) => {
                        self.status = format!("{} is ready.", model_label(&id));
                        if id == self.settings.model_key && self.src.is_some() && self.alpha.is_none() && self.view != View::Doc {
                            self.start_seg(ctx, true);
                        }
                    }
                    Err(e) if e == "Cancelled" => self.status = "Download cancelled.".into(),
                    Err(e) => self.error = Some(e),
                }
            }
            Msg::MOp(r) => {
                self.mui.busy = false;
                self.refresh_models();
                match r {
                    Ok(s) => {
                        self.mui.msg = s.clone();
                        self.status = s;
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            Msg::AiProg(f, t) => self.ai.prog = (f, t),
            Msg::AiDone(id, r) => {
                self.ai.busy = false;
                match r {
                    Ok(out) if id == self.img_id => self.apply_ai(ctx, out),
                    Ok(_) => {}
                    Err(e) if e == "Cancelled" => self.status = "AI enhance cancelled.".into(),
                    Err(e) => self.error = Some(e),
                }
            }
            Msg::DocDetect(i, r) => {
                self.doc.slots[i].busy = false;
                match r {
                    Ok(_) if self.doc.slots[i].touched => self.status = "AI detect ignored: you already moved the corners by hand.".into(),
                    Ok(d) => self.doc_apply_detect(i, d, "AI"),
                    Err(e) => self.error = Some(e),
                }
            }
            Msg::DocAi(i, key, r) => {
                self.doc.slots[i].busy = false;
                match r {
                    Ok(img) => {
                        self.doc.slots[i].ai = Some((key, img));
                        self.status = "AI clean-up done.".into();
                    }
                    Err(e) if e == "Cancelled" => self.status = "AI clean-up cancelled.".into(),
                    Err(e) => self.error = Some(e),
                }
            }
            _ => {}
        }
    }

    // ============================================================ models
    pub(super) fn refresh_models(&mut self) {
        models::reload_registry();
        self.mui.reg = models::registry();
        self.mui.links = self.mui.reg.iter().map(|e| e.urls.join("\n")).collect();
    }

    pub(super) fn start_model_dl(&mut self, ctx: &egui::Context, id: &str) {
        if self.mui.dl.is_some() {
            self.status = "Another download is running - wait or cancel it in AI Models.".into();
            return;
        }
        self.mui.cancel.store(false, Ordering::Relaxed);
        self.mui.dl = Some((id.to_string(), 0, 1));
        let (tx, cancel, ctx, id) = (self.tx.clone(), self.mui.cancel.clone(), ctx.clone(), id.to_string());
        std::thread::spawn(move || {
            let (tx2, ctx2, id2) = (tx.clone(), ctx.clone(), id.clone());
            let r = models::download(&id, &move |d, t| {
                let _ = tx2.send(Msg::MDl(id2.clone(), d, t));
                ctx2.request_repaint();
            }, &cancel);
            let _ = tx.send(Msg::MDone(id, r));
            ctx.request_repaint();
        });
    }

    /// "Get model" button used inside cards: progress bar while downloading, button otherwise.
    fn model_need(&mut self, ui: &mut Ui, ctx: &egui::Context, id: &str) -> bool {
        if models::is_installed(id) {
            return true;
        }
        let e = models::entry(id);
        if let Some((did, d, t)) = self.mui.dl.clone() {
            if did == id {
                ui.add(egui::ProgressBar::new(d as f32 / t as f32).corner_radius(cr(8)).text(format!("Downloading  {:.0} / {:.0} MB", d as f32 / 1e6, t as f32 / 1e6)));
                if button(ui, "Cancel", Kind::Danger, vec2(80.0, 26.0)).clicked() {
                    self.mui.cancel.store(true, Ordering::Relaxed);
                }
                return false;
            }
        }
        hint(ui, &format!("Needs: {}", e.as_ref().map(|e| e.label.as_str()).unwrap_or(id)));
        ui.horizontal(|ui| {
            if button(ui, &format!("Download ({} MB)", e.as_ref().map(|e| e.size_mb).unwrap_or(0)), Kind::Accent, vec2(0.0, 28.0)).clicked() {
                self.start_model_dl(ctx, id);
            }
            if button(ui, "AI Models...", Kind::Ghost, vec2(0.0, 28.0)).clicked() {
                self.mui.open = true;
                self.refresh_models();
            }
        });
        false
    }

    pub(super) fn models_window(&mut self, ctx: &egui::Context) {
        if !self.mui.open {
            return;
        }
        if self.mui.reg.is_empty() {
            self.refresh_models();
        }
        let mut open = true;
        let mut act: Option<(usize, &'static str)> = None;
        egui::Window::new(RichText::new("AI Models").font(brand::semibold(15.0)).color(ORANGE)).open(&mut open).collapsible(false).default_size([700.0, 600.0]).show(ctx, |ui| {
            let dir = models::models_dir();
            ui.label(RichText::new(format!("Folder:  {}", dir.display())).font(brand::medium(12.0)).color(TEXT));
            hint(ui, "Same folder layout as the Ripon Computer (Python) app. Models you already have there are used automatically.");
            ui.horizontal_wrapped(|ui| {
                if button(ui, "Open folder", Kind::Ghost, vec2(0.0, 28.0)).clicked() {
                    let _ = open::that(&dir);
                }
                ui.add_enabled_ui(!self.mui.busy, |ui| {
                    if button(ui, "Backup all models (.zip)", Kind::Primary, vec2(0.0, 28.0)).clicked() {
                        act = Some((0, "backup"));
                    }
                    if button(ui, "Restore from backup (.zip)", Kind::Primary, vec2(0.0, 28.0)).clicked() {
                        act = Some((0, "restore"));
                    }
                });
                if self.mui.busy {
                    ui.spinner();
                }
            });
            if !self.mui.msg.is_empty() {
                ui.label(RichText::new(&self.mui.msg).size(11.5).color(GREEN));
            }
            ui.add_space(4.0);
            ui.separator();
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for i in 0..self.mui.reg.len() {
                    let e = self.mui.reg[i].clone();
                    let inst = models::is_installed(&e.id);
                    egui::Frame::new().fill(CARD).stroke(Stroke::new(1.0_f32, BORDER)).corner_radius(cr(10)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&e.label).font(brand::semibold(13.0)).color(TEXT));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(RichText::new(if inst { "Installed" } else { "Not installed" }).font(brand::semibold(12.0)).color(if inst { GREEN } else { ORANGE }));
                            });
                        });
                        hint(ui, &format!("{}   |   ~{} MB   |   models/{}/{}", e.purpose, e.size_mb, e.folder, e.file));
                        let dling = self.mui.dl.as_ref().filter(|d| d.0 == e.id).cloned();
                        if let Some((_, d, t)) = dling {
                            ui.add(egui::ProgressBar::new(d as f32 / t as f32).corner_radius(cr(8)).text(format!("{:.0} / {:.0} MB", d as f32 / 1e6, t as f32 / 1e6)));
                            if button(ui, "Cancel", Kind::Danger, vec2(80.0, 26.0)).clicked() {
                                self.mui.cancel.store(true, Ordering::Relaxed);
                            }
                        } else {
                            ui.horizontal_wrapped(|ui| {
                                ui.add_enabled_ui(self.mui.dl.is_none(), |ui| {
                                    if button(ui, if inst { "Download again" } else { "Download" }, Kind::Accent, vec2(0.0, 26.0)).clicked() {
                                        act = Some((i, "download"));
                                    }
                                });
                                if button(ui, "Import file...", Kind::Ghost, vec2(0.0, 26.0)).on_hover_text("Choose a .onnx file you downloaded by hand").clicked() {
                                    act = Some((i, "import"));
                                }
                                if inst && button(ui, "Delete", Kind::Danger, vec2(0.0, 26.0)).clicked() {
                                    act = Some((i, "delete"));
                                }
                            });
                        }
                        egui::CollapsingHeader::new(RichText::new("Download links  (edit - one per line, tried in order)").size(12.0).color(MUTED)).id_salt(("links", &e.id)).show(ui, |ui| {
                            ui.add(egui::TextEdit::multiline(&mut self.mui.links[i]).desired_rows(2).desired_width(ui.available_width()).font(egui::TextStyle::Small));
                            ui.horizontal(|ui| {
                                if button(ui, "Save links", Kind::Primary, vec2(0.0, 26.0)).clicked() {
                                    act = Some((i, "savelinks"));
                                }
                                if button(ui, "Reset to default link", Kind::Ghost, vec2(0.0, 26.0)).clicked() {
                                    act = Some((i, "resetlinks"));
                                }
                            });
                            hint(ui, "A link expired? Paste a new direct .onnx link here and press Save, then Download.");
                        });
                    });
                    ui.add_space(4.0);
                }
            });
        });
        if !open {
            self.mui.open = false;
        }
        if let Some((i, a)) = act {
            self.models_action(ctx, i, a);
        }
    }

    fn models_action(&mut self, ctx: &egui::Context, i: usize, a: &str) {
        match a {
            "backup" => {
                let name = format!("Ripon_AI_models_backup_{}.zip", chrono::Local::now().format("%Y%m%d"));
                if let Some(p) = rfd::FileDialog::new().set_file_name(&name).add_filter("Zip", &["zip"]).save_file() {
                    self.mui.busy = true;
                    self.mui.msg = "Backing up models...".into();
                    let (tx, ctx) = (self.tx.clone(), ctx.clone());
                    std::thread::spawn(move || {
                        let r = models::backup(&p).map(|n| format!("Backup saved: {n} file(s) -> {}", p.display()));
                        let _ = tx.send(Msg::MOp(r));
                        ctx.request_repaint();
                    });
                }
            }
            "restore" => {
                if let Some(p) = rfd::FileDialog::new().add_filter("Zip", &["zip"]).pick_file() {
                    self.mui.busy = true;
                    self.mui.msg = "Restoring...".into();
                    let (tx, ctx) = (self.tx.clone(), ctx.clone());
                    std::thread::spawn(move || {
                        let r = models::restore(&p).map(|n| format!("Restored {n} file(s) (models and download links)."));
                        let _ = tx.send(Msg::MOp(r));
                        ctx.request_repaint();
                    });
                }
            }
            "download" => {
                let id = self.mui.reg[i].id.clone();
                self.start_model_dl(ctx, &id);
            }
            "import" => {
                let id = self.mui.reg[i].id.clone();
                if let Some(p) = rfd::FileDialog::new().add_filter("ONNX model", &["onnx"]).pick_file() {
                    match models::import(&id, &p) {
                        Ok(()) => self.mui.msg = format!("Imported {}", self.mui.reg[i].label),
                        Err(e) => self.error = Some(e),
                    }
                }
            }
            "delete" => {
                models::delete(&self.mui.reg[i].id.clone());
                self.mui.msg = format!("Deleted {}", self.mui.reg[i].label);
            }
            "savelinks" => {
                let urls: Vec<String> = self.mui.links[i].lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
                if urls.is_empty() {
                    self.error = Some("Put at least one link.".into());
                    return;
                }
                self.mui.reg[i].urls = urls;
                match models::save_registry(&self.mui.reg) {
                    Ok(()) => self.mui.msg = "Links saved.".into(),
                    Err(e) => self.error = Some(e),
                }
            }
            "resetlinks" => {
                if let Some(d) = models::defaults().into_iter().find(|d| d.id == self.mui.reg[i].id) {
                    self.mui.reg[i].urls = d.urls.clone();
                    self.mui.links[i] = d.urls.join("\n");
                    let _ = models::save_registry(&self.mui.reg);
                    self.mui.msg = "Default link restored.".into();
                }
            }
            _ => {}
        }
    }

    // ============================================================ AI enhance (photo)
    pub(super) fn ai_card(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let has = self.src.is_some();
        card(ui, "", "AI Enhance  (upscale + face)", false, |ui| {
            hint(ui, "For small, blurry or old photos. Works on the crop frame (plus a margin).");
            ui.checkbox(&mut self.ai.up, "Upscale x4  (bigger and sharper)");
            if self.ai.up {
                egui::ComboBox::from_id_salt("upmodel").width(ui.available_width()).selected_text(enhance::UPSCALERS.iter().find(|m| m.0 == self.ai.up_model).map(|m| m.1).unwrap_or("?")).show_ui(ui, |ui| {
                    for (id, label) in enhance::UPSCALERS {
                        ui.selectable_value(&mut self.ai.up_model, id.to_string(), *label);
                    }
                });
            }
            ui.checkbox(&mut self.ai.face, "Restore face  (blur / old photo)");
            if self.ai.face {
                egui::ComboBox::from_id_salt("facemodel").width(ui.available_width()).selected_text(enhance::FACE_MODELS.iter().find(|m| m.0 == self.ai.face_model).map(|m| m.1).unwrap_or("?")).show_ui(ui, |ui| {
                    for (id, label) in enhance::FACE_MODELS {
                        ui.selectable_value(&mut self.ai.face_model, id.to_string(), *label);
                    }
                });
                slider_row(ui, "Strength", &mut self.ai.strength, 0.0, 1.0, 100.0, "");
            }
            let mut ready = self.ai.up || self.ai.face;
            if self.ai.up {
                let id = self.ai.up_model.clone();
                ready &= self.model_need(ui, ctx, &id);
            }
            if self.ai.face {
                let id = self.ai.face_model.clone();
                ready &= self.model_need(ui, ctx, &id);
            }
            if self.ai.busy {
                ui.add(egui::ProgressBar::new(self.ai.prog.0).corner_radius(cr(8)).text(self.ai.prog.1.clone()));
                if button(ui, "Cancel", Kind::Danger, vec2(80.0, 26.0)).clicked() {
                    self.ai.cancel.store(true, Ordering::Relaxed);
                }
            } else {
                ui.add_enabled_ui(has && ready, |ui| {
                    if full_button(ui, "Run AI enhance", Kind::Primary, 34.0).clicked() {
                        self.start_ai(ctx);
                    }
                });
                ui.add_enabled_ui(self.ai.undo.is_some(), |ui| {
                    if full_button(ui, "Undo AI enhance", Kind::Ghost, 28.0).clicked() {
                        self.undo_ai(ctx);
                    }
                });
            }
            if self.ai.up && self.ai.up_model.starts_with("real_esrgan") {
                hint(ui, "Real-ESRGAN is the best quality but slow (about a minute for a small photo). SPAN is fast.");
            }
        });
    }

    fn set_src_image(&mut self, ctx: &egui::Context, img: RgbImage) {
        let (w, h) = img.dimensions();
        let k = (1600.0 / w.max(h) as f32).min(1.0);
        let prev = if k < 1.0 { image::imageops::resize(&img, (w as f32 * k) as u32, (h as f32 * k) as u32, FilterType::Triangle) } else { img.clone() };
        self.src_tex = Some(tex_from(ctx, "src", &prev));
        self.src = Some(Arc::new(img));
        self.last_key = None;
    }

    fn start_ai(&mut self, ctx: &egui::Context) {
        let Some(src) = self.src.clone() else { return };
        let (alpha, edit) = (self.alpha.clone(), self.edit.clone());
        let up = if self.ai.up { Some(self.ai.up_model.clone()) } else { None };
        let face = if self.ai.face { Some((self.ai.face_model.clone(), self.ai.strength)) } else { None };
        self.ai.busy = true;
        self.ai.prog = (0.0, "Starting...".into());
        self.ai.cancel.store(false, Ordering::Relaxed);
        let (tx, cancel, ctx, id) = (self.tx.clone(), self.ai.cancel.clone(), ctx.clone(), self.img_id);
        std::thread::spawn(move || {
            let (tx2, ctx2) = (tx.clone(), ctx.clone());
            let prog = move |f: f32, t: &str| {
                let _ = tx2.send(Msg::AiProg(f, t.to_string()));
                ctx2.request_repaint();
            };
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_ai(src, alpha, edit, up, face, &cancel, &prog))).unwrap_or_else(|_| Err("AI engine crashed".into()));
            let _ = tx.send(Msg::AiDone(id, r));
            ctx.request_repaint();
        });
    }

    fn apply_ai(&mut self, ctx: &egui::Context, out: AiOut) {
        let Some(old) = self.src.clone() else { return };
        self.ai.undo = Some(AiUndo { src: old, alpha: self.alpha.clone(), edit: self.edit.clone() });
        let (w, h) = out.img.dimensions();
        self.set_src_image(ctx, out.img);
        if out.region {
            self.alpha = out.alpha.map(Arc::new);
            self.alpha_ver += 1;
            self.edit.strokes.clear();
            self.edit.cuts.clear();
            self.edit.spots.clear();
            self.overlay.clear();
            self.edit.cx = w as f32 / 2.0;
            self.edit.cy = h as f32 / 2.0;
            self.edit.angle = 0.0;
            if let Some(cw) = out.crop_cw {
                self.edit.cw = cw;
            }
            self.status = format!("AI enhance done: picture is now {w} x {h} px. (Undo AI enhance is in the AI Enhance card)");
        } else {
            self.status = "Face restored.".into();
        }
    }

    fn undo_ai(&mut self, ctx: &egui::Context) {
        if let Some(u) = self.ai.undo.take() {
            let img = (*u.src).clone();
            self.set_src_image(ctx, img);
            self.alpha = u.alpha;
            self.alpha_ver += 1;
            self.edit = u.edit;
            self.overlay.clear();
            self.status = "AI enhance undone.".into();
        }
    }

    // ============================================================ document tool: slots
    pub(super) fn doc_drop(&mut self, ctx: &egui::Context, p: PathBuf) {
        let i = if self.doc.slots[0].img.is_none() { 0 } else if self.doc.slots[1].img.is_none() { 1 } else { self.doc.active.min(1) };
        self.doc_open(ctx, i, p);
    }

    fn doc_pick(&mut self, ctx: &egui::Context, i: usize) {
        let mut d = rfd::FileDialog::new().add_filter("Pictures", &["jpg", "jpeg", "png", "bmp", "webp"]);
        if !self.settings.last_dir.is_empty() {
            d = d.set_directory(&self.settings.last_dir);
        }
        if let Some(p) = d.pick_file() {
            self.doc_open(ctx, i, p);
        }
    }

    pub(super) fn doc_open(&mut self, ctx: &egui::Context, i: usize, path: PathBuf) {
        let img = match load_image(&path) {
            Ok(i) => i,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        if let Some(d) = path.parent() {
            self.settings.last_dir = d.to_string_lossy().into();
        }
        let (w, h) = img.dimensions();
        let k = (1400.0 / w.max(h) as f32).min(1.0);
        let small = if k < 1.0 { image::imageops::resize(&img, (w as f32 * k) as u32, (h as f32 * k) as u32, FilterType::Triangle) } else { img.clone() };
        let s = &mut self.doc.slots[i];
        s.tex = Some(tex_from(ctx, &format!("doc{i}"), &small));
        s.small = Some((Arc::new(small), k));
        s.img = Some(Arc::new(img));
        s.name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        s.path = Some(path);
        s.ai = None;
        s.res_key.clear();
        s.quad = doc::full_quad(w, h);
        s.rot = 0;
        s.flip_h = false;
        s.flip_v = false;
        s.opts = DocOpts::default(); // every picture has its OWN clean-up settings
        s.compare = false;
        self.doc.active = i;
        self.doc.last_slot = i;
        self.doc.slots[i].touched = false;
        self.doc.zoom[i * 2] = 1.0;
        self.doc.zoom[i * 2 + 1] = 1.0;
        self.doc.pan[i * 2] = Vec2::ZERO;
        self.doc.pan[i * 2 + 1] = Vec2::ZERO;
        self.doc_ensure_item(i);
        self.doc_auto_detect(ctx, i);
    }

    /// make sure the page has at least one item showing slot `i`
    fn doc_ensure_item(&mut self, i: usize) {
        if self.settings.page.items.iter().any(|it| it.slot == i) {
            return;
        }
        let (w, h) = self.doc_target_mm();
        self.settings.page.items.push(doc::PageItem { slot: i, x_mm: 0.0, y_mm: 0.0, w_mm: w, h_mm: h, lock: false });
        self.settings.page.auto_arrange();
        self.doc.sel = Some(self.settings.page.items.len() - 1);
    }

    fn doc_remove_slot(&mut self, i: usize) {
        self.doc.slots[i] = DocSlot::default();
        self.settings.page.items.retain(|it| it.slot != i);
        self.doc.sel = None;
        self.doc.item_tex.clear();
    }

    /// classic detection; a sheet of paper (A5 and bigger) that fills the picture has no visible edge -> whole picture
    fn doc_classic(&self, img: &RgbImage) -> Option<([Pt; 4], f32)> {
        let mut det = doc::detect_classic(img);
        let (tw, th) = self.doc_target_mm();
        if tw.max(th) >= 140.0 {
            let (iw, ih) = (img.width() as f32, img.height() as f32);
            let area = |q: &[Pt; 4]| -> f32 {
                let mut a = 0.0;
                for k in 0..4 {
                    let (p, n) = (q[k], q[(k + 1) % 4]);
                    a += p[0] * n[1] - n[0] * p[1];
                }
                a.abs() / 2.0
            };
            if det.as_ref().map(|d| area(&d.0) < 0.40 * iw * ih).unwrap_or(true) {
                det = Some(([[iw * 0.003, ih * 0.003], [iw * 0.997, ih * 0.003], [iw * 0.997, ih * 0.997], [iw * 0.003, ih * 0.997]], 1.0));
            }
        }
        det
    }

    /// the picture the left panel edits: the shown one, or the one selected in the page layout
    fn doc_act(&self) -> usize {
        if self.doc.active < 2 {
            return self.doc.active;
        }
        self.doc.sel.and_then(|s| self.settings.page.items.get(s)).map(|it| it.slot.min(1)).unwrap_or(self.doc.last_slot)
    }

    fn doc_auto_detect(&mut self, ctx: &egui::Context, i: usize) {
        let Some(img) = self.doc.slots[i].img.clone() else { return };
        let det = self.doc_classic(&img);
        let good = det.as_ref().map(|d| d.1 >= 0.88).unwrap_or(false);
        self.doc_apply_detect(i, det, "Auto");
        if !good && models::is_installed("isnet_general") {
            self.doc_detect_ai(ctx, i);
        }
    }

    fn doc_detect_ai(&mut self, ctx: &egui::Context, i: usize) {
        let Some(img) = self.doc.slots[i].img.clone() else { return };
        if !models::is_installed("isnet_general") {
            self.error = Some("AI detect needs the IS-Net model. Open AI Models and download it (170 MB), then try again.".into());
            return;
        }
        self.doc.slots[i].busy = true;
        self.doc.slots[i].info = "AI is looking for the card...".into();
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = std::panic::catch_unwind(|| match seg::segment(&img, "isnet_general", 1024) {
                Ok(m) => Ok(doc::detect_from_matte(&m, img.width(), img.height())),
                Err(e) => Err(e),
            })
            .unwrap_or_else(|_| Err("AI engine crashed".into()));
            let _ = tx.send(Msg::DocDetect(i, r));
            ctx.request_repaint();
        });
    }

    /// does the (first) page item of this slot want a landscape result?
    fn doc_wants_landscape(&self, i: usize) -> bool {
        match self.settings.page.items.iter().find(|it| it.slot == i) {
            Some(it) => it.w_mm >= it.h_mm,
            None => !self.settings.doc_portrait,
        }
    }

    fn doc_apply_detect(&mut self, i: usize, det: Option<([Pt; 4], f32)>, who: &str) {
        let land = self.doc_wants_landscape(i);
        let s = &mut self.doc.slots[i];
        let Some(img) = s.img.clone() else { return };
        match det {
            Some((q, score)) if score >= 0.80 => {
                s.quad = doc::order_quad(q);
                s.rot = doc::auto_rot(&s.quad, land);
                s.ai = None;
                s.info = format!("{who}: found (fit {:.0}%). Check the 4 corners and drag if needed.", score * 100.0);
            }
            Some((q, _)) => {
                s.quad = doc::order_quad(q);
                s.rot = doc::auto_rot(&s.quad, land);
                s.ai = None;
                s.info = format!("{who}: not sure about the edges - please drag the 4 corners onto the paper / card.");
            }
            None => {
                if s.info.is_empty() || who == "Auto" {
                    s.quad = doc::full_quad(img.width(), img.height());
                    s.rot = 0;
                }
                s.info = format!("{who}: nothing found. Drag the 4 corners onto the corners yourself.");
            }
        }
    }

    /// default size for a new item (settings combo)
    fn doc_target_mm(&self) -> (f32, f32) {
        let st = &self.settings;
        let (w, h) = if st.doc_size_idx == doc::DOC_CUSTOM { (st.doc_w_mm.max(10.0), st.doc_h_mm.max(10.0)) } else { (doc::DOC_SIZES[st.doc_size_idx].w_mm, doc::DOC_SIZES[st.doc_size_idx].h_mm) };
        if st.doc_portrait { (h, w) } else { (w, h) }
    }

    /// width / height of the photographed paper (after the chosen turn)
    fn doc_slot_aspect(&self, i: usize) -> f32 {
        let s = &self.doc.slots[i];
        let (w, h) = doc::quad_wh(&doc::rotate_quad(s.quad, s.rot));
        if w > 1.0 && h > 1.0 { w / h } else { 1.5 }
    }

    /// Everything that changes the geometry of the straightened picture of slot `i`.
    fn doc_geom_key(&self, i: usize) -> String {
        let s = &self.doc.slots[i];
        format!("{:?}|{}|{}{}", s.quad.map(|p| [p[0].round(), p[1].round()]), s.rot, s.flip_h as u8, s.flip_v as u8)
    }

    /// Straightened (+ cleaned) picture of slot `i` at exactly `w_mm` x `h_mm` and `dpi`.
    fn doc_render_slot(&self, i: usize, w_mm: f32, h_mm: f32, dpi: u32, full: bool, clean: bool) -> Option<RgbImage> {
        let s = &self.doc.slots[i];
        let (img, k) = if full { (s.img.clone()?, 1.0) } else { let (im, k) = s.small.as_ref()?; (im.clone(), *k) };
        let px = |mm: f32| (mm / 25.4 * dpi as f32).round().max(8.0) as u32;
        let (ow, oh) = (px(w_mm), px(h_mm));
        let mut base = match &s.ai {
            // an AI-cleaned picture is only reused while the corners are unchanged and the shape is the same
            Some((key, im)) if *key == self.doc_geom_key(i) && ((im.width() as f32 / im.height() as f32) / (ow as f32 / oh as f32) - 1.0).abs() < 0.02 => {
                if im.dimensions() == (ow, oh) { im.clone() } else { image::imageops::resize(im, ow, oh, FilterType::Lanczos3) }
            }
            _ => {
                let q = doc::rotate_quad(s.quad, s.rot).map(|p| [p[0] * k, p[1] * k]);
                doc::warp_quad(&img, q, ow, oh)
            }
        };
        if s.flip_h {
            base = image::imageops::flip_horizontal(&base);
        }
        if s.flip_v {
            base = image::imageops::flip_vertical(&base);
        }
        Some(if clean { doc::enhance_doc(&base, &s.opts) } else { base })
    }

    fn doc_item_render(&self, idx: usize, dpi: u32, full: bool) -> Option<RgbImage> {
        let it = self.settings.page.items.get(idx)?;
        self.doc_render_slot(it.slot, it.w_mm, it.h_mm, dpi, full, true)
    }

    fn doc_item_label(&self, idx: usize) -> String {
        let it = &self.settings.page.items[idx];
        let nth = self.settings.page.items[..=idx].iter().filter(|x| x.slot == it.slot).count();
        format!("{}{}", if it.slot == 0 { "Front" } else { "Back" }, if nth > 1 { format!(" #{nth}") } else { String::new() })
    }

    pub(super) fn doc_refresh(&mut self, ctx: &egui::Context) {
        // side preview of the active picture
        let i = self.doc.active;
        if i < 2 && self.doc.slots[i].img.is_some() {
            let s = &self.doc.slots[i];
            let ai_on = s.ai.as_ref().map(|a| a.0 == self.doc_geom_key(i)).unwrap_or(false);
            let (w, h) = self.settings.page.items.iter().find(|it| it.slot == i).map(|it| (it.w_mm, it.h_mm)).unwrap_or_else(|| self.doc_target_mm());
            let key = format!("{}|{:?}|{}|{}|{:.1}x{:.1}", self.doc_geom_key(i), s.opts, ai_on, s.compare, w, h);
            if s.res_key != key {
                let scale = 700.0 / w.max(h);
                let dpi = (scale * 25.4).round().max(30.0) as u32;
                if let Some(im) = self.doc_render_slot(i, w, h, dpi, false, !self.doc.slots[i].compare) {
                    self.doc.slots[i].res_tex = Some(tex_from(ctx, &format!("docres{i}"), &im));
                }
                self.doc.slots[i].res_key = key;
            }
        }
        // page items
        if self.doc.active == 2 {
            let n = self.settings.page.items.len();
            self.doc.item_tex.resize_with(n, || None);
            let dpi = if self.doc.page_drag.as_ref().map(|d| d.resize).unwrap_or(false) { 40 } else { 100 };
            for idx in 0..n {
                let it = self.settings.page.items[idx].clone();
                if self.doc.slots[it.slot].img.is_none() {
                    self.doc.item_tex[idx] = None;
                    continue;
                }
                let s = &self.doc.slots[it.slot];
                let ai_on = s.ai.as_ref().map(|a| a.0 == self.doc_geom_key(it.slot)).unwrap_or(false);
                let key = format!("{}|{:?}|{}|{:.1}x{:.1}|{}", self.doc_geom_key(it.slot), s.opts, ai_on, it.w_mm, it.h_mm, dpi);
                if self.doc.item_tex[idx].as_ref().map(|t| t.0 != key).unwrap_or(true) {
                    if let Some(im) = self.doc_item_render(idx, dpi, false) {
                        self.doc.item_tex[idx] = Some((key, tex_from(ctx, &format!("docitem{idx}"), &im)));
                    }
                }
            }
            let (pw, ph) = self.settings.page.paper_mm();
            self.doc.page_info = if n == 0 {
                "Open the front (and back) of the card first.".into()
            } else if !self.settings.page.fits() {
                "A card sticks out of the page - move it inside, or press 'Auto arrange'.".into()
            } else {
                format!("{n} item(s) on {pw:.0} x {ph:.0} mm paper, real size")
            };
        }
    }

    fn doc_ai_clear(&mut self, ctx: &egui::Context, i: usize) {
        let Some(img) = self.doc.slots[i].img.clone() else { return };
        let id = self.doc.ai_model.clone();
        let (wm, hm) = self.settings.page.items.iter().find(|it| it.slot == i).map(|it| (it.w_mm, it.h_mm)).unwrap_or_else(|| self.doc_target_mm());
        let dpi = self.settings.page.dpi;
        let px = |mm: f32| (mm / 25.4 * dpi as f32).round().max(8.0) as u32;
        let (ow, oh) = (px(wm), px(hm));
        let q = doc::rotate_quad(self.doc.slots[i].quad, self.doc.slots[i].rot);
        let key = self.doc_geom_key(i);
        self.doc.slots[i].busy = true;
        self.ai.cancel.store(false, Ordering::Relaxed);
        self.ai.prog = (0.0, "Starting...".into());
        let (tx, cancel, ctx) = (self.tx.clone(), self.ai.cancel.clone(), ctx.clone());
        std::thread::spawn(move || {
            let (tx2, ctx2) = (tx.clone(), ctx.clone());
            let prog = move |f: f32, t: &str| {
                let _ = tx2.send(Msg::AiProg(f, t.to_string()));
                ctx2.request_repaint();
            };
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let native = doc::quad_wh(&q).0;
                let inw = native.min(640.0).max(120.0).round() as u32;
                let inh = ((inw as f32) * oh as f32 / ow as f32).round().max(16.0) as u32;
                let base = doc::warp_quad(&img, q, inw, inh);
                let up = enhance::upscale(&base, &id, &prog, &cancel)?;
                Ok(image::imageops::resize(&up, ow, oh, FilterType::Lanczos3))
            }))
            .unwrap_or_else(|_| Err("AI engine crashed".into()));
            let _ = tx.send(Msg::DocAi(i, key, r));
            ctx.request_repaint();
        });
    }

    // ============================================================ document tool: panels
    pub(super) fn doc_left(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        card(ui, "1", "Card / document photos", true, |ui| {
            for i in 0..2 {
                let name = if i == 0 { "FRONT" } else { "BACK" };
                let on = self.doc.active == i;
                ui.horizontal(|ui| {
                    if let Some(t) = &self.doc.slots[i].tex {
                        let sz = t.size();
                        let h = 40.0;
                        ui.image((t.id(), vec2(h * sz[0] as f32 / sz[1] as f32, h)));
                    } else {
                        let (r, _) = ui.allocate_exact_size(vec2(60.0, 40.0), Sense::hover());
                        ui.painter().rect_stroke(r, cr(6), Stroke::new(1.0_f32, BORDER), StrokeKind::Inside);
                    }
                    ui.vertical(|ui| {
                        if chip(ui, name, on).clicked() {
                            self.doc.active = i;
                        }
                        ui.horizontal(|ui| {
                            if button(ui, if self.doc.slots[i].img.is_some() { "Replace" } else { "Open" }, Kind::Primary, vec2(0.0, 26.0)).clicked() {
                                self.doc_pick(ctx, i);
                            }
                            if self.doc.slots[i].img.is_some() && button(ui, "Remove", Kind::Ghost, vec2(0.0, 26.0)).clicked() {
                                self.doc_remove_slot(i);
                            }
                        });
                    });
                });
                if !self.doc.slots[i].name.is_empty() {
                    hint(ui, &self.doc.slots[i].name);
                }
                ui.add_space(4.0);
            }
            if button(ui, "Swap front / back", Kind::Ghost, vec2(ui.available_width(), 26.0)).clicked() {
                self.doc.slots.swap(0, 1);
                for it in self.settings.page.items.iter_mut() {
                    it.slot = 1 - it.slot;
                }
                self.doc.item_tex.clear();
            }
            hint(ui, "Front and back are completely independent: crop, turn and clean-up of one never change the other. You can also drag & drop pictures.");
        });

        let act = self.doc_act();
        let has = self.doc.slots[act].img.is_some();
        card(ui, "2", &format!("Straighten  -  {}", if act == 0 { "FRONT" } else { "BACK" }), true, |ui| {
            sub(ui, "Size of this picture on the page");
            let before = (self.settings.doc_size_idx, self.settings.doc_w_mm, self.settings.doc_h_mm, self.settings.doc_portrait);
            egui::ComboBox::from_id_salt("docsize").width(ui.available_width()).selected_text(doc::DOC_SIZES[self.settings.doc_size_idx].name).show_ui(ui, |ui| {
                for (i, d) in doc::DOC_SIZES.iter().enumerate() {
                    ui.selectable_value(&mut self.settings.doc_size_idx, i, d.name);
                }
            });
            if self.settings.doc_size_idx == doc::DOC_CUSTOM {
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut self.settings.doc_w_mm).range(20.0..=400.0).suffix(" mm"));
                    ui.label("x");
                    ui.add(egui::DragValue::new(&mut self.settings.doc_h_mm).range(20.0..=400.0).suffix(" mm"));
                });
            }
            ui.checkbox(&mut self.settings.doc_portrait, "Portrait (upright)");
            if before != (self.settings.doc_size_idx, self.settings.doc_w_mm, self.settings.doc_h_mm, self.settings.doc_portrait) {
                let (w, h) = self.doc_target_mm();
                for it in self.settings.page.items.iter_mut().filter(|it| it.slot == act) {
                    let (cx, cy) = (it.x_mm + it.w_mm / 2.0, it.y_mm + it.h_mm / 2.0);
                    it.w_mm = w;
                    it.h_mm = h;
                    it.x_mm = cx - w / 2.0;
                    it.y_mm = cy - h / 2.0;
                }
                self.settings.page.auto_arrange();
            }
            ui.add_space(2.0);
            ui.add_enabled_ui(has && !self.doc.slots[act].busy, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if button(ui, "Auto detect", Kind::Primary, vec2(0.0, 30.0)).on_hover_text("Finds the paper / card edges by colour").clicked() {
                        if let Some(img) = self.doc.slots[act].img.clone() {
                            let d = self.doc_classic(&img);
                            self.doc.slots[act].touched = false;
                            self.doc_apply_detect(act, d, "Auto");
                        }
                    }
                    if button(ui, "AI detect", Kind::Accent, vec2(0.0, 30.0)).on_hover_text("Uses the IS-Net AI model to find it").clicked() {
                        self.doc.slots[act].touched = false;
                        self.doc_detect_ai(ctx, act);
                    }
                    if button(ui, "Reset corners", Kind::Ghost, vec2(0.0, 30.0)).clicked() {
                        if let Some(img) = self.doc.slots[act].img.clone() {
                            self.doc.slots[act].quad = doc::full_quad(img.width(), img.height());
                            self.doc.slots[act].rot = 0;
                            self.doc.slots[act].touched = true;
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    sub(ui, "Turn");
                    if button(ui, "90\u{00B0}", Kind::Ghost, vec2(0.0, 26.0)).clicked() {
                        self.doc.slots[act].rot = (self.doc.slots[act].rot + 1) % 4;
                    }
                    if button(ui, "180\u{00B0}", Kind::Ghost, vec2(0.0, 26.0)).on_hover_text("Upside down").clicked() {
                        self.doc.slots[act].rot = (self.doc.slots[act].rot + 2) % 4;
                    }
                    if chip(ui, "Flip \u{2194}", self.doc.slots[act].flip_h).clicked() {
                        self.doc.slots[act].flip_h ^= true;
                    }
                    if chip(ui, "Flip \u{2195}", self.doc.slots[act].flip_v).clicked() {
                        self.doc.slots[act].flip_v ^= true;
                    }
                });
            });
            if self.doc.slots[act].busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    sub(ui, "Working...");
                });
            }
            if !self.doc.slots[act].info.is_empty() {
                hint(ui, &self.doc.slots[act].info);
            }
            hint(ui, "Drag the 4 round handles onto the corners (a loupe helps). The ORANGE handle becomes the top-left of the result.");
        });

        card(ui, "3", &format!("Clean up  -  {}", if act == 0 { "FRONT" } else { "BACK" }), true, |ui| {
            ui.horizontal_wrapped(|ui| {
                if button(ui, "Card preset", Kind::Ghost, vec2(0.0, 26.0)).clicked() {
                    self.doc.slots[act].opts = DocOpts::default();
                }
                if button(ui, "Paper document preset", Kind::Accent, vec2(0.0, 26.0)).on_hover_text("Removes shadows, whitens the paper, sharpens the text").clicked() {
                    self.doc.slots[act].opts = DocOpts { shadow: 0.9, even_light: 0.0, auto_levels: 0.8, contrast: 0.12, colour: 0.0, sharpen: 0.7, denoise: 0.25, brightness: 0.0, mode: 0, ..Default::default() };
                }
                if button(ui, "Old document restore", Kind::Accent, vec2(0.0, 26.0)).on_hover_text("Faded / yellow / dull paper: whitens, boosts contrast, sharpens the writing").clicked() {
                    let keep = (self.doc.slots[act].opts.rm_red, self.doc.slots[act].opts.rm_blue, self.doc.slots[act].opts.rm_pen, self.doc.slots[act].opts.rm_dots.clone());
                    self.doc.slots[act].opts = DocOpts { shadow: 0.8, even_light: 0.0, auto_levels: 1.0, contrast: 0.3, colour: -0.2, sharpen: 0.85, denoise: 0.35, brightness: 0.05, mode: 0, rm_red: keep.0, rm_blue: keep.1, rm_pen: keep.2, rm_dots: keep.3 };
                }
            });
            let o = &mut self.doc.slots[act].opts;
            ui.horizontal(|ui| {
                for (m, n) in [(0u8, "Colour"), (1, "Grey"), (2, "Scan B/W")] {
                    if chip(ui, n, o.mode == m).clicked() {
                        o.mode = m;
                    }
                }
            });
            slider_row(ui, "Remove shadow", &mut o.shadow, 0.0, 1.0, 100.0, "");
            slider_row(ui, "Even light", &mut o.even_light, 0.0, 1.0, 100.0, "");
            slider_row(ui, "Auto levels", &mut o.auto_levels, 0.0, 1.0, 100.0, "");
            slider_row(ui, "Brightness", &mut o.brightness, -1.0, 1.0, 100.0, "");
            slider_row(ui, "Contrast", &mut o.contrast, -0.5, 0.8, 100.0, "");
            slider_row(ui, "Colour", &mut o.colour, -1.0, 1.0, 100.0, "");
            slider_row(ui, "Sharpen", &mut o.sharpen, 0.0, 1.0, 100.0, "");
            slider_row(ui, "Reduce noise", &mut o.denoise, 0.0, 1.0, 100.0, "");
            sub(ui, "Remove seal / writing");
            slider_row(ui, "Red seal / ink", &mut o.rm_red, 0.0, 1.0, 100.0, "");
            slider_row(ui, "Blue seal / pen", &mut o.rm_blue, 0.0, 1.0, 100.0, "");
            slider_row(ui, "Any colour ink", &mut o.rm_pen, 0.0, 1.0, 100.0, "");
            let ndots = o.rm_dots.len();
            ui.horizontal_wrapped(|ui| {
                if chip(ui, "Eraser  (paint on the right picture)", self.doc.erase).on_hover_text("Rub over any writing, seal or mark in the right-hand picture to wipe it out").clicked() {
                    self.doc.erase ^= true;
                }
            });
            if self.doc.erase {
                ui.horizontal(|ui| {
                    sub(ui, "Eraser size");
                    ui.add(egui::Slider::new(&mut self.doc.erase_r, 0.005..=0.08).show_value(false));
                });
            }
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(ndots > 0, egui::Button::new("Undo last rub")).clicked() {
                    let n = self.doc.slots[act].opts.rm_dots.len().saturating_sub(30);
                    self.doc.slots[act].opts.rm_dots.truncate(n);
                }
                if ui.add_enabled(ndots > 0, egui::Button::new("Clear erased")).clicked() {
                    self.doc.slots[act].opts.rm_dots.clear();
                }
            });
            let copy = self.doc.slots[act].opts.clone();
            ui.horizontal_wrapped(|ui| {
                if button(ui, "Reset", Kind::Ghost, vec2(0.0, 26.0)).clicked() {
                    self.doc.slots[act].opts = DocOpts::default();
                }
                let other = 1 - act;
                if button(ui, &format!("Copy to {}", if other == 0 { "front" } else { "back" }), Kind::Ghost, vec2(0.0, 26.0)).on_hover_text("Only when YOU press it - normally the two sides stay separate").clicked() {
                    self.doc.slots[other].opts = copy;
                }
                if chip(ui, "Before", self.doc.slots[act].compare).on_hover_text("Show without clean-up").clicked() {
                    self.doc.slots[act].compare ^= true;
                }
            });
        });

        card(ui, "4", "AI clear  (optional)", false, |ui| {
            hint(ui, "Makes a low-quality / blurry photo clearer with an AI upscaler, then fits it to the size.");
            egui::ComboBox::from_id_salt("docai").width(ui.available_width()).selected_text(enhance::UPSCALERS.iter().find(|m| m.0 == self.doc.ai_model).map(|m| m.1).unwrap_or("?")).show_ui(ui, |ui| {
                for (id, label) in enhance::UPSCALERS {
                    ui.selectable_value(&mut self.doc.ai_model, id.to_string(), *label);
                }
            });
            let id = self.doc.ai_model.clone();
            let ready = self.model_need(ui, ctx, &id);
            if self.doc.slots[act].busy && self.ai.busy_doc_hint() {
                ui.add(egui::ProgressBar::new(self.ai.prog.0).corner_radius(cr(8)).text(self.ai.prog.1.clone()));
                if button(ui, "Cancel", Kind::Danger, vec2(80.0, 26.0)).clicked() {
                    self.ai.cancel.store(true, Ordering::Relaxed);
                }
            } else {
                ui.add_enabled_ui(has && ready && !self.doc.slots[act].busy, |ui| {
                    if full_button(ui, "AI clear this picture", Kind::Primary, 32.0).clicked() {
                        self.doc_ai_clear(ctx, act);
                    }
                });
            }
            if self.doc.slots[act].ai.is_some() {
                let on = self.doc.slots[act].ai.as_ref().map(|a| a.0 == self.doc_geom_key(act)).unwrap_or(false);
                hint(ui, if on { "AI clean-up is applied." } else { "Corners changed - press AI clear again." });
                if button(ui, "Remove AI clean-up", Kind::Ghost, vec2(0.0, 26.0)).clicked() {
                    self.doc.slots[act].ai = None;
                }
            }
        });
        ui.add_space(30.0);
    }

    pub(super) fn doc_right(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        card(ui, "", "Page", true, |ui| {
            let p = &mut self.settings.page;
            egui::ComboBox::from_id_salt("docpaper").width(ui.available_width()).selected_text(sheet::PAPERS[p.paper_idx].name).show_ui(ui, |ui| {
                for (i, pp) in sheet::PAPERS.iter().enumerate() {
                    ui.selectable_value(&mut p.paper_idx, i, pp.name);
                }
            });
            if p.paper_idx == sheet::CUSTOM_PAPER {
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut p.custom_w_mm).range(40.0..=1000.0).suffix(" mm"));
                    ui.label("x");
                    ui.add(egui::DragValue::new(&mut p.custom_h_mm).range(40.0..=1000.0).suffix(" mm"));
                });
            }
            ui.horizontal(|ui| {
                sub(ui, "Margin");
                ui.add(egui::DragValue::new(&mut p.margin_mm).range(0.0..=40.0).speed(0.2).suffix(" mm"));
                sub(ui, "Gap");
                ui.add(egui::DragValue::new(&mut p.gap_mm).range(0.0..=60.0).speed(0.2).suffix(" mm"));
            });
            ui.horizontal_wrapped(|ui| {
                if chip(ui, "Stack", p.stack).clicked() {
                    p.stack = true;
                }
                if chip(ui, "Side by side", !p.stack).clicked() {
                    p.stack = false;
                }
                if chip(ui, "Centre", !p.top_align).clicked() {
                    p.top_align = false;
                }
                if chip(ui, "Top", p.top_align).clicked() {
                    p.top_align = true;
                }
            });
            if button(ui, "Auto arrange all", Kind::Primary, vec2(ui.available_width(), 28.0)).on_hover_text("Puts every item in a neat stack / row").clicked() {
                p.auto_arrange();
            }
            ui.checkbox(&mut p.outline, "Thin outline around each item (cut guide)");
            ui.horizontal(|ui| {
                sub(ui, "Quality");
                for d in [200u32, 300, 600] {
                    if chip(ui, &format!("{d} dpi"), p.dpi == d).clicked() {
                        p.dpi = d;
                    }
                }
            });
            if !self.doc.page_info.is_empty() {
                egui::Frame::new().fill(Color32::from_rgb(0x2a, 0x1d, 0x08)).corner_radius(cr(8)).inner_margin(egui::Margin::symmetric(9, 5)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(&self.doc.page_info).font(brand::medium(12.0)).color(ORANGE));
                });
            }
            if button(ui, "Show page layout editor", Kind::Ghost, vec2(ui.available_width(), 28.0)).clicked() {
                self.doc.active = 2;
            }
        });

        card(ui, "", "Items on the page", true, |ui| {
            let n = self.settings.page.items.len();
            for idx in 0..n {
                let label = format!("{}   {:.1} x {:.1} mm", self.doc_item_label(idx), self.settings.page.items[idx].w_mm, self.settings.page.items[idx].h_mm);
                if chip(ui, &label, self.doc.sel == Some(idx)).clicked() {
                    self.doc.sel = Some(idx);
                    self.doc.active = 2;
                }
            }
            ui.horizontal_wrapped(|ui| {
                for (slot, nm) in [(0usize, "+ Front copy"), (1, "+ Back copy")] {
                    ui.add_enabled_ui(self.doc.slots[slot].img.is_some(), |ui| {
                        if button(ui, nm, Kind::Ghost, vec2(0.0, 26.0)).clicked() {
                            let (w, h) = self.settings.page.items.iter().find(|it| it.slot == slot).map(|it| (it.w_mm, it.h_mm)).unwrap_or_else(|| self.doc_target_mm());
                            self.settings.page.items.push(doc::PageItem { slot, x_mm: 10.0, y_mm: 10.0, w_mm: w, h_mm: h, lock: false });
                            self.settings.page.auto_arrange();
                            self.doc.sel = Some(self.settings.page.items.len() - 1);
                        }
                    });
                }
            });
            if let Some(idx) = self.doc.sel.filter(|i| *i < self.settings.page.items.len()) {
                let aspect = self.doc_slot_aspect(self.settings.page.items[idx].slot);
                let (pw, ph) = self.settings.page.paper_mm();
                let it = &mut self.settings.page.items[idx];
                ui.add_space(4.0);
                sub(ui, "Selected: exact position and size (mm)");
                egui::Grid::new("itemgrid").num_columns(4).spacing([6.0, 4.0]).show(ui, |ui| {
                    ui.label("X");
                    ui.add(egui::DragValue::new(&mut it.x_mm).speed(0.2).fixed_decimals(1));
                    ui.label("Y");
                    ui.add(egui::DragValue::new(&mut it.y_mm).speed(0.2).fixed_decimals(1));
                    ui.end_row();
                    ui.label("W");
                    ui.add(egui::DragValue::new(&mut it.w_mm).range(5.0..=1000.0).speed(0.2).fixed_decimals(1));
                    ui.label("H");
                    ui.add(egui::DragValue::new(&mut it.h_mm).range(5.0..=1000.0).speed(0.2).fixed_decimals(1));
                    ui.end_row();
                });
                ui.checkbox(&mut it.lock, "Keep shape when dragging corners");
                ui.horizontal_wrapped(|ui| {
                    if button(ui, "Centre \u{2194}", Kind::Ghost, vec2(0.0, 26.0)).clicked() {
                        it.x_mm = (pw - it.w_mm) / 2.0;
                    }
                    if button(ui, "Centre \u{2195}", Kind::Ghost, vec2(0.0, 26.0)).clicked() {
                        it.y_mm = (ph - it.h_mm) / 2.0;
                    }
                });
                let mut act = 0;
                ui.horizontal_wrapped(|ui| {
                    if button(ui, "Fit page (keep shape)", Kind::Primary, vec2(0.0, 26.0)).on_hover_text("Biggest size that fits the margins with the photographed paper's own shape").clicked() {
                        act = 1;
                    }
                    if button(ui, "Delete", Kind::Danger, vec2(0.0, 26.0)).clicked() {
                        act = 2;
                    }
                });
                if act == 1 {
                    self.settings.page.fit_item(idx, aspect);
                } else if act == 2 {
                    self.settings.page.items.remove(idx);
                    self.doc.sel = None;
                    self.doc.item_tex.clear();
                }
            }
            hint(ui, "In the page editor: drag a card to move it, drag the handles to resize. Arrow keys nudge by 0.5 mm (Shift = 5 mm).");
        });

        let any = self.doc.slots.iter().any(|s| s.img.is_some()) && !self.settings.page.items.is_empty();
        ui.add_enabled_ui(any, |ui| {
            card(ui, "", "Save & print", true, |ui| {
                if full_button(ui, "Print page", Kind::Accent, 40.0).on_hover_text("Opens the Windows print window (choose printer / copies), prints at real size").clicked() {
                    self.doc_print(ctx);
                }
                sub(ui, "Save the page as");
                let w = (ui.available_width() - 16.0) / 3.0;
                ui.horizontal(|ui| {
                    if button(ui, "PDF", Kind::Primary, vec2(w, 32.0)).clicked() { self.doc_save("pdf"); }
                    if button(ui, "JPG", Kind::Primary, vec2(w, 32.0)).clicked() { self.doc_save("jpg"); }
                    if button(ui, "PNG", Kind::Primary, vec2(w, 32.0)).clicked() { self.doc_save("png"); }
                });
                sub(ui, "Save each picture alone");
                let w2 = (ui.available_width() - 8.0) / 2.0;
                ui.horizontal(|ui| {
                    if button(ui, "Front JPG", Kind::Ghost, vec2(w2 - 2.0, 30.0)).clicked() { self.doc_save_card(0); }
                    if button(ui, "Back JPG", Kind::Ghost, vec2(w2 - 2.0, 30.0)).clicked() { self.doc_save_card(1); }
                });
                ui.checkbox(&mut self.settings.doc_keep_original, "Also keep a copy of the original photo(s)");
                let mut q = self.settings.jpeg_q as f32;
                if slider_row(ui, "JPG quality", &mut q, 60.0, 100.0, 1.0, "").changed() {
                    self.settings.jpeg_q = q as u8;
                }
            });
        });
        ui.add_space(20.0);
    }

    // ============================================================ document tool: export
    fn doc_page_full(&mut self) -> Option<(RgbImage, bool)> {
        let o = self.settings.page.clone();
        let mut imgs = Vec::new();
        for idx in 0..o.items.len() {
            match self.doc_item_render(idx, o.dpi, true) {
                Some(im) => imgs.push(im),
                None => {
                    self.error = Some("An item on the page has no picture. Open the picture or delete the item.".into());
                    return None;
                }
            }
        }
        if imgs.is_empty() {
            self.error = Some("Open at least one picture first.".into());
            return None;
        }
        Some((doc::compose_items(&o, &imgs), o.fits()))
    }

    /// the originals are never touched; optionally copy them next to the saved file
    fn doc_keep_originals(&self, saved: &Path, slots: &[usize]) {
        if !self.settings.doc_keep_original {
            return;
        }
        let (Some(dir), Some(stem)) = (saved.parent(), saved.file_stem().map(|s| s.to_string_lossy().into_owned())) else { return };
        for &i in slots {
            if let Some(src) = &self.doc.slots[i].path {
                let ext = src.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_else(|| "jpg".into());
                let _ = std::fs::copy(src, dir.join(format!("{stem}_original_{}.{ext}", if i == 0 { "front" } else { "back" })));
            }
        }
    }

    fn doc_print(&mut self, ctx: &egui::Context) {
        let Some((page, fits)) = self.doc_page_full() else { return };
        let (dpi, paper) = (self.settings.page.dpi, self.settings.page.paper_mm());
        self.print_now(ctx, vec![page], dpi, paper);
        if !fits {
            self.error = Some("A card sticks out of the page - it will be cut at the paper edge.".into());
        }
    }

    fn doc_save(&mut self, kind: &str) {
        let filter: (&str, &[&str]) = match kind {
            "pdf" => ("PDF", &["pdf"]),
            "png" => ("PNG", &["png"]),
            _ => ("JPEG", &["jpg", "jpeg"]),
        };
        let Some(path) = self.save_dialog(&format!("ID_card_page.{kind}"), filter) else { return };
        let Some((page, _)) = self.doc_page_full() else { return };
        let (dpi, q) = (self.settings.page.dpi, self.settings.jpeg_q);
        let bytes = match kind {
            "pdf" => sheet::encode_pdf(&[page], dpi, q),
            "png" => sheet::encode_png(&page, dpi),
            _ => sheet::encode_jpeg(&page, q, dpi),
        };
        self.status = match std::fs::write(&path, bytes) {
            Ok(()) => {
                let used: Vec<usize> = (0..2).filter(|i| self.settings.page.items.iter().any(|it| it.slot == *i)).collect();
                self.doc_keep_originals(&path, &used);
                format!("Saved {}", path.display())
            }
            Err(e) => format!("Could not save: {e}"),
        };
    }

    fn doc_save_card(&mut self, i: usize) {
        let dpi = self.settings.page.dpi;
        let (w, h) = self.settings.page.items.iter().find(|it| it.slot == i).map(|it| (it.w_mm, it.h_mm)).unwrap_or_else(|| self.doc_target_mm());
        let Some(card) = self.doc_render_slot(i, w, h, dpi, true, true) else {
            self.error = Some("That side has no picture yet.".into());
            return;
        };
        let nm = if i == 0 { "front" } else { "back" };
        if let Some(path) = self.save_dialog(&format!("ID_card_{nm}.jpg"), ("JPEG", &["jpg", "jpeg"])) {
            self.status = match std::fs::write(&path, sheet::encode_jpeg(&card, self.settings.jpeg_q, dpi)) {
                Ok(()) => {
                    self.doc_keep_originals(&path, &[i]);
                    format!("Saved {}", path.display())
                }
                Err(e) => format!("Could not save: {e}"),
            };
        }
    }

    // ============================================================ document tool: canvas
    pub(super) fn doc_canvas(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            for (i, n) in [(0usize, "Front"), (1, "Back"), (2, "Page layout")] {
                if chip(ui, n, self.doc.active == i).clicked() {
                    self.doc.active = i;
                    if i < 2 {
                        self.doc.last_slot = i;
                    }
                }
            }
        });
        ui.add_space(6.0);
        let area = ui.available_rect_before_wrap();
        if self.doc.active == 2 {
            self.doc_page_canvas(ui, area);
            return;
        }
        let i = self.doc.active;
        let split = area.min.x + area.width() * 0.60;
        let left = Rect::from_min_max(area.min, pos2(split - 5.0, area.max.y));
        let right = Rect::from_min_max(pos2(split + 5.0, area.min.y), area.max);
        self.doc_source(ui, left, i, ctx);
        let painter = ui.painter_at(right);
        self.panel_bg(&painter, right, if self.doc.slots[i].compare { "STRAIGHTENED  (before clean-up)" } else { "STRAIGHTENED + CLEANED" });
        self.doc_zoom_buttons(ui, left, i * 2);
        self.doc_zoom_buttons(ui, right, i * 2 + 1);
        if let Some(t) = self.doc.slots[i].res_tex.clone() {
            let sz = t.size();
            let inner = Rect::from_min_max(right.min + vec2(0.0, 30.0), right.max);
            let r = self.doc_view_rect(ui, inner, sz[0] as f32, sz[1] as f32, i * 2 + 1);
            let painter = painter.with_clip_rect(inner);
            painter.rect_filled(r.expand(2.0), cr(4), Color32::from_black_alpha(100));
            painter.image(t.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            if !self.doc.erase {
                let resp = ui.interact(r.intersect(inner), egui::Id::new(("docres", i)), Sense::click_and_drag());
                self.doc_pan_input(ui, &resp, i * 2 + 1);
                if resp.dragged_by(egui::PointerButton::Primary) && self.doc.zoom[i * 2 + 1] > 1.0 {
                    let d = ui.input(|i| i.pointer.delta());
                    self.doc.pan[i * 2 + 1] += d;
                }
            }
            if self.doc.erase {
                let resp = ui.interact(r.intersect(inner), egui::Id::new("doc_erase"), Sense::click_and_drag());
                self.doc_pan_input(ui, &resp, i * 2 + 1);
                if let Some(p) = resp.hover_pos() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::None);
                    painter.circle_stroke(p, self.doc.erase_r * r.width(), Stroke::new(1.5_f32, RED));
                }
                if resp.dragged() || resp.clicked() {
                    if let Some(p) = resp.interact_pointer_pos() {
                        let (x, y) = ((p.x - r.min.x) / r.width(), (p.y - r.min.y) / r.height());
                        let rad = self.doc.erase_r;
                        let dots = &mut self.doc.slots[i].opts.rm_dots;
                        let (x, y) = (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0));
                        let last = dots.last().copied();
                        let far = last.map(|d| ((d[0] - x).powi(2) * 4.0 + (d[1] - y).powi(2)).sqrt() > rad * 0.5).unwrap_or(true);
                        if far {
                            // fill the gap when the mouse moved fast
                            if let (Some(d), true) = (last, resp.dragged()) {
                                let dist = ((d[0] - x).powi(2) * 4.0 + (d[1] - y).powi(2)).sqrt();
                                let n = ((dist / (rad * 0.5)) as usize).min(60);
                                for k in 1..n {
                                    let f = k as f32 / n as f32;
                                    if d[2] == rad {
                                        dots.push([d[0] + (x - d[0]) * f, d[1] + (y - d[1]) * f, rad]);
                                    }
                                }
                            }
                            dots.push([x, y, rad]);
                        }
                    }
                }
                painter.text(pos2(right.center().x, right.max.y - 8.0), egui::Align2::CENTER_BOTTOM, "Eraser ON  -  rub over the writing / seal", egui::FontId::proportional(11.5), ORANGE);
            }
        }
    }

    /// Page editor: drag cards, resize with handles, nudge with the arrow keys.
    fn doc_page_canvas(&mut self, ui: &mut Ui, area: Rect) {
        let painter = ui.painter_at(area);
        self.panel_bg(&painter, area, "PAGE LAYOUT  -  drag cards to move, drag the handles to resize");
        let (pw, ph) = self.settings.page.paper_mm();
        let r = fit_rect(Rect::from_min_max(area.min + vec2(0.0, 30.0), area.max - vec2(0.0, 26.0)), pw, ph, 14.0);
        let scale = r.width() / pw;
        painter.rect_filled(r.expand(3.0), cr(2), Color32::from_black_alpha(90));
        painter.rect_filled(r, cr(0), Color32::WHITE);
        let m = self.settings.page.margin_mm;
        painter.rect_stroke(Rect::from_min_max(r.min + vec2(m * scale, m * scale), r.max - vec2(m * scale, m * scale)), cr(0), Stroke::new(1.0_f32, Color32::from_rgb(0xd8, 0xdd, 0xe6)), StrokeKind::Inside);
        let to_s = |x: f32, y: f32| pos2(r.min.x + x * scale, r.min.y + y * scale);
        let n = self.settings.page.items.len();
        // pictures
        for idx in 0..n {
            let it = &self.settings.page.items[idx];
            let rect = Rect::from_min_max(to_s(it.x_mm, it.y_mm), to_s(it.x_mm + it.w_mm, it.y_mm + it.h_mm));
            if let Some(Some((_, t))) = self.doc.item_tex.get(idx) {
                painter.image(t.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            } else {
                painter.rect_filled(rect, cr(0), Color32::from_rgb(0xee, 0xf1, 0xf6));
            }
            painter.rect_stroke(rect, cr(0), Stroke::new(1.0_f32, Color32::from_rgb(0x99, 0xa3, 0xb5)), StrokeKind::Outside);
            painter.text(rect.left_top() + vec2(4.0, 3.0), egui::Align2::LEFT_TOP, self.doc_item_label(idx), brand::medium(10.5), Color32::from_rgb(0x55, 0x60, 0x75));
        }
        let resp = ui.interact(r.expand(24.0), egui::Id::new("docpage"), Sense::click_and_drag());
        let sel = self.doc.sel.filter(|i| *i < n);
        // handles of the selected item
        let handles = |it: &doc::PageItem| -> [Pos2; 8] {
            let (x0, y0, x1, y1) = (it.x_mm, it.y_mm, it.x_mm + it.w_mm, it.y_mm + it.h_mm);
            let (xm, ym) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
            [to_s(x0, y0), to_s(xm, y0), to_s(x1, y0), to_s(x1, ym), to_s(x1, y1), to_s(xm, y1), to_s(x0, y1), to_s(x0, ym)]
        };
        if resp.drag_started() {
            if let Some(pp) = resp.interact_pointer_pos() {
                let mut hit: Option<(usize, Option<usize>)> = None;
                if let Some(si) = sel {
                    for (k, h) in handles(&self.settings.page.items[si]).iter().enumerate() {
                        if h.distance(pp) < 11.0 {
                            hit = Some((si, Some(k)));
                            break;
                        }
                    }
                }
                if hit.is_none() {
                    for idx in (0..n).rev() {
                        let it = &self.settings.page.items[idx];
                        if Rect::from_min_max(to_s(it.x_mm, it.y_mm), to_s(it.x_mm + it.w_mm, it.y_mm + it.h_mm)).contains(pp) {
                            hit = Some((idx, None));
                            break;
                        }
                    }
                }
                match hit {
                    Some((idx, hd)) => {
                        self.doc.sel = Some(idx);
                        let it = self.settings.page.items[idx].clone();
                        self.doc.page_drag = Some(PageDrag { item: idx, handle: hd, start: pp, orig: it, resize: hd.is_some() });
                    }
                    None => {
                        self.doc.sel = None;
                        self.doc.page_drag = None;
                    }
                }
            }
        }
        if resp.clicked() {
            if let Some(pp) = resp.interact_pointer_pos() {
                self.doc.sel = (0..n).rev().find(|&idx| {
                    let it = &self.settings.page.items[idx];
                    Rect::from_min_max(to_s(it.x_mm, it.y_mm), to_s(it.x_mm + it.w_mm, it.y_mm + it.h_mm)).contains(pp)
                });
            }
        }
        let mut guides: Vec<(bool, f32)> = vec![];
        if resp.dragged() {
            if let (Some(d), Some(pp)) = (self.doc.page_drag.clone(), resp.interact_pointer_pos()) {
                let (dx, dy) = ((pp.x - d.start.x) / scale, (pp.y - d.start.y) / scale);
                let o = d.orig.clone();
                let mut it = o.clone();
                match d.handle {
                    None => {
                        it.x_mm = o.x_mm + dx;
                        it.y_mm = o.y_mm + dy;
                        // snap to page centre lines and margins
                        let snap = 1.6;
                        let (cx, cy) = (it.x_mm + it.w_mm / 2.0, it.y_mm + it.h_mm / 2.0);
                        if (cx - pw / 2.0).abs() < snap {
                            it.x_mm = pw / 2.0 - it.w_mm / 2.0;
                            guides.push((true, pw / 2.0));
                        }
                        if (cy - ph / 2.0).abs() < snap {
                            it.y_mm = ph / 2.0 - it.h_mm / 2.0;
                            guides.push((false, ph / 2.0));
                        }
                        if (it.x_mm - m).abs() < snap {
                            it.x_mm = m;
                            guides.push((true, m));
                        }
                        if (it.x_mm + it.w_mm - (pw - m)).abs() < snap {
                            it.x_mm = pw - m - it.w_mm;
                            guides.push((true, pw - m));
                        }
                        if (it.y_mm - m).abs() < snap {
                            it.y_mm = m;
                            guides.push((false, m));
                        }
                        if (it.y_mm + it.h_mm - (ph - m)).abs() < snap {
                            it.y_mm = ph - m - it.h_mm;
                            guides.push((false, ph - m));
                        }
                    }
                    Some(h) => {
                        // handle order: TL, T, TR, R, BR, B, BL, L
                        let left = matches!(h, 0 | 6 | 7);
                        let right = matches!(h, 2 | 3 | 4);
                        let top = matches!(h, 0 | 1 | 2);
                        let bottom = matches!(h, 4 | 5 | 6);
                        let (mut x0, mut y0, mut x1, mut y1) = (o.x_mm, o.y_mm, o.x_mm + o.w_mm, o.y_mm + o.h_mm);
                        if left { x0 += dx; }
                        if right { x1 += dx; }
                        if top { y0 += dy; }
                        if bottom { y1 += dy; }
                        let min = 8.0;
                        if x1 - x0 < min { if left { x0 = x1 - min } else { x1 = x0 + min } }
                        if y1 - y0 < min { if top { y0 = y1 - min } else { y1 = y0 + min } }
                        if o.lock {
                            let a = o.w_mm / o.h_mm.max(0.1);
                            let corner = (left || right) && (top || bottom);
                            if corner {
                                let w = x1 - x0;
                                let hh = w / a;
                                if top { y0 = y1 - hh } else { y1 = y0 + hh }
                            } else if left || right {
                                let hh = (x1 - x0) / a;
                                let cy = (o.y_mm + o.y_mm + o.h_mm) / 2.0;
                                y0 = cy - hh / 2.0;
                                y1 = cy + hh / 2.0;
                            } else {
                                let w = (y1 - y0) * a;
                                let cx = (o.x_mm + o.x_mm + o.w_mm) / 2.0;
                                x0 = cx - w / 2.0;
                                x1 = cx + w / 2.0;
                            }
                        }
                        it.x_mm = x0;
                        it.y_mm = y0;
                        it.w_mm = x1 - x0;
                        it.h_mm = y1 - y0;
                    }
                }
                if let Some(slot) = self.settings.page.items.get_mut(d.item) {
                    *slot = it;
                }
            }
        }
        if resp.drag_stopped() {
            self.doc.page_drag = None;
        }
        // cursor hints
        if let (Some(pp), Some(si)) = (resp.hover_pos(), sel) {
            if handles(&self.settings.page.items[si]).iter().any(|h| h.distance(pp) < 11.0) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
            }
        }
        // guides + selection
        for (vertical, v) in guides {
            let (a, b) = if vertical { (to_s(v, 0.0), to_s(v, ph)) } else { (to_s(0.0, v), to_s(pw, v)) };
            painter.line_segment([a, b], Stroke::new(1.0_f32, ORANGE));
        }
        if let Some(si) = self.doc.sel.filter(|i| *i < self.settings.page.items.len()) {
            let it = &self.settings.page.items[si];
            let rect = Rect::from_min_max(to_s(it.x_mm, it.y_mm), to_s(it.x_mm + it.w_mm, it.y_mm + it.h_mm));
            painter.rect_stroke(rect, cr(0), Stroke::new(2.0_f32, ORANGE), StrokeKind::Outside);
            for h in handles(it) {
                painter.rect_filled(Rect::from_center_size(h, vec2(11.0, 11.0)), cr(2), BLUE);
                painter.rect_stroke(Rect::from_center_size(h, vec2(11.0, 11.0)), cr(2), Stroke::new(1.5_f32, Color32::WHITE), StrokeKind::Inside);
            }
        }
        // keyboard: arrows nudge, Delete removes
        if ui.memory(|mm| mm.focused().is_none()) {
            if let Some(si) = self.doc.sel.filter(|i| *i < self.settings.page.items.len()) {
                let (mut dx, mut dy, mut del) = (0.0f32, 0.0f32, false);
                ui.input(|i| {
                    let step = if i.modifiers.shift { 5.0 } else { 0.5 };
                    if i.key_pressed(egui::Key::ArrowLeft) { dx -= step; }
                    if i.key_pressed(egui::Key::ArrowRight) { dx += step; }
                    if i.key_pressed(egui::Key::ArrowUp) { dy -= step; }
                    if i.key_pressed(egui::Key::ArrowDown) { dy += step; }
                    del = i.key_pressed(egui::Key::Delete);
                });
                if del {
                    self.settings.page.items.remove(si);
                    self.doc.sel = None;
                    self.doc.item_tex.clear();
                } else if dx != 0.0 || dy != 0.0 {
                    self.settings.page.items[si].x_mm += dx;
                    self.settings.page.items[si].y_mm += dy;
                }
            }
        }
        painter.text(pos2(area.center().x, area.max.y - 14.0), egui::Align2::CENTER_CENTER, &self.doc.page_info, brand::medium(12.0), MUTED);
    }

    /// Where the picture is drawn, with mouse-wheel zoom around the cursor.
    fn doc_view_rect(&mut self, ui: &Ui, inner: Rect, iw: f32, ih: f32, k: usize) -> Rect {
        let base = fit_rect(inner, iw, ih, 12.0);
        let (mut z, mut pan) = (self.doc.zoom[k], self.doc.pan[k]);
        if ui.rect_contains_pointer(inner) {
            let sd = ui.input(|i| i.smooth_scroll_delta.y);
            if sd.abs() > 0.1 {
                if let Some(p) = ui.input(|i| i.pointer.hover_pos()) {
                    let draw = Rect::from_center_size(base.center() + pan, base.size() * z);
                    let nz = (z * (1.0 + sd * 0.0025)).clamp(1.0, 16.0);
                    let rel = p - draw.center();
                    let f = nz / z;
                    pan = p - rel * f - base.center();
                    z = nz;
                }
            }
        }
        if z <= 1.001 {
            z = 1.0;
            pan = Vec2::ZERO;
        } else {
            let lim = (base.size() * z * 0.5 + inner.size() * 0.3).max(Vec2::ZERO);
            pan = pan.clamp(-lim, lim);
        }
        self.doc.zoom[k] = z;
        self.doc.pan[k] = pan;
        Rect::from_center_size(base.center() + pan, base.size() * z)
    }

    /// right / middle mouse drag moves a zoomed picture
    fn doc_pan_input(&mut self, ui: &Ui, resp: &egui::Response, k: usize) {
        if resp.hovered() && ui.input(|i| i.pointer.secondary_down() || i.pointer.middle_down()) && self.doc.zoom[k] > 1.0 {
            let d = ui.input(|i| i.pointer.delta());
            self.doc.pan[k] += d;
        }
    }

    /// small  -  +  Fit  buttons in the top-right corner of a picture panel
    fn doc_zoom_buttons(&mut self, ui: &mut Ui, area: Rect, k: usize) {
        let w = 34.0;
        let y = area.min.y + 4.0;
        let x0 = area.max.x - 3.0 * w - 22.0;
        let zoom_by = |this: &mut Self, f: f32| {
            let z = (this.doc.zoom[k] * f).clamp(1.0, 16.0);
            this.doc.pan[k] *= z / this.doc.zoom[k];
            this.doc.zoom[k] = z;
        };
        if ui.put(Rect::from_min_size(pos2(x0, y), vec2(w - 4.0, 22.0)), egui::Button::new("-").corner_radius(cr(6))).on_hover_text("Zoom out  (or mouse wheel)").clicked() {
            zoom_by(self, 1.0 / 1.4);
        }
        if ui.put(Rect::from_min_size(pos2(x0 + w, y), vec2(w - 4.0, 22.0)), egui::Button::new("+").corner_radius(cr(6))).on_hover_text("Zoom in  (or mouse wheel)").clicked() {
            zoom_by(self, 1.4);
        }
        if ui.put(Rect::from_min_size(pos2(x0 + 2.0 * w, y), vec2(w + 12.0, 22.0)), egui::Button::new("Fit").corner_radius(cr(6))).on_hover_text("Show the whole picture").clicked() {
            self.doc.zoom[k] = 1.0;
            self.doc.pan[k] = Vec2::ZERO;
        }
    }

    fn doc_source(&mut self, ui: &mut Ui, area: Rect, i: usize, ctx: &egui::Context) {
        let painter = ui.painter_at(area);
        self.panel_bg(&painter, area, "PHOTO  -  DRAG THE 4 CORNERS ONTO THE CARD");
        let (Some(img), Some(tex)) = (self.doc.slots[i].img.clone(), self.doc.slots[i].tex.clone()) else {
            painter.text(area.center() - vec2(0.0, 20.0), egui::Align2::CENTER_CENTER, if i == 0 { "Open the FRONT of the card" } else { "Open the BACK of the card" }, brand::semibold(16.0), TEXT);
            painter.text(area.center() + vec2(0.0, 6.0), egui::Align2::CENTER_CENTER, "or drag & drop a picture here", brand::medium(12.5), MUTED);
            let r = Rect::from_center_size(area.center() + vec2(0.0, 56.0), vec2(170.0, 36.0));
            let resp = ui.put(r, egui::Button::new(RichText::new("Open picture").font(brand::medium(13.5)).color(Color32::from_rgb(0x1a, 0x10, 0x00))).fill(ORANGE).corner_radius(cr(9)));
            if resp.clicked() {
                self.doc_pick(ctx, i);
            }
            return;
        };
        let (iw, ih) = (img.width() as f32, img.height() as f32);
        let inner = Rect::from_min_max(area.min + vec2(0.0, 30.0), area.max);
        let draw = self.doc_view_rect(ui, inner, iw, ih, i * 2);
        let painter = painter.with_clip_rect(inner);
        painter.image(tex.id(), draw, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        let scale = draw.width() / iw;
        let resp = ui.interact(draw.intersect(inner), egui::Id::new(("docsrc", i)), Sense::click_and_drag());
        self.doc_pan_input(ui, &resp, i * 2);
        let to_s = |p: Pt| draw.min + vec2(p[0] * scale, p[1] * scale);
        let mut quad = self.doc.slots[i].quad;
        let near = |pos: Pos2, quad: &[Pt; 4]| -> Option<usize> {
            let mut best = None;
            let mut bd = 28.0;
            for (k, p) in quad.iter().enumerate() {
                let d = to_s(*p).distance(pos);
                if d < bd {
                    bd = d;
                    best = Some(k);
                }
            }
            best
        };
        if resp.drag_started() {
            if let Some(pp) = resp.interact_pointer_pos() {
                self.doc.drag = near(pp, &quad);
            }
        }
        if resp.dragged_by(egui::PointerButton::Primary) {
            if let (Some(k), Some(pp)) = (self.doc.drag, resp.interact_pointer_pos()) {
                quad[k] = [((pp.x - draw.min.x) / scale).clamp(0.0, iw), ((pp.y - draw.min.y) / scale).clamp(0.0, ih)];
                self.doc.slots[i].quad = quad;
                self.doc.slots[i].touched = true;
            } else if self.doc.drag.is_none() && self.doc.zoom[i * 2] > 1.0 {
                let d = ui.input(|i| i.pointer.delta());
                self.doc.pan[i * 2] += d;
            }
        }
        if resp.drag_stopped() {
            self.doc.drag = None;
        }
        if let Some(pp) = resp.hover_pos() {
            if near(pp, &quad).is_some() || self.doc.drag.is_some() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
        }
        // outline + handles
        let pts: Vec<Pos2> = quad.iter().map(|p| to_s(*p)).collect();
        painter.add(egui::Shape::closed_line(pts.clone(), Stroke::new(2.0_f32, ORANGE)));
        let first = doc::rotate_quad(quad, self.doc.slots[i].rot)[0];
        for (k, p) in pts.iter().enumerate() {
            let is_first = to_s(first).distance(*p) < 0.5;
            let col = if is_first { ORANGE } else { BLUE };
            painter.circle_filled(*p, 9.0, Color32::from_black_alpha(130));
            painter.circle_filled(*p, 7.0, col);
            painter.circle_stroke(*p, 7.0, Stroke::new(1.5_f32, Color32::WHITE));
            if self.doc.drag == Some(k) {
                painter.circle_stroke(*p, 12.0, Stroke::new(1.5_f32, Color32::WHITE));
            }
        }
        // loupe while dragging
        if let Some(k) = self.doc.drag {
            let c = quad[k];
            let half = (iw.min(ih) * 0.04).max(24.0);
            let uv = Rect::from_min_max(pos2(((c[0] - half) / iw).max(0.0), ((c[1] - half) / ih).max(0.0)), pos2(((c[0] + half) / iw).min(1.0), ((c[1] + half) / ih).min(1.0)));
            let on_left = to_s(c).x < area.center().x;
            let lr = Rect::from_min_size(if on_left { pos2(area.max.x - 176.0, area.min.y + 40.0) } else { pos2(area.min.x + 14.0, area.min.y + 40.0) }, vec2(160.0, 160.0));
            painter.rect_filled(lr.expand(3.0), cr(8), Color32::from_black_alpha(200));
            painter.image(tex.id(), lr, uv, Color32::WHITE);
            painter.line_segment([lr.center() - vec2(10.0, 0.0), lr.center() + vec2(10.0, 0.0)], Stroke::new(1.0_f32, ORANGE));
            painter.line_segment([lr.center() - vec2(0.0, 10.0), lr.center() + vec2(0.0, 10.0)], Stroke::new(1.0_f32, ORANGE));
            painter.rect_stroke(lr, cr(8), Stroke::new(1.5_f32, ORANGE), StrokeKind::Inside);
        }
        if self.doc.slots[i].busy {
            painter.text(area.min + vec2(14.0, 34.0), egui::Align2::LEFT_TOP, "Working...", brand::semibold(12.5), ORANGE);
        }
    }
}

impl AiState {
    /// the shared progress bar belongs to the document tool only when the photo AI is idle
    fn busy_doc_hint(&self) -> bool {
        !self.busy
    }
}

// ================================================================== AI worker
fn run_ai(
    src: Arc<RgbImage>,
    alpha: Option<Arc<GrayImage>>,
    edit: Edit,
    up: Option<String>,
    face: Option<(String, f32)>,
    cancel: &AtomicBool,
    prog: &dyn Fn(f32, &str),
) -> Result<AiOut, String> {
    let (sw, _sh) = src.dimensions();
    let ch = edit.crop_h();
    let mut alpha_new: Option<GrayImage> = None;
    let mut crop_cw = None;
    let work: RgbImage;
    let (center, frame_h);
    if let Some(id) = &up {
        let cap = if id.starts_with("real_esrgan") { 500 } else { 1400 };
        let sized = |m: f32| (((edit.cw * m).round().max(16.0)) as u32, ((ch * m).round().max(16.0)) as u32);
        let mut m = 1.3f32;
        let (mut rw, mut rh) = sized(m);
        let (mut reg, cov) = crate::imgops::crop_rgb(&src, edit.cx, edit.cy, rw as f32, rh as f32, edit.angle, rw, rh);
        if cov.pixels().any(|p| p[0] < 250) {
            m = 1.0;
            (rw, rh) = sized(m);
            reg = crate::imgops::crop_rgb(&src, edit.cx, edit.cy, rw as f32, rh as f32, edit.angle, rw, rh).0;
        }
        if rw.max(rh) > cap {
            return Err(format!(
                "The crop area is already {rw} x {rh} px. AI upscaling is meant for small / blurry photos (limit {cap} px for this model).\nUse 'Restore face' only, a faster model, or make the crop frame smaller."
            ));
        }
        let top = if face.is_some() { 0.8 } else { 1.0 };
        let upimg = enhance::upscale(&reg, id, &|f, t| prog(f * top, t), cancel)?;
        let s = upimg.width() as f32 / rw as f32;
        if let Some(a) = &alpha {
            let k = (1536.0 / upimg.width().max(upimg.height()) as f32).min(1.0);
            let (aw, ah) = (((upimg.width() as f32 * k) as u32).max(8), ((upimg.height() as f32 * k) as u32).max(8));
            alpha_new = Some(crate::imgops::crop_gray(a, sw, edit.cx, edit.cy, rw as f32, rh as f32, edit.angle, aw, ah));
        }
        crop_cw = Some(edit.cw * s);
        center = (upimg.width() as f32 / 2.0, upimg.height() as f32 / 2.0);
        frame_h = ch * s;
        work = upimg;
    } else {
        work = (*src).clone();
        center = (edit.cx, edit.cy);
        frame_h = ch;
    }
    let mut work = work;
    if let Some((fid, strength)) = &face {
        let head_alpha: Option<&GrayImage> = alpha_new.as_ref().or(if up.is_none() { alpha.as_deref() } else { None });
        let fb = match head_alpha.and_then(head::estimate) {
            Some(hd) => {
                let f = work.width() as f32 / head_alpha.unwrap().width() as f32;
                let hh = (hd.chin_y - hd.crown_y) * f;
                enhance::FaceBox { cx: hd.cx * f, cy: hd.crown_y * f + hh * 0.52, side: hh * 1.45 }
            }
            None => enhance::FaceBox { cx: center.0, cy: center.1 - frame_h * 0.03, side: frame_h * 0.95 },
        };
        let base = if up.is_some() { 0.8 } else { 0.0 };
        work = enhance::restore_face(&work, fb, fid, *strength, &|f, t| prog(base + (1.0 - base) * f, t), cancel)?;
    }
    Ok(AiOut { img: work, alpha: alpha_new, crop_cw, region: up.is_some() })
}
