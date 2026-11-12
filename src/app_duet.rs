//! Dual photo mode: two independent photos (each with its own crop / colours / brush / retouch),
//! pin to A or B, free mouse layout (move, resize handles, snapping) and Undo / Redo.
use super::*;

/// Everything that belongs to the photo that is NOT being edited right now.
pub(super) struct Stash {
    pub src: Option<Arc<RgbImage>>,
    pub src_tex: Option<TextureHandle>,
    pub src_path: Option<PathBuf>,
    pub alpha: Option<Arc<GrayImage>>,
    pub alpha_ver: u32,
    pub edit: Edit,
}

#[derive(Clone, PartialEq)]
pub(super) struct Snap {
    pub edit: Edit,
    pub other: Option<Edit>,
}

#[derive(Default)]
pub(super) struct Hist {
    pub undo: Vec<Snap>,
    pub redo: Vec<Snap>,
    pub cur: Option<Snap>,
}

pub(super) struct CellDrag {
    slot: usize,
    mode: usize, // 0..7 handles, 8 = move
    start: Pos2,
    rect: (f32, f32, f32, f32), // frame px at drag start
}

const HANDLES: [(i32, i32); 8] = [(-1, -1), (0, -1), (1, -1), (1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0)];

impl App {
    fn cell_rect_px(&self, slot: usize) -> (f32, f32, f32, f32) {
        let c = self.edit.duet_cells()[slot];
        (c.0 as f32, c.1 as f32, c.2 as f32, c.3 as f32)
    }

    fn set_cell_px(&mut self, slot: usize, r: (f32, f32, f32, f32)) {
        let (tw, th) = self.edit.duet_px();
        self.edit.duet_make_free();
        self.edit.duet_rect[slot] = [r.0 / tw as f32, r.1 / th as f32, r.2 / tw as f32, r.3 / th as f32];
    }

    // ------------------------------------------------------------ the two photos
    pub(super) fn render_other_pub(&self) -> Option<RgbImage> {
        self.render_other()
    }

    fn render_other(&self) -> Option<RgbImage> {
        let st = self.other.as_ref()?;
        let src = st.src.as_ref()?;
        Some(render_photo(src, st.alpha.as_deref(), &self.other_edit(st), None))
    }

    fn other_edit(&self, st: &Stash) -> Edit {
        let mut e = st.edit.clone();
        e.size_idx = self.edit.size_idx;
        e.custom_w_mm = self.edit.custom_w_mm;
        e.custom_h_mm = self.edit.custom_h_mm;
        e.dpi = self.edit.dpi;
        e.duet = self.edit.duet;
        e.duet_w_mm = self.edit.duet_w_mm;
        e.duet_h_mm = self.edit.duet_h_mm;
        e.duet_between_mm = self.edit.duet_between_mm;
        e.duet_stack = self.edit.duet_stack;
        e.duet_free = self.edit.duet_free;
        e.duet_rect = self.edit.duet_rect;
        e.duet_active = 1 - self.edit.duet_active.min(1);
        e.caption = self.edit.caption.clone();
        e.caption_date = self.edit.caption_date.clone();
        e
    }

    /// Keep the rendered picture of the other photo up to date (cheap check, renders only when needed).
    pub(super) fn ensure_other(&mut self) {
        if !self.edit.duet || self.cell_drag.is_some() {
            return;
        }
        let Some(st) = &self.other else {
            self.other_img = None;
            self.other_key = None;
            return;
        };
        let key = (self.other_edit(st), st.alpha_ver);
        if self.other_key.as_ref() != Some(&key) || self.other_img.is_none() {
            self.other_img = self.render_other();
            self.other_key = Some(key);
        }
    }

    /// (photo for cell A, photo for cell B) in the right order for `compose_duet`.
    pub(super) fn duet_images<'a>(&'a self, work: &'a RgbImage, other: Option<&'a RgbImage>) -> (&'a RgbImage, Option<&'a RgbImage>) {
        match (self.edit.duet_active, other) {
            (0, o) => (work, o),
            (_, Some(o)) => (o, Some(work)),
            (_, None) => (work, None),
        }
    }

    /// Pin the photo you are editing as A or B. The other slot keeps the old photo; you keep working on this one
    /// (open another picture to replace it).
    pub(super) fn pin_as(&mut self, ctx: &egui::Context, slot: usize) {
        if self.src.is_none() {
            return;
        }
        let copy = Stash { src: self.src.clone(), src_tex: self.src_tex.clone(), src_path: self.src_path.clone(), alpha: self.alpha.clone(), alpha_ver: self.alpha_ver, edit: self.edit.clone() };
        // the pinned copy lives in `slot`; the working photo moves to the other slot
        self.edit.duet = true;
        self.edit.duet_active = (1 - slot.min(1)) as u8;
        self.other = Some(copy);
        self.other_key = None;
        self.other_img = None;
        self.last_aspect = self.edit.aspect();
        self.last_key = None;
        self.hist.cur = None;
        self.ensure_other();
        self.rebuild_unit(ctx);
        let (a, b) = if slot == 0 { ("A", "B") } else { ("B", "A") };
        self.status = format!("Pinned as Photo {a}. You are now editing Photo {b} - open another picture for it.");
    }

    pub(super) fn swap_active(&mut self, ctx: &egui::Context) {
        if self.src.is_none() {
            return;
        }
        if self.other.is_none() {
            self.other = Some(Stash { src: self.src.clone(), src_tex: self.src_tex.clone(), src_path: self.src_path.clone(), alpha: self.alpha.clone(), alpha_ver: self.alpha_ver, edit: self.edit.clone() });
            self.other_img = self.photo.clone();
            self.other_key = None;
        }
        let Some(mut st) = self.other.take() else { return };
        std::mem::swap(&mut self.src, &mut st.src);
        std::mem::swap(&mut self.src_tex, &mut st.src_tex);
        std::mem::swap(&mut self.src_path, &mut st.src_path);
        std::mem::swap(&mut self.alpha, &mut st.alpha);
        std::mem::swap(&mut self.alpha_ver, &mut st.alpha_ver);
        self.edit.swap_photo(&mut st.edit);
        self.other = Some(st);
        std::mem::swap(&mut self.photo, &mut self.other_img);
        self.edit.duet_active = 1 - self.edit.duet_active.min(1);
        self.other_key = None;
        self.last_key = None;
        self.last_aspect = self.edit.aspect();
        self.overlay.clear();
        self.tool = Tool::None;
        self.img_id += 1;
        self.hist.cur = None;
        self.hist.undo.clear();
        self.hist.redo.clear();
        self.ensure_other();
        self.rebuild_unit(ctx);
        self.status = format!("Now editing Photo {}", if self.edit.duet_active == 0 { "A" } else { "B" });
    }

    pub(super) fn select_slot(&mut self, ctx: &egui::Context, slot: usize) {
        if slot as u8 != self.edit.duet_active {
            self.swap_active(ctx);
        }
    }

    pub(super) fn clear_other(&mut self, ctx: &egui::Context) {
        self.other = None;
        self.other_img = None;
        self.other_key = None;
        self.edit.duet_active = 0;
        self.last_aspect = self.edit.aspect();
        self.last_key = None;
        self.rebuild_unit(ctx);
    }

    // ------------------------------------------------------------ layout helpers
    fn duet_swap_places(&mut self) {
        self.edit.duet_make_free();
        self.edit.duet_rect.swap(0, 1);
        self.last_key = None;
    }

    fn duet_same_size(&mut self) {
        self.edit.duet_make_free();
        let a = self.edit.duet_active as usize;
        let o = 1 - a;
        let r = self.edit.duet_rect[a];
        self.edit.duet_rect[o][2] = r[2];
        self.edit.duet_rect[o][3] = r[3];
    }

    /// B right next to A (or below it) with the chosen gap; both centred on the other axis.
    pub(super) fn duet_tidy(&mut self) {
        self.edit.duet_make_free();
        let (tw, th) = self.edit.duet_px();
        let gap = self.edit.duet_between_px() as f32;
        let (a, b) = (self.cell_rect_px(0), self.cell_rect_px(1));
        if self.edit.duet_stack {
            let total = a.3 + gap + b.3;
            let y0 = ((th as f32 - total) / 2.0).max(0.0);
            self.set_cell_px(0, ((tw as f32 - a.2) / 2.0, y0, a.2, a.3));
            self.set_cell_px(1, ((tw as f32 - b.2) / 2.0, y0 + a.3 + gap, b.2, b.3));
        } else {
            let total = a.2 + gap + b.2;
            let x0 = ((tw as f32 - total) / 2.0).max(0.0);
            self.set_cell_px(0, (x0, (th as f32 - a.3) / 2.0, a.2, a.3));
            self.set_cell_px(1, (x0 + a.2 + gap, (th as f32 - b.3) / 2.0, b.2, b.3));
        }
    }

    /// kind: 0 top, 1 middle, 2 bottom (each photo)   |   3 left, 4 centre, 5 right (the pair as a group)
    fn duet_align(&mut self, kind: u8) {
        self.edit.duet_make_free();
        let (tw, th) = (self.edit.duet_px().0 as f32, self.edit.duet_px().1 as f32);
        let (a, b) = (self.cell_rect_px(0), self.cell_rect_px(1));
        match kind {
            0..=2 => {
                for (i, c) in [a, b].into_iter().enumerate() {
                    let y = match kind {
                        0 => 0.0,
                        1 => (th - c.3) / 2.0,
                        _ => th - c.3,
                    };
                    self.set_cell_px(i, (c.0, y.max(0.0), c.2, c.3));
                }
            }
            _ => {
                let (l, r) = (a.0.min(b.0), (a.0 + a.2).max(b.0 + b.2));
                let dx = match kind {
                    3 => -l,
                    4 => (tw - (l + r)) / 2.0,
                    _ => tw - r,
                };
                self.set_cell_px(0, (a.0 + dx, a.1, a.2, a.3));
                self.set_cell_px(1, (b.0 + dx, b.1, b.2, b.3));
            }
        }
    }

    // ------------------------------------------------------------ undo / redo
    pub(super) fn hist_snap(&self) -> Snap {
        Snap { edit: self.edit.clone(), other: self.other.as_ref().map(|s| s.edit.clone()) }
    }

    pub(super) fn hist_reset(&mut self) {
        self.hist.cur = None;
        self.hist.undo.clear();
        self.hist.redo.clear();
    }

    /// Called every frame: one history step per finished gesture (slider drag, brush stroke, mouse drag ...).
    pub(super) fn hist_tick(&mut self, ctx: &egui::Context) {
        let snap = self.hist_snap();
        match &self.hist.cur {
            None => self.hist.cur = Some(snap),
            Some(c) if *c != snap => {
                if !ctx.input(|i| i.pointer.any_down()) {
                    let old = self.hist.cur.take().unwrap();
                    self.hist.undo.push(old);
                    if self.hist.undo.len() > 200 {
                        self.hist.undo.remove(0);
                    }
                    self.hist.redo.clear();
                    self.hist.cur = Some(snap);
                }
            }
            _ => {}
        }
    }

    fn apply_snap(&mut self, s: Snap) {
        self.edit = s.edit;
        if let (Some(st), Some(o)) = (self.other.as_mut(), s.other) {
            st.edit = o;
        }
        self.overlay.clear();
        self.last_aspect = self.edit.aspect();
        self.last_key = None;
        self.other_key = None;
    }

    pub(super) fn undo(&mut self) {
        self.hist_tick_force();
        if let Some(prev) = self.hist.undo.pop() {
            if let Some(c) = self.hist.cur.take() {
                self.hist.redo.push(c);
            }
            self.hist.cur = Some(prev.clone());
            self.apply_snap(prev);
        }
    }

    pub(super) fn redo(&mut self) {
        if let Some(next) = self.hist.redo.pop() {
            if let Some(c) = self.hist.cur.take() {
                self.hist.undo.push(c);
            }
            self.hist.cur = Some(next.clone());
            self.apply_snap(next);
        }
    }

    /// commit a pending change right now (so Ctrl+Z undoes it)
    fn hist_tick_force(&mut self) {
        let snap = self.hist_snap();
        if let Some(c) = self.hist.cur.take() {
            if c != snap {
                self.hist.undo.push(c);
                self.hist.redo.clear();
            }
        }
        self.hist.cur = Some(snap);
    }

    // ------------------------------------------------------------ left panel
    pub(super) fn duet_panel(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let has = self.src.is_some();
        let act = self.edit.duet_active as usize;
        egui::Frame::new().fill(Color32::from_rgb(0x0f, 0x15, 0x22)).corner_radius(cr(10)).inner_margin(egui::Margin::same(9)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            sub(ui, "Each photo has its own crop, colours and clean-up");
            ui.horizontal(|ui| {
                sub(ui, "Editing");
                for (i, name) in ["Photo A", "Photo B"].iter().enumerate() {
                    if chip(ui, name, act == i).on_hover_text("Click, or click the photo in the picture").clicked() {
                        self.select_slot(ctx, i);
                    }
                }
            });
            ui.horizontal(|ui| {
                if let Some(t) = self.other.as_ref().and_then(|s| s.src_tex.clone()) {
                    ui.image((t.id(), vec2(34.0, 34.0 * t.size()[1] as f32 / t.size()[0] as f32)));
                    sub(ui, &format!("Photo {} is a different picture", if act == 0 { "B" } else { "A" }));
                } else {
                    sub(ui, "Photo B = same picture as A");
                }
            });
            ui.add_enabled_ui(has, |ui| {
                ui.horizontal(|ui| {
                    let w = (ui.available_width() - 8.0) / 2.0;
                    if button(ui, "Pin as Photo A", Kind::Ghost, vec2(w, 28.0)).on_hover_text("Keep this photo as A, then open the next picture for B").clicked() {
                        self.pin_as(ctx, 0);
                    }
                    if button(ui, "Pin as Photo B", Kind::Ghost, vec2(w, 28.0)).on_hover_text("Keep this photo as B, then open the next picture for A").clicked() {
                        self.pin_as(ctx, 1);
                    }
                });
            });
            if self.other.is_some() && ui.small_button("Use one photo in both").clicked() {
                self.clear_other(ctx);
            }
            ui.add_space(4.0);
            sub(ui, "Layout  -  drag the photos in the picture, or type");
            ui.horizontal(|ui| {
                if chip(ui, "Auto", !self.edit.duet_free).clicked() {
                    self.edit.duet_free = false;
                }
                if chip(ui, "Free (mouse)", self.edit.duet_free).clicked() {
                    self.edit.duet_make_free();
                }
                if ui.small_button("Swap places").clicked() {
                    self.duet_swap_places();
                }
            });
            if self.edit.duet_free {
                let (fw, fh) = (self.edit.duet_w_mm, self.edit.duet_h_mm);
                let mut r = self.edit.duet_rect[act];
                let mut ch = false;
                ui.horizontal(|ui| {
                    sub(ui, "Position");
                    let (mut x, mut y) = (r[0] * fw, r[1] * fh);
                    let a = ui.add(egui::DragValue::new(&mut x).range(0.0..=fw).speed(0.1).fixed_decimals(1).suffix(" mm")).changed();
                    let b = ui.add(egui::DragValue::new(&mut y).range(0.0..=fh).speed(0.1).fixed_decimals(1).suffix(" mm")).changed();
                    if a || b {
                        r[0] = x / fw;
                        r[1] = y / fh;
                        ch = true;
                    }
                });
                ui.horizontal(|ui| {
                    sub(ui, "Size  W x H");
                    let (mut w, mut h) = (r[2] * fw, r[3] * fh);
                    let a = ui.add(egui::DragValue::new(&mut w).range(3.0..=fw).speed(0.1).fixed_decimals(1).suffix(" mm")).changed();
                    let b = ui.add(egui::DragValue::new(&mut h).range(3.0..=fh).speed(0.1).fixed_decimals(1).suffix(" mm")).changed();
                    if a || b {
                        r[2] = w / fw;
                        r[3] = h / fh;
                        ch = true;
                    }
                });
                if ch {
                    self.edit.duet_rect[act] = r;
                }
                ui.horizontal_wrapped(|ui| {
                    if ui.small_button("Same size as other").clicked() {
                        self.duet_same_size();
                    }
                    if ui.small_button("Place side by side").on_hover_text("Places the photos next to each other using the 'Gap between photos' value").clicked() {
                        self.duet_tidy();
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    sub(ui, "Align");
                    for (k, t) in [(0u8, "Top"), (1, "Middle"), (2, "Bottom"), (3, "Left"), (4, "Centre"), (5, "Right")] {
                        if ui.small_button(t).clicked() {
                            self.duet_align(k);
                        }
                    }
                });
                hint(ui, "Shift + corner = keep shape. Edges snap.");
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(!self.hist.undo.is_empty(), egui::Button::new("Undo  Ctrl+Z")).clicked() {
                    self.undo();
                }
                if ui.add_enabled(!self.hist.redo.is_empty(), egui::Button::new("Redo  Ctrl+Y")).clicked() {
                    self.redo();
                }
            });
        });
    }

    // ------------------------------------------------------------ mouse layout on the result picture
    /// Draw the photo frames + handles and handle the mouse. Returns true while a photo is being moved/resized.
    pub(super) fn duet_canvas(&mut self, ui: &mut Ui, painter: &egui::Painter, resp: &egui::Response, draw: Rect, scale: f32, inner: Rect) -> bool {
        let ctx = ui.ctx().clone();
        let (tw, th) = self.edit.duet_px();
        let (twf, thf) = (tw as f32, th as f32);
        let to_scr = |x: f32, y: f32| draw.min + vec2(x * scale, y * scale);
        let act = self.edit.duet_active as usize;
        let srect = |c: (f32, f32, f32, f32)| Rect::from_min_size(to_scr(c.0, c.1), vec2(c.2 * scale, c.3 * scale));
        let hpos = |r: Rect, h: usize| {
            let (dx, dy) = HANDLES[h];
            pos2(r.center().x + dx as f32 * r.width() / 2.0, r.center().y + dy as f32 * r.height() / 2.0)
        };
        let hover = resp.hover_pos();

        // ---- hit test at the moment a drag starts / on hover
        let hit = |p: Pos2, this: &Self| -> Option<(usize, usize)> {
            let r = srect(this.cell_rect_px(act));
            for h in 0..8 {
                if hpos(r, h).distance(p) <= 9.0 {
                    return Some((act, h));
                }
            }
            if r.contains(p) {
                return Some((act, 8));
            }
            let o = srect(this.cell_rect_px(1 - act));
            if o.contains(p) {
                return Some((1 - act, 8));
            }
            None
        };

        if resp.drag_started_by(egui::PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos() {
                if let Some((slot, mode)) = hit(p, self) {
                    self.select_slot(&ctx, slot);
                    self.edit.duet_make_free();
                    let act2 = self.edit.duet_active as usize;
                    let cell = self.cell_rect_px(act2);
                    self.cell_drag = Some(CellDrag { slot: act2, mode, start: p, rect: cell });
                }
            }
        }
        let mut dragging = false;
        if let Some(cd) = &self.cell_drag {
            let (slot, mode, start, r0) = (cd.slot, cd.mode, cd.start, cd.rect);
            if ui.input(|i| i.pointer.primary_down()) {
                dragging = true;
                if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
                    let (dx, dy) = ((p.x - start.x) / scale, (p.y - start.y) / scale);
                    let shift = ui.input(|i| i.modifiers.shift);
                    let other = self.cell_rect_px(1 - slot);
                    let thr = 6.0 / scale;
                    // candidate snap lines
                    let xs = [0.0, twf / 2.0, twf, other.0, other.0 + other.2, other.0 + other.2 / 2.0];
                    let ys = [0.0, thf / 2.0, thf, other.1, other.1 + other.3, other.1 + other.3 / 2.0];
                    let snap = |v: f32, c: &[f32]| -> Option<f32> { c.iter().copied().map(|q| (q - v, q)).filter(|(d, _)| d.abs() <= thr).min_by(|a, b| a.0.abs().partial_cmp(&b.0.abs()).unwrap()).map(|(_, q)| q) };
                    let (mut x0, mut y0, mut x1, mut y1) = (r0.0, r0.1, r0.0 + r0.2, r0.1 + r0.3);
                    if mode == 8 {
                        let (mut nx, mut ny) = (r0.0 + dx, r0.1 + dy);
                        let w = r0.2;
                        let h = r0.3;
                        // snap left / right / centre
                        let mut best: Option<f32> = None;
                        for off in [0.0, w / 2.0, w] {
                            if let Some(q) = snap(nx + off, &xs) {
                                let c = q - off - nx;
                                if best.map(|b| c.abs() < b.abs()).unwrap_or(true) {
                                    best = Some(c);
                                }
                            }
                        }
                        if let Some(c) = best {
                            nx += c;
                        }
                        let mut best: Option<f32> = None;
                        for off in [0.0, h / 2.0, h] {
                            if let Some(q) = snap(ny + off, &ys) {
                                let c = q - off - ny;
                                if best.map(|b| c.abs() < b.abs()).unwrap_or(true) {
                                    best = Some(c);
                                }
                            }
                        }
                        if let Some(c) = best {
                            ny += c;
                        }
                        x0 = nx.clamp(0.0, (twf - w).max(0.0));
                        y0 = ny.clamp(0.0, (thf - h).max(0.0));
                        x1 = x0 + w;
                        y1 = y0 + h;
                    } else {
                        let (hx, hy) = HANDLES[mode];
                        if hx < 0 {
                            x0 = snap(x0 + dx, &xs).unwrap_or(x0 + dx);
                        }
                        if hx > 0 {
                            x1 = snap(x1 + dx, &xs).unwrap_or(x1 + dx);
                        }
                        if hy < 0 {
                            y0 = snap(y0 + dy, &ys).unwrap_or(y0 + dy);
                        }
                        if hy > 0 {
                            y1 = snap(y1 + dy, &ys).unwrap_or(y1 + dy);
                        }
                        x0 = x0.clamp(0.0, twf);
                        x1 = x1.clamp(0.0, twf);
                        y0 = y0.clamp(0.0, thf);
                        y1 = y1.clamp(0.0, thf);
                        let min = 12.0;
                        if x1 - x0 < min {
                            if hx < 0 {
                                x0 = x1 - min;
                            } else {
                                x1 = x0 + min;
                            }
                        }
                        if y1 - y0 < min {
                            if hy < 0 {
                                y0 = y1 - min;
                            } else {
                                y1 = y0 + min;
                            }
                        }
                        if shift && hx != 0 && hy != 0 {
                            let ratio = r0.2 / r0.3;
                            let nh = (x1 - x0) / ratio;
                            if hy < 0 {
                                y0 = y1 - nh;
                            } else {
                                y1 = y0 + nh;
                            }
                            y0 = y0.max(0.0);
                            y1 = y1.min(thf);
                        }
                    }
                    self.set_cell_px(slot, (x0, y0, (x1 - x0).max(8.0), (y1 - y0).max(8.0)));
                }
            } else {
                self.cell_drag = None;
                self.last_key = None;
                self.other_key = None;
            }
        }
        if resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                if let Some((slot, _)) = hit(p, self) {
                    self.select_slot(&ctx, slot);
                }
            }
        }

        // ---- cursor
        if let Some(p) = hover {
            let m = self.cell_drag.as_ref().map(|c| c.mode).or_else(|| hit(p, self).map(|h| h.1));
            if let Some(m) = m {
                let icon = match m {
                    0 | 4 => egui::CursorIcon::ResizeNwSe,
                    2 | 6 => egui::CursorIcon::ResizeNeSw,
                    1 | 5 => egui::CursorIcon::ResizeVertical,
                    3 | 7 => egui::CursorIcon::ResizeHorizontal,
                    _ => egui::CursorIcon::Grab,
                };
                ui.ctx().set_cursor_icon(icon);
            }
        }

        // ---- frames + handles
        let pl = painter.with_clip_rect(inner.expand(2.0));
        let cells = [self.cell_rect_px(0), self.cell_rect_px(1)];
        for i in 0..2 {
            let r = srect(cells[i]);
            let on = i == act;
            let col = if on { ORANGE } else { BLUE };
            pl.rect_stroke(r, 0.0, Stroke::new(if on { 2.0_f32 } else { 1.0_f32 }, col), StrokeKind::Inside);
            let tag = if i == 0 { "A" } else { "B" };
            let lab = Rect::from_min_size(r.min + vec2(3.0, 3.0), vec2(18.0, 16.0));
            pl.rect_filled(lab, 4.0, col);
            pl.text(lab.center(), egui::Align2::CENTER_CENTER, tag, egui::FontId::proportional(11.5), Color32::from_rgb(0x10, 0x10, 0x18));
            if on {
                for h in 0..8 {
                    let c = hpos(r, h);
                    pl.rect_filled(Rect::from_center_size(c, vec2(9.0, 9.0)), 2.0, Color32::WHITE);
                    pl.rect_stroke(Rect::from_center_size(c, vec2(9.0, 9.0)), 2.0, Stroke::new(1.5_f32, ORANGE), StrokeKind::Inside);
                }
            }
        }
        if dragging {
            let c = cells[act];
            let mmw = c.2 / self.edit.dpi as f32 * 25.4;
            let mmh = c.3 / self.edit.dpi as f32 * 25.4;
            let r = srect(c);
            pl.text(r.center_bottom() + vec2(0.0, -4.0), egui::Align2::CENTER_BOTTOM, format!("{:.1} x {:.1} mm", mmw, mmh), egui::FontId::proportional(11.5), Color32::WHITE);
        }
        dragging
    }
}
