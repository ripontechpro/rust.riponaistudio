//! Multi-customer batch print: import many customers' photos, set copies for each, and lay all of them
//! on as few sheets as possible. Every customer keeps an independent edit state.
use super::duet::Stash;
use super::*;
use crate::batch;
use std::hash::{Hash, Hasher};

#[derive(Clone, PartialEq)]
pub(super) enum CState {
    Loading,
    Queued,
    Working,
    Ready,
    Failed(String),
}

pub(super) struct Cust {
    pub id: u64,
    pub name: String,
    pub path: Option<PathBuf>,
    pub src: Option<Arc<RgbImage>>,
    pub tex: Option<TextureHandle>,
    pub alpha: Option<Arc<GrayImage>>,
    pub edit: Edit,
    pub copies: u32,
    pub state: CState,
    /// finished photo (with caption) at the output DPI; None = needs rendering
    pub photo: Option<RgbImage>,
    pub note: String,
    /// this customer has an own size (not following the global size)
    pub own: bool,
}

pub(super) struct BatchState {
    pub items: Vec<Cust>,
    pub sel: Option<usize>,
    pub landscape: bool,
    pub optimise: bool,
    pub page: usize,
    pub tex: Vec<TextureHandle>,
    pub key: Option<u64>,
    pub info: String,
    pub next_id: u64,
    pub seg_busy: bool,
    pub plan: batch::Plan,
    /// the Edit-photo workspace that was open before a customer photo was sent to the editor
    pub saved: Option<Stash>,
    pub editing: Option<usize>,
    pub default_copies: u32,
}
impl Default for BatchState {
    fn default() -> Self {
        BatchState { items: Vec::new(), sel: None, landscape: false, optimise: true, page: 0, tex: Vec::new(), key: None, info: String::new(), next_id: 1, seg_busy: false, plan: batch::Plan::default(), saved: None, editing: None, default_copies: 2 }
    }
}

impl App {
    fn cust_idx(&self, id: u64) -> Option<usize> {
        self.batch.items.iter().position(|c| c.id == id)
    }

    // ------------------------------------------------------------ import
    pub(super) fn batch_pick(&mut self, ctx: &egui::Context) {
        let mut d = rfd::FileDialog::new().add_filter("Photos", &["jpg", "jpeg", "png", "bmp", "webp"]);
        if !self.settings.last_dir.is_empty() {
            d = d.set_directory(&self.settings.last_dir);
        }
        if let Some(ps) = d.pick_files() {
            self.batch_import(ctx, ps);
        }
    }

    pub(super) fn batch_import(&mut self, ctx: &egui::Context, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        if let Some(d) = paths[0].parent() {
            self.settings.last_dir = d.to_string_lossy().into();
        }
        let mut jobs = Vec::new();
        for p in paths {
            let id = self.batch.next_id;
            self.batch.next_id += 1;
            let name = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| format!("Customer {id}"));
            self.batch.items.push(Cust { id, name, path: Some(p.clone()), src: None, tex: None, alpha: None, edit: self.edit.clone(), copies: self.batch.default_copies, state: CState::Loading, photo: None, note: String::new(), own: false });
            jobs.push((id, p));
        }
        self.batch.sel.get_or_insert(0);
        let (tx, ctx2) = (self.tx.clone(), ctx.clone());
        // one worker loads them one after the other (keeps memory low and the window responsive)
        std::thread::spawn(move || {
            for (id, p) in jobs {
                let r = std::panic::catch_unwind(|| load_image(&p)).unwrap_or_else(|_| Err("The picture could not be read (decoder crashed)".into()));
                let _ = tx.send(Msg::BatchLoaded(id, r));
                ctx2.request_repaint();
            }
        });
        self.status = "Loading customer photos...".into();
    }

    fn batch_new_edit(&self, w: u32, h: u32) -> Edit {
        let mut e = self.edit.clone();
        e.duet = false;
        e.duet_active = 0;
        e.spots.clear();
        e.strokes.clear();
        e.cuts.clear();
        e.reset_crop(w, h);
        e
    }

    pub(super) fn batch_msg(&mut self, ctx: &egui::Context, m: Msg) {
        match m {
            Msg::BatchLoaded(id, r) => {
                let Some(i) = self.cust_idx(id) else { return };
                match r {
                    Ok(img) => {
                        let (w, h) = img.dimensions();
                        let k = (400.0 / w.max(h) as f32).min(1.0);
                        let th = image::imageops::resize(&img, ((w as f32 * k) as u32).max(1), ((h as f32 * k) as u32).max(1), image::imageops::FilterType::Triangle);
                        let ci = egui::ColorImage::from_rgb([th.width() as usize, th.height() as usize], th.as_raw());
                        let tex = ctx.load_texture(format!("cust{id}"), ci, TextureOptions::LINEAR);
                        let e = self.batch_new_edit(w, h);
                        let want_ai = self.settings.auto_bg && seg::is_installed(&self.settings.model_key);
                        let c = &mut self.batch.items[i];
                        c.tex = Some(tex);
                        c.src = Some(Arc::new(img));
                        c.edit = e;
                        c.state = if want_ai { CState::Queued } else { CState::Ready };
                        if !want_ai && self.settings.auto_bg {
                            c.note = "No background AI model installed - open 'AI Models'".into();
                        }
                        c.photo = None;
                    }
                    Err(e) => {
                        self.batch.items[i].state = CState::Failed(e);
                    }
                }
                self.batch_pump(ctx);
            }
            Msg::BatchSeg(id, r) => {
                self.batch.seg_busy = false;
                if let Some(i) = self.cust_idx(id) {
                    match r {
                        Ok(a) => {
                            let c = &mut self.batch.items[i];
                            if let Some(s) = &c.src {
                                let ok = c.edit.auto_crop(&a, s.width(), s.height());
                                c.note = if ok { String::new() } else { "Head not found - adjust in Edit".into() };
                            }
                            c.alpha = Some(Arc::new(a));
                            c.state = CState::Ready;
                            c.photo = None;
                        }
                        Err(e) => {
                            let c = &mut self.batch.items[i];
                            c.state = CState::Ready; // still printable, just without background removal
                            c.note = format!("Background AI failed: {e}");
                            c.photo = None;
                        }
                    }
                }
                self.batch_pump(ctx);
            }
            _ => {}
        }
    }

    /// start background removal for the next queued customer (one at a time)
    fn batch_pump(&mut self, ctx: &egui::Context) {
        if self.batch.seg_busy {
            return;
        }
        let Some(i) = self.batch.items.iter().position(|c| c.state == CState::Queued) else { return };
        let c = &mut self.batch.items[i];
        let Some(src) = c.src.clone() else { return };
        c.state = CState::Working;
        let (id, key, tx, ctx2) = (c.id, self.settings.model_key.clone(), self.tx.clone(), ctx.clone());
        self.batch.seg_busy = true;
        std::thread::spawn(move || {
            let r = std::panic::catch_unwind(|| seg::segment(&src, &key, 1536)).unwrap_or_else(|_| Err("AI engine crashed".into()));
            let _ = tx.send(Msg::BatchSeg(id, r));
            ctx2.request_repaint();
        });
    }

    // ------------------------------------------------------------ per customer
    fn cust_resize(&mut self, i: usize, size_idx: usize, w: f32, h: f32) {
        let c = &mut self.batch.items[i];
        c.edit.size_idx = size_idx;
        c.edit.custom_w_mm = w;
        c.edit.custom_h_mm = h;
        if let Some(s) = &c.src {
            let (sw, sh) = (s.width(), s.height());
            match &c.alpha {
                Some(a) => {
                    if !c.edit.auto_crop(a, sw, sh) {
                        c.edit.reset_crop(sw, sh);
                    }
                }
                None => c.edit.reset_crop(sw, sh),
            }
        }
        c.photo = None;
    }

    fn cust_render(&mut self, i: usize) {
        let dpi = self.settings.dpi;
        let c = &mut self.batch.items[i];
        let Some(src) = c.src.clone() else { return };
        c.edit.dpi = dpi;
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let p = render_photo(&src, c.alpha.as_deref(), &c.edit, None);
            add_caption(&p, &c.edit)
        }));
        match r {
            Ok(p) => c.photo = Some(p),
            Err(_) => {
                c.state = CState::Failed("Could not process this photo".into());
            }
        }
    }

    /// Send one customer to the full editor. "Done" in the tab bar brings the changes back.
    fn cust_edit(&mut self, ctx: &egui::Context, i: usize) {
        if self.batch.editing.is_some() {
            self.batch_commit_edit();
        }
        let c = &self.batch.items[i];
        let Some(src) = c.src.clone() else { return };
        // a sharp preview of THIS customer's current picture (it may have been upscaled before)
        let (w, h) = src.dimensions();
        let k = (1600.0 / w.max(h) as f32).min(1.0);
        let prev = if k < 1.0 { image::imageops::resize(&*src, (w as f32 * k) as u32, (h as f32 * k) as u32, image::imageops::FilterType::Triangle) } else { (*src).clone() };
        let tex = Some(super::extra::tex_from(ctx, "src", &prev));
        self.batch.saved = Some(Stash { src: self.src.clone(), src_tex: self.src_tex.clone(), src_path: self.src_path.clone(), alpha: self.alpha.clone(), alpha_ver: self.alpha_ver, edit: self.edit.clone() });
        self.src = Some(src);
        self.src_tex = tex;
        self.src_path = c.path.clone();
        self.alpha = c.alpha.clone();
        self.alpha_ver += 1;
        self.img_id += 1;
        self.edit = c.edit.clone();
        self.other = None;
        self.other_img = None;
        self.ai.undo = None; // "Undo AI enhance" must never restore another customer's picture
        self.photo = None;
        self.overlay.clear();
        self.tool = Tool::None;
        self.last_key = None;
        self.last_aspect = self.edit.aspect();
        self.hist_reset();
        self.batch.editing = Some(i);
        self.view = View::Edit;
        self.status = format!("Editing {} - press 'Done - back to batch' when finished", self.batch.items[i].name);
    }

    /// take the editor's result back into the customer and restore the previous workspace
    pub(super) fn batch_commit_edit(&mut self) {
        let Some(i) = self.batch.editing.take() else { return };
        if i < self.batch.items.len() {
            let c = &mut self.batch.items[i];
            // take everything back: the editor may have changed the picture itself (AI upscale / face restore)
            if let Some(s) = &self.src {
                c.src = Some(s.clone());
                c.tex = self.src_tex.clone();
            }
            c.edit = self.edit.clone();
            c.alpha = self.alpha.clone();
            c.photo = None;
            c.state = CState::Ready;
        }
        self.ai.undo = None;
        if let Some(s) = self.batch.saved.take() {
            self.src = s.src;
            self.src_tex = s.src_tex;
            self.src_path = s.src_path;
            self.alpha = s.alpha;
            self.alpha_ver += 1;
            self.edit = s.edit;
            self.img_id += 1;
            self.photo = None;
            self.unit = None;
            self.last_key = None;
            self.last_aspect = self.edit.aspect();
            self.hist_reset();
        }
        self.view = View::Batch;
        self.status = "Changes saved to the customer - sheet updated".into();
    }

    fn cust_remove(&mut self, i: usize) {
        if self.batch.editing == Some(i) {
            self.batch_commit_edit();
        }
        if i < self.batch.items.len() {
            self.batch.items.remove(i);
        }
        self.batch.sel = if self.batch.items.is_empty() { None } else { Some(self.batch.sel.unwrap_or(0).min(self.batch.items.len() - 1)) };
        self.batch.key = None;
    }

    fn cust_replace(&mut self, ctx: &egui::Context, i: usize) {
        let mut d = rfd::FileDialog::new().add_filter("Photos", &["jpg", "jpeg", "png", "bmp", "webp"]);
        if !self.settings.last_dir.is_empty() {
            d = d.set_directory(&self.settings.last_dir);
        }
        let Some(p) = d.pick_file() else { return };
        let c = &mut self.batch.items[i];
        let id = c.id;
        c.state = CState::Loading;
        c.src = None;
        c.alpha = None;
        c.photo = None;
        c.path = Some(p.clone());
        c.note.clear();
        let (tx, ctx2) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = std::panic::catch_unwind(|| load_image(&p)).unwrap_or_else(|_| Err("The picture could not be read".into()));
            let _ = tx.send(Msg::BatchLoaded(id, r));
            ctx2.request_repaint();
        });
    }

    // ------------------------------------------------------------ plan + preview
    fn batch_key(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for c in &self.batch.items {
            (c.id, c.copies, c.photo.as_ref().map(|p| (p.width(), p.height())), c.photo.is_some()).hash(&mut h);
            // photo content changes always go through `photo = None` -> re-render -> bump via pointer-free counter
            c.photo.as_ref().map(|p| p.as_raw().iter().step_by(997).fold(0u64, |a, v| a.wrapping_mul(31).wrapping_add(*v as u64))).hash(&mut h);
        }
        serde_json::to_string(&self.settings.sheet).unwrap_or_default().hash(&mut h);
        (self.batch.landscape, self.batch.optimise, self.settings.dpi).hash(&mut h);
        h.finish()
    }

    fn batch_ready_items(&self) -> (Vec<usize>, Vec<batch::Item>) {
        let idx: Vec<usize> = (0..self.batch.items.len()).filter(|&i| self.batch.items[i].photo.is_some() && self.batch.items[i].copies > 0).collect();
        let items = idx.iter().map(|&i| {
            let c = &self.batch.items[i];
            let p = c.photo.as_ref().unwrap();
            batch::Item { w: p.width(), h: p.height(), copies: c.copies }
        }).collect();
        (idx, items)
    }

    /// all finished sheets at full resolution (for saving / printing)
    fn batch_pages_full(&self) -> Option<Vec<RgbImage>> {
        let (idx, items) = self.batch_ready_items();
        if items.is_empty() {
            return None;
        }
        let plan = batch::plan(&items, &self.settings.sheet, self.batch.landscape, self.settings.dpi, self.batch.optimise);
        let imgs: Vec<&RgbImage> = idx.iter().map(|&i| self.batch.items[i].photo.as_ref().unwrap()).collect();
        Some(batch::render(&plan, &imgs, &self.settings.sheet, self.settings.dpi))
    }

    fn batch_refresh(&mut self, ctx: &egui::Context) {
        // render a couple of photos per frame so the window never freezes
        let todo: Vec<usize> = (0..self.batch.items.len()).filter(|&i| self.batch.items[i].photo.is_none() && matches!(self.batch.items[i].state, CState::Ready) && self.batch.items[i].src.is_some()).take(2).collect();
        for i in todo {
            self.cust_render(i);
            ctx.request_repaint();
        }
        let key = self.batch_key();
        if self.batch.key == Some(key) {
            return;
        }
        self.batch.key = Some(key);
        let (idx, items) = self.batch_ready_items();
        let waiting = self.batch.items.iter().filter(|c| matches!(c.state, CState::Loading | CState::Queued | CState::Working)).count();
        if items.is_empty() {
            self.batch.tex.clear();
            self.batch.plan = batch::Plan::default();
            self.batch.info = if self.batch.items.is_empty() { String::new() } else { "Preparing photos...".into() };
            return;
        }
        let plan = batch::plan(&items, &self.settings.sheet, self.batch.landscape, self.settings.dpi, self.batch.optimise);
        let imgs: Vec<&RgbImage> = idx.iter().map(|&i| self.batch.items[i].photo.as_ref().unwrap()).collect();
        let pages = batch::render(&plan, &imgs, &self.settings.sheet, self.settings.dpi);
        self.batch.tex = pages
            .iter()
            .enumerate()
            .map(|(n, pg)| {
                let k = (1300.0 / pg.height().max(pg.width()) as f32).min(1.0);
                let small = image::imageops::resize(pg, ((pg.width() as f32 * k) as u32).max(1), ((pg.height() as f32 * k) as u32).max(1), image::imageops::FilterType::Triangle);
                let ci = egui::ColorImage::from_rgb([small.width() as usize, small.height() as usize], small.as_raw());
                ctx.load_texture(format!("batchpage{n}"), ci, TextureOptions::LINEAR)
            })
            .collect();
        self.batch.page = self.batch.page.min(self.batch.tex.len().saturating_sub(1));
        let mut info = format!("{} photos on {} sheet(s)  -  paper used {:.0}%", plan.total.saturating_sub(plan.too_big.iter().map(|&k| items[k].copies).sum::<u32>()), plan.pages, plan.used * 100.0);
        if !plan.too_big.is_empty() {
            info.push_str(&format!("   |   {} customer(s) do not fit on this paper!", plan.too_big.len()));
        }
        if waiting > 0 {
            info.push_str(&format!("   |   {waiting} more being prepared..."));
        }
        self.batch.info = info;
        self.batch.plan = plan;
    }

    pub(super) fn batch_tick(&mut self, ctx: &egui::Context) {
        if self.view == View::Batch {
            self.batch_refresh(ctx);
            if self.status.starts_with("Loading customer") && !self.batch.items.iter().any(|c| matches!(c.state, CState::Loading | CState::Queued | CState::Working)) {
                let bad = self.batch.items.iter().filter(|c| matches!(c.state, CState::Failed(_))).count();
                self.status = if bad > 0 { format!("{bad} photo(s) could not be loaded - see the red message on the customer") } else { "All customer photos are ready".into() };
            }
        }
    }

    // ------------------------------------------------------------ export
    fn batch_print(&mut self, ctx: &egui::Context) {
        let Some(pages) = self.batch_pages_full() else { return };
        let (dpi, mut paper) = (self.settings.dpi, self.settings.sheet.paper_mm());
        if self.batch.landscape {
            paper = (paper.0.max(paper.1), paper.0.min(paper.1));
        }
        self.print_now(ctx, pages, dpi, paper);
    }

    fn batch_save(&mut self, kind: &str) {
        let filter: (&str, &[&str]) = match kind {
            "pdf" => ("PDF", &["pdf"]),
            "png" => ("PNG", &["png"]),
            _ => ("JPEG", &["jpg", "jpeg"]),
        };
        let Some(path) = self.save_dialog(&format!("customers_sheet.{kind}"), filter) else { return };
        let Some(pages) = self.batch_pages_full() else { return };
        let (dpi, q) = (self.settings.dpi, self.settings.jpeg_q);
        let res: std::io::Result<()> = (|| {
            if kind == "pdf" {
                return std::fs::write(&path, sheet::encode_pdf(&pages, dpi, q));
            }
            for (i, pg) in pages.iter().enumerate() {
                let mut p = path.clone();
                if pages.len() > 1 {
                    let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                    p.set_file_name(format!("{}_{}.{}", stem, i + 1, kind));
                }
                std::fs::write(p, if kind == "png" { sheet::encode_png(pg, dpi) } else { sheet::encode_jpeg(pg, q, dpi) })?;
            }
            Ok(())
        })();
        self.status = match res {
            Ok(()) => format!("Saved {} sheet(s) to {}", pages.len(), path.parent().map(|p| p.display().to_string()).unwrap_or_default()),
            Err(e) => format!("Could not save: {e}"),
        };
    }

    // ------------------------------------------------------------ panels
    pub(super) fn batch_left(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        card(ui, "1", "Customers  -  photos to print", true, |ui| {
            let total: u32 = self.batch.items.iter().map(|c| c.copies).sum();
            ui.horizontal(|ui| {
                if button(ui, "Add photos...", Kind::Accent, vec2(140.0, 32.0)).on_hover_text("Choose many photos at once - or drag & drop them into this window").clicked() {
                    self.batch_pick(ctx);
                }
                if !self.batch.items.is_empty() && button(ui, "Clear all", Kind::Ghost, vec2(90.0, 32.0)).clicked() {
                    if self.batch.editing.is_some() {
                        self.batch_commit_edit();
                    }
                    self.batch.items.clear();
                    self.batch.sel = None;
                    self.batch.key = None;
                }
            });
            ui.horizontal(|ui| {
                sub(ui, "Copies for new photos");
                ui.add(egui::DragValue::new(&mut self.batch.default_copies).range(1..=50));
                if !self.batch.items.is_empty() && ui.button("Set for all").on_hover_text("Give every customer this number of copies").clicked() {
                    let n = self.batch.default_copies;
                    for c in self.batch.items.iter_mut() {
                        c.copies = n;
                    }
                }
            });
            if self.batch.items.is_empty() {
                hint(ui, "One photo per customer. Drag & drop many photos here, set the copies for each customer, and all of them are placed on as few A4 sheets as possible.");
            } else {
                egui::Frame::new().fill(Color32::from_rgb(0x2a, 0x1d, 0x08)).corner_radius(cr(8)).inner_margin(egui::Margin::symmetric(9, 5)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(format!("{} customers   -   {} photos in total", self.batch.items.len(), total)).font(brand::medium(12.5)).color(ORANGE));
                });
            }
        });

        let mut remove: Option<usize> = None;
        let mut swap: Option<(usize, usize)> = None;
        let mut edit: Option<usize> = None;
        let mut replace: Option<usize> = None;
        let n = self.batch.items.len();
        for i in 0..n {
            let selected = self.batch.sel == Some(i);
            let fill = if selected { Color32::from_rgb(0x14, 0x24, 0x3f) } else { Color32::from_rgb(0x0f, 0x15, 0x22) };
            let resp = egui::Frame::new().fill(fill).corner_radius(cr(10)).stroke(Stroke::new(1.0_f32, if selected { BLUE } else { BORDER })).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let c = &mut self.batch.items[i];
                ui.horizontal(|ui| {
                    match &c.tex {
                        Some(t) => {
                            let s = t.size();
                            let k = 54.0 / s[0].max(s[1]) as f32;
                            ui.image((t.id(), vec2(s[0] as f32 * k, s[1] as f32 * k)));
                        }
                        None => {
                            ui.add_sized([54.0, 54.0], egui::Spinner::new());
                        }
                    }
                    ui.vertical(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut c.name).desired_width(ui.available_width() - 4.0).font(egui::FontId::proportional(13.0)));
                        let (txt, col) = match &c.state {
                            CState::Loading => ("Loading...".to_string(), MUTED),
                            CState::Queued => ("Waiting for background AI...".to_string(), MUTED),
                            CState::Working => ("Removing background...".to_string(), ORANGE),
                            CState::Ready => (if c.note.is_empty() { "Ready".to_string() } else { c.note.clone() }, if c.note.is_empty() { GREEN } else { ORANGE }),
                            CState::Failed(e) => (e.clone(), RED),
                        };
                        ui.label(RichText::new(txt).size(11.0).color(col));
                    });
                });
                ui.horizontal(|ui| {
                    sub(ui, "Copies");
                    if ui.add_sized([26.0, 22.0], egui::Button::new("-")).clicked() {
                        c.copies = c.copies.saturating_sub(1).max(1);
                    }
                    ui.add(egui::DragValue::new(&mut c.copies).range(1..=99));
                    if ui.add_sized([26.0, 22.0], egui::Button::new("+")).clicked() {
                        c.copies = (c.copies + 1).min(99);
                    }
                });
            });
            // select on any click inside the card (an overlay widget would swallow the +/- clicks)
            if ctx.input(|inp| inp.pointer.primary_clicked()) && ui.rect_contains_pointer(resp.response.rect) {
                self.batch.sel = Some(i);
            }
            if selected {
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(self.batch.items[i].src.is_some(), egui::Button::new("Edit photo")).on_hover_text("Open this photo in the full editor (crop, colours, retouch)").clicked() {
                        edit = Some(i);
                    }
                    if ui.small_button("Replace").clicked() {
                        replace = Some(i);
                    }
                    if ui.add_enabled(i > 0, egui::Button::new("Up")).clicked() {
                        swap = Some((i, i - 1));
                    }
                    if ui.add_enabled(i + 1 < n, egui::Button::new("Down")).clicked() {
                        swap = Some((i, i + 1));
                    }
                    if ui.button(RichText::new("Remove").color(RED)).clicked() {
                        remove = Some(i);
                    }
                });
                // own size for this customer
                let (mut w, mut h) = self.batch.items[i].edit.size_mm();
                let mut own = self.batch.items[i].own;
                let was_own = own;
                ui.checkbox(&mut own, "Own size for this customer");
                let mut changed = false;
                if own {
                    ui.horizontal(|ui| {
                        changed |= ui.add(egui::DragValue::new(&mut w).range(10.0..=300.0).speed(0.2).fixed_decimals(1).suffix(" mm")).changed();
                        ui.label("x");
                        changed |= ui.add(egui::DragValue::new(&mut h).range(10.0..=300.0).speed(0.2).fixed_decimals(1).suffix(" mm")).changed();
                    });
                }
                self.batch.items[i].own = own;
                if own && (changed || !was_own) {
                    self.cust_resize(i, CUSTOM_IDX, w, h);
                } else if !own && was_own {
                    let (si, cw, ch) = (self.edit.size_idx, self.edit.custom_w_mm, self.edit.custom_h_mm);
                    self.cust_resize(i, si, cw, ch);
                }
            }
            ui.add_space(4.0);
        }
        if let Some((a, b)) = swap {
            self.batch.items.swap(a, b);
            self.batch.sel = Some(b);
            if let Some(e) = self.batch.editing {
                if e == a { self.batch.editing = Some(b); } else if e == b { self.batch.editing = Some(a); }
            }
        }
        if let Some(i) = remove {
            self.cust_remove(i);
        }
        if let Some(i) = edit {
            self.cust_edit(ctx, i);
        }
        if let Some(i) = replace {
            self.cust_replace(ctx, i);
        }
        ui.add_space(20.0);
    }

    pub(super) fn batch_right(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        card(ui, "", "Photo size  (all customers)", true, |ui| {
            let mut apply = false;
            egui::ComboBox::from_id_salt("bsize").width(ui.available_width()).selected_text(PRESETS[self.edit.size_idx.min(PRESETS.len() - 1)].name).show_ui(ui, |ui| {
                for (i, p) in PRESETS.iter().enumerate() {
                    if ui.selectable_value(&mut self.edit.size_idx, i, p.name).clicked() {
                        apply = true;
                    }
                }
            });
            if self.edit.size_idx == CUSTOM_IDX {
                ui.horizontal(|ui| {
                    apply |= ui.add(egui::DragValue::new(&mut self.edit.custom_w_mm).range(10.0..=300.0).speed(0.2).suffix(" mm")).changed();
                    ui.label("x");
                    apply |= ui.add(egui::DragValue::new(&mut self.edit.custom_h_mm).range(10.0..=300.0).speed(0.2).suffix(" mm")).changed();
                });
            }
            ui.horizontal(|ui| {
                sub(ui, "Background");
                apply |= ui.color_edit_button_srgb(&mut self.edit.bg).changed();
            });
            hint(ui, "Size / background here apply to every customer, except those with an own size.");
            if apply {
                let (si, w, h, bg) = (self.edit.size_idx, self.edit.custom_w_mm, self.edit.custom_h_mm, self.edit.bg);
                for i in 0..self.batch.items.len() {
                    self.batch.items[i].edit.bg = bg;
                    if !self.batch.items[i].own {
                        self.cust_resize(i, si, w, h);
                    } else {
                        self.batch.items[i].photo = None;
                    }
                }
                self.settings.size_idx = si;
                self.settings.custom_w_mm = w;
                self.settings.custom_h_mm = h;
                self.settings.bg = bg;
            }
        });

        card(ui, "", "Paper", true, |ui| {
            let s = &mut self.settings.sheet;
            egui::ComboBox::from_id_salt("bpaper").width(ui.available_width()).selected_text(sheet::PAPERS[s.paper_idx].name).show_ui(ui, |ui| {
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
            ui.horizontal(|ui| {
                sub(ui, "Paper");
                if chip(ui, "Portrait", !self.batch.landscape).clicked() {
                    self.batch.landscape = false;
                }
                if chip(ui, "Landscape", self.batch.landscape).clicked() {
                    self.batch.landscape = true;
                }
            });
            ui.horizontal(|ui| {
                sub(ui, "Gap");
                ui.add(egui::DragValue::new(&mut s.gap_mm).range(0.0..=20.0).speed(0.2).suffix(" mm"));
                sub(ui, "Margin");
                ui.add(egui::DragValue::new(&mut s.margin_mm).range(0.0..=30.0).speed(0.2).suffix(" mm"));
            });
            ui.horizontal(|ui| {
                sub(ui, "Lines");
                egui::ComboBox::from_id_salt("bborder").width(100.0).selected_text(match s.border { Border::None => "None", Border::Thin => "Thin border", Border::CutMarks => "Cut marks" }).show_ui(ui, |ui| {
                    ui.selectable_value(&mut s.border, Border::None, "None");
                    ui.selectable_value(&mut s.border, Border::Thin, "Thin border");
                    ui.selectable_value(&mut s.border, Border::CutMarks, "Cut marks");
                });
                egui::ComboBox::from_id_salt("balign").width(86.0).selected_text(match s.align { Align::TopLeft => "Top-left", Align::Center => "Centre" }).show_ui(ui, |ui| {
                    ui.selectable_value(&mut s.align, Align::TopLeft, "Top-left");
                    ui.selectable_value(&mut s.align, Align::Center, "Centre");
                });
            });
            ui.checkbox(&mut s.auto_rotate, "Rotate photos to save paper");
            ui.checkbox(&mut self.batch.optimise, "Smart packing (bigger photos first, saves paper)");
            ui.checkbox(&mut s.studio_footer, "Studio name at sheet bottom");
            ui.horizontal(|ui| {
                sub(ui, "Quality");
                for d in [200u32, 300, 600] {
                    if chip(ui, &format!("{d} dpi"), self.settings.dpi == d).clicked() {
                        self.settings.dpi = d;
                        for c in self.batch.items.iter_mut() {
                            c.photo = None;
                        }
                    }
                }
            });
            if !self.batch.info.is_empty() {
                egui::Frame::new().fill(Color32::from_rgb(0x2a, 0x1d, 0x08)).corner_radius(cr(8)).inner_margin(egui::Margin::symmetric(9, 5)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(&self.batch.info).font(brand::medium(12.0)).color(ORANGE));
                });
            }
        });

        let ready = self.batch.items.iter().any(|c| c.photo.is_some());
        ui.add_enabled_ui(ready, |ui| {
            card(ui, "", "Save & print", true, |ui| {
                if full_button(ui, "Print all sheets", Kind::Accent, 40.0).on_hover_text("Opens the Windows print window (choose printer / copies), prints at real size").clicked() {
                    self.batch_print(ctx);
                }
                sub(ui, "Save all sheets as");
                let w = (ui.available_width() - 16.0) / 3.0;
                ui.horizontal(|ui| {
                    if button(ui, "PDF", Kind::Primary, vec2(w, 32.0)).clicked() { self.batch_save("pdf"); }
                    if button(ui, "JPG", Kind::Primary, vec2(w, 32.0)).clicked() { self.batch_save("jpg"); }
                    if button(ui, "PNG", Kind::Primary, vec2(w, 32.0)).clicked() { self.batch_save("png"); }
                });
                let mut q = self.settings.jpeg_q as f32;
                if slider_row(ui, "JPG quality", &mut q, 60.0, 100.0, 1.0, "").changed() {
                    self.settings.jpeg_q = q as u8;
                }
            });
        });
        let _ = ctx;
        ui.add_space(20.0);
    }

    pub(super) fn batch_canvas(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let area = ui.available_rect_before_wrap();
        let painter = ui.painter_at(area);
        self.panel_bg(&painter, area, "ALL CUSTOMERS  -  PRINT PREVIEW");
        if self.batch.items.is_empty() {
            painter.text(area.center() - vec2(0.0, 14.0), egui::Align2::CENTER_CENTER, "Drag & drop your customers' photos here", egui::FontId::proportional(20.0), TEXT);
            painter.text(area.center() + vec2(0.0, 16.0), egui::Align2::CENTER_CENTER, "or press  'Add photos...'  on the left. Set the copies for each customer.", egui::FontId::proportional(13.5), MUTED);
            return;
        }
        // page switcher
        let n = self.batch.tex.len();
        if n > 1 {
            let mut pg = self.batch.page;
            let bar = Rect::from_min_size(pos2(area.max.x - 280.0, area.min.y + 4.0), vec2(270.0, 24.0));
            ui.put(bar, |ui: &mut Ui| {
                ui.horizontal(|ui| {
                    if ui.add_enabled(pg > 0, egui::Button::new("<")).clicked() {
                        pg -= 1;
                    }
                    ui.label(RichText::new(format!("Sheet {} of {}", pg + 1, n)).size(12.5).color(TEXT));
                    if ui.add_enabled(pg + 1 < n, egui::Button::new(">")).clicked() {
                        pg += 1;
                    }
                })
                .response
            });
            self.batch.page = pg;
        }
        match self.batch.tex.get(self.batch.page) {
            Some(t) => {
                let r = fit_rect(Rect::from_min_max(area.min + vec2(0.0, 34.0), area.max - vec2(0.0, 26.0)), t.size()[0] as f32, t.size()[1] as f32, 10.0);
                painter.rect_filled(r.expand(3.0), 3.0, Color32::from_gray(60));
                painter.image(t.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                painter.text(pos2(area.center().x, area.max.y - 13.0), egui::Align2::CENTER_CENTER, &self.batch.info, egui::FontId::proportional(11.5), MUTED);
            }
            None => {
                painter.text(area.center(), egui::Align2::CENTER_CENTER, "Preparing photos...", egui::FontId::proportional(16.0), ORANGE);
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
            }
        }
        let _ = ctx;
    }
}
