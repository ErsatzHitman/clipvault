use crate::{
    clipboard::{ClipboardAction, ClipboardWorker},
    config::{Config, ImageEmbedding, Paths, Provider},
    engine::{Command, Engine, Event, SearchRequest},
    model::{Kind, SearchHit, SearchMode},
    platform,
};
use anyhow::Result;
use crossbeam_channel::Sender;
use eframe::egui;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const CARD_W: f32 = 200.0;
const GAP: f32 = 10.0;
const THUMB_CAP: usize = 96;
fn search_id() -> egui::Id {
    egui::Id::new("search-edit")
}
const PILL: f32 = 34.0;
const RADIUS: u8 = 12;

/// One small palette shared by every surface in the strip.
mod theme {
    use eframe::egui::Color32;
    pub const BG: Color32 = Color32::from_rgb(20, 21, 25);
    pub const SURFACE: Color32 = Color32::from_rgb(30, 32, 38);
    pub const SURFACE_HI: Color32 = Color32::from_rgb(40, 43, 51);
    pub const LINE: Color32 = Color32::from_rgb(52, 56, 66);
    pub const TEXT: Color32 = Color32::from_rgb(228, 231, 238);
    pub const MUTED: Color32 = Color32::from_rgb(134, 140, 154);
    pub const ACCENT: Color32 = Color32::from_rgb(122, 146, 255);
    pub const WARN: Color32 = Color32::from_rgb(232, 178, 72);
    pub const DANGER: Color32 = Color32::from_rgb(232, 96, 96);
}

fn apply_theme(ctx: &egui::Context) {
    use egui::{FontFamily::Proportional, FontId, Stroke, TextStyle};
    let mut v = egui::Visuals::dark();
    v.panel_fill = theme::BG;
    v.window_fill = theme::SURFACE;
    v.extreme_bg_color = theme::BG;
    v.faint_bg_color = theme::SURFACE;
    v.window_stroke = Stroke::new(1.0_f32, theme::LINE);
    v.window_corner_radius = 14.into();
    v.menu_corner_radius = 12.into();
    let shadow = egui::Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: egui::Color32::from_black_alpha(110),
    };
    (v.window_shadow, v.popup_shadow) = (shadow, shadow);
    v.selection.bg_fill = theme::ACCENT.gamma_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0_f32, theme::ACCENT);
    v.hyperlink_color = theme::ACCENT;
    let w = &mut v.widgets;
    for s in [
        &mut w.noninteractive,
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
    ] {
        s.corner_radius = 8.into();
        s.expansion = 0.0;
    }
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, theme::LINE);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, theme::TEXT.gamma_multiply(0.9));
    (w.inactive.bg_fill, w.inactive.weak_bg_fill) = (theme::SURFACE_HI, theme::SURFACE_HI);
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0_f32, theme::TEXT);
    (w.hovered.bg_fill, w.hovered.weak_bg_fill) = (theme::LINE, theme::LINE);
    w.hovered.bg_stroke = Stroke::NONE;
    w.hovered.fg_stroke = Stroke::new(1.0_f32, theme::TEXT);
    (w.active.bg_fill, w.active.weak_bg_fill) = (
        theme::ACCENT.gamma_multiply(0.6),
        theme::ACCENT.gamma_multiply(0.6),
    );
    w.active.bg_stroke = Stroke::NONE;
    (w.open.bg_fill, w.open.weak_bg_fill) = (theme::SURFACE_HI, theme::SURFACE_HI);
    let mut style = egui::Style {
        visuals: v,
        always_scroll_the_only_direction: true,
        ..Default::default()
    };
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.spacing.interact_size.y = 28.0;
    style.spacing.menu_margin = egui::Margin::same(8);
    style.spacing.window_margin = egui::Margin::same(18);
    style.spacing.scroll = egui::style::ScrollStyle::floating();
    for (t, size) in [
        (TextStyle::Small, 11.0),
        (TextStyle::Body, 13.5),
        (TextStyle::Button, 13.5),
        (TextStyle::Heading, 17.0),
    ] {
        style.text_styles.insert(t, FontId::new(size, Proportional));
    }
    ctx.set_style(style);
}

/// Move toward `target` along an ease-out-back curve, so changes overshoot a little and settle.
fn spring(ctx: &egui::Context, id: egui::Id, target: f32, secs: f32) -> f32 {
    let now = ctx.input(|i| i.time);
    let Some((from, to, start)) = ctx.data(|d| d.get_temp::<(f32, f32, f64)>(id)) else {
        ctx.data_mut(|d| d.insert_temp(id, (target, target, now - secs as f64)));
        return target;
    };
    let t = (((now - start) / secs as f64) as f32).clamp(0.0, 1.0);
    let cur = from + (to - from) * egui::emath::easing::back_out(t);
    if to != target {
        ctx.data_mut(|d| d.insert_temp(id, (cur, target, now)));
    }
    if to != target || t < 1.0 {
        ctx.request_repaint();
    }
    cur
}

/// Round, borderless icon button; `paint` draws the glyph in the hover-aware colour.
fn icon_button(
    ui: &mut egui::Ui,
    paint: impl FnOnce(&egui::Painter, egui::Rect, egui::Color32),
) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(egui::vec2(PILL, PILL), egui::Sense::click());
    let h = ui.ctx().animate_bool_with_time(r.id, r.hovered(), 0.12);
    let p = ui.painter();
    p.circle_filled(
        rect.center(),
        PILL / 2.0,
        theme::SURFACE.lerp_to_gamma(theme::SURFACE_HI, h),
    );
    p.circle_stroke(
        rect.center(),
        PILL / 2.0 - 0.5,
        egui::Stroke::new(1.0_f32, theme::LINE),
    );
    paint(p, rect, theme::MUTED.lerp_to_gamma(theme::TEXT, h));
    r
}

fn cross(p: &egui::Painter, c: egui::Pos2, r: f32, color: egui::Color32) {
    let s = egui::Stroke::new(1.5_f32, color);
    p.line_segment([c + egui::vec2(-r, -r), c + egui::vec2(r, r)], s);
    p.line_segment([c + egui::vec2(-r, r), c + egui::vec2(r, -r)], s);
}

fn magnifier(p: &egui::Painter, c: egui::Pos2, color: egui::Color32) {
    let s = egui::Stroke::new(1.7_f32, color);
    p.circle_stroke(c + egui::vec2(-1.5, -1.5), 5.5, s);
    p.line_segment([c + egui::vec2(2.6, 2.6), c + egui::vec2(6.5, 6.5)], s);
}

/// Pill-shaped segmented control with a sliding highlight. Returns true when the value changed.
fn segmented(ui: &mut egui::Ui, id: egui::Id, labels: &[&str], value: &mut usize) -> bool {
    use egui::{Rect, Sense, vec2};
    let font = egui::FontId::proportional(13.0);
    let widths: Vec<f32> = labels
        .iter()
        .map(|l| {
            ui.painter()
                .layout_no_wrap((*l).to_owned(), font.clone(), theme::TEXT)
                .size()
                .x
                + 26.0
        })
        .collect();
    let (rect, _) =
        ui.allocate_exact_size(vec2(widths.iter().sum::<f32>() + 6.0, PILL), Sense::hover());
    let mut x = rect.min.x + 3.0;
    let segs: Vec<Rect> = widths
        .iter()
        .map(|w| {
            let r = Rect::from_min_size(egui::pos2(x, rect.min.y + 3.0), vec2(*w, PILL - 6.0));
            x += w;
            r
        })
        .collect();
    let mut changed = false;
    for (i, seg) in segs.iter().enumerate() {
        if ui.interact(*seg, id.with(i), Sense::click()).clicked() && *value != i {
            *value = i;
            changed = true;
        }
    }
    let target = segs[(*value).min(segs.len() - 1)];
    let ax = spring(ui.ctx(), id.with("x"), target.min.x - rect.min.x, 0.34);
    let aw = spring(ui.ctx(), id.with("w"), target.width(), 0.34);
    let p = ui.painter();
    p.rect_filled(rect, (PILL / 2.0) as u8, theme::SURFACE);
    p.rect_stroke(
        rect,
        (PILL / 2.0) as u8,
        egui::Stroke::new(1.0_f32, theme::LINE),
        egui::StrokeKind::Inside,
    );
    let knob = Rect::from_min_size(
        egui::pos2(rect.min.x + ax, target.min.y),
        vec2(aw.max(8.0), target.height()),
    );
    p.rect_filled(knob, (knob.height() / 2.0) as u8, theme::SURFACE_HI);
    for (i, (seg, label)) in segs.iter().zip(labels).enumerate() {
        let hov = ui.rect_contains_pointer(*seg);
        let c = if i == *value {
            theme::TEXT
        } else if hov {
            theme::TEXT.gamma_multiply(0.8)
        } else {
            theme::MUTED
        };
        p.text(
            seg.center(),
            egui::Align2::CENTER_CENTER,
            *label,
            font.clone(),
            c,
        );
    }
    changed
}

struct Thumb {
    tex: Option<egui::TextureHandle>,
    used: u64,
}
enum Act {
    Select(usize),
    Open(usize),
    Copy(usize),
    Paste(usize),
    Pin(i64, bool),
    Del(i64),
}

/// Indices of the cards that intersect the horizontal viewport (plus one card of slack).
fn visible_range(min_x: f32, max_x: f32, step: f32, n: usize) -> Range<usize> {
    let first = ((min_x / step).floor().max(0.0) as usize).saturating_sub(1);
    let last = ((max_x / step).ceil().max(0.0) as usize + 1).min(n);
    first.min(last)..last
}
fn human_size(b: u64) -> String {
    let (mut v, mut u) = (b as f64, 0);
    while v >= 1024.0 && u < 4 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", ["B", "KB", "MB", "GB", "TB"][u])
    }
}
fn age(secs: i64) -> String {
    match secs.max(0) {
        s if s < 60 => "now".into(),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86400),
    }
}
/// (first file name, lowercase extension, file count) from a file entry title.
fn file_parts(title: &str) -> (String, String, usize) {
    let first = title.split(", ").next().unwrap_or(title).to_owned();
    let ext = std::path::Path::new(&first)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    (first, ext, title.split(", ").count())
}
fn is_image_ext(ext: &str) -> bool {
    matches!(ext, "png" | "jpg" | "jpeg" | "webp")
}
fn badge_color(ext: &str) -> egui::Color32 {
    let h = ext
        .bytes()
        .fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    let hue = (h % 360) as f32 / 360.0;
    egui::Color32::from(egui::ecolor::Hsva::new(hue, 0.5, 0.95, 1.0))
}

pub struct App {
    engine: Engine,
    clipboard: Sender<ClipboardAction>,
    _hotkeys: Option<GlobalHotKeyManager>,
    hotkey_id: Option<u32>,
    query: String,
    mode: SearchMode,
    generation: u64,
    pending_search: bool,
    edited: Instant,
    hits: Vec<SearchHit>,
    selected: usize,
    filter: usize,
    thumbs: HashMap<i64, Thumb>,
    thumb_req: HashSet<i64>,
    frame: u64,
    scroll_to_selected: bool,
    docked: bool,
    dock_frames: u8,
    tall: bool,
    area: Option<platform::WorkArea>,
    shown_at: Instant,
    was_focused: bool,
    settings: bool,
    draft: Config,
    exclusions: String,
    status: String,
    counts: (u64, u64),
    semantic: bool,
    focus_search: bool,
    search_open: bool,
    mode_menu: bool,
    status_seen: String,
    status_at: Instant,
    visible: bool,
    confirm_clear: bool,
    paths: Paths,
    quitting: bool,
}
impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, paths: Paths, c: Config) -> Result<Self> {
        let ctx = cc.egui_ctx.clone();
        let repaint: Arc<dyn Fn() + Send + Sync> = Arc::new(move || ctx.request_repaint());
        let engine = Engine::start(paths.clone(), c.clone(), repaint.clone())?;
        let focus = Arc::new(Mutex::new(platform::foreground()));
        let clipboard = ClipboardWorker::start(
            paths.clone(),
            engine.config.clone(),
            engine.stop.clone(),
            engine.commands.clone(),
            focus.clone(),
            repaint,
        )?;
        let mut status = String::new();
        let mut manager = None;
        let mut hotkey_id = None;
        match c.hotkey.parse::<HotKey>() {
            Ok(key) => match GlobalHotKeyManager::new().and_then(|m| {
                m.register(key)?;
                Ok(m)
            }) {
                Ok(m) => {
                    manager = Some(m);
                    hotkey_id = Some(key.id());
                }
                Err(e) => status = format!("Shortcut unavailable: {e}. The window remains usable."),
            },
            Err(e) => status = format!("Invalid shortcut: {e}"),
        }
        let docked = hotkey_id.is_some();
        if !docked {
            // Without a working shortcut the strip would be unreachable: fall back to a normal window.
            for cmd in [
                egui::ViewportCommand::Decorations(true),
                egui::ViewportCommand::WindowLevel(egui::WindowLevel::Normal),
                egui::ViewportCommand::InnerSize(egui::vec2(920.0, 620.0)),
            ] {
                cc.egui_ctx.send_viewport_cmd(cmd);
            }
        }
        apply_theme(&cc.egui_ctx);
        Ok(Self {
            engine,
            clipboard,
            _hotkeys: manager,
            hotkey_id,
            query: String::new(),
            mode: SearchMode::Hybrid,
            generation: 0,
            pending_search: true,
            edited: Instant::now(),
            hits: Vec::new(),
            selected: 0,
            filter: 0,
            thumbs: HashMap::new(),
            thumb_req: HashSet::new(),
            frame: 0,
            scroll_to_selected: false,
            docked,
            dock_frames: if docked { 3 } else { 0 },
            tall: false,
            area: if docked { platform::work_area() } else { None },
            shown_at: Instant::now(),
            was_focused: false,
            settings: false,
            exclusions: c.excluded_apps.join("\n"),
            draft: c,
            status,
            counts: (0, 0),
            semantic: false,
            focus_search: false,
            search_open: false,
            mode_menu: false,
            status_seen: String::new(),
            status_at: Instant::now(),
            visible: true,
            confirm_clear: false,
            paths,
            quitting: false,
        })
    }
    fn refresh(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.pending_search = true;
        self.edited = Instant::now();
    }
    fn command(&mut self, c: Command) {
        if !self.engine.send(c) {
            self.status = "Worker queue is busy. Try again.".into();
        }
    }
    fn hide(&mut self, ctx: &egui::Context) {
        self.visible = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    }
    fn restore(&mut self, ctx: &egui::Context, paste: bool) {
        if let Some(hit) = self.hits.get(self.selected) {
            match self
                .clipboard
                .try_send(ClipboardAction::Restore(hit.entry.id, paste))
            {
                Ok(()) => {
                    self.status = if paste {
                        "Copied; returning to target app."
                    } else {
                        "Copied to clipboard."
                    }
                    .into();
                    if paste && self.hotkey_id.is_some() {
                        self.hide(ctx);
                    }
                }
                Err(_) => self.status = "Clipboard worker is busy. Try again.".into(),
            }
        }
    }
    fn show(&mut self, ctx: &egui::Context) {
        self.visible = true;
        if self.docked {
            self.area = platform::work_area();
            self.dock_frames = 3;
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                egui::WindowLevel::AlwaysOnTop,
            ));
        }
        self.shown_at = Instant::now();
        self.was_focused = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        self.focus_search = false;
        self.search_open = false;
        self.query.clear();
        self.selected = 0;
        self.scroll_to_selected = true;
        self.refresh();
    }
    /// Place the window along the bottom of the work area. Re-run for a few frames so a
    /// monitor with a different DPI scale settles on the right size.
    fn dock(&mut self, ctx: &egui::Context) {
        let ppp = ctx.pixels_per_point();
        let mon = ctx
            .input(|i| i.viewport().monitor_size)
            .unwrap_or(egui::vec2(1280.0, 800.0));
        let (ax, ay, aw, ah, pts) = match self.area {
            Some((x, y, w, h)) => (x, y, w, h, platform::WORK_AREA_IN_POINTS),
            None => (
                0.0,
                0.0,
                mon.x * ppp,
                (mon.y - 48.0).max(200.0) * ppp,
                false,
            ),
        };
        let (x, mut y, mut w, mut h) = platform::dock_rect((ax, ay, aw, ah), ppp, pts);
        let (bottom, area_h) = (y + h, if pts { ah } else { ah / ppp });
        if self.tall {
            h = (area_h * 0.8).max(h);
            y = bottom - h;
        }
        w = w.min(mon.x);
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(egui::vec2(
            320.0, 160.0,
        )));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(w, h)));
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(x, y)));
    }
    fn want_thumb(&mut self, id: i64) {
        if !self.thumb_req.contains(&id) && self.engine.thumbs.try_send(id).is_ok() {
            self.thumb_req.insert(id);
        }
    }
    fn thumb(&mut self, id: i64) -> Option<egui::TextureHandle> {
        let t = self.thumbs.get_mut(&id)?;
        t.used = self.frame;
        t.tex.clone()
    }
    fn receive(&mut self, ctx: &egui::Context) {
        for event in self.engine.events.try_iter().take(128).collect::<Vec<_>>() {
            match event {
                Event::Changed => self.refresh(),
                Event::Status(s) => self.status = s,
                Event::Counts(a, b) => self.counts = (a, b),
                Event::Results {
                    generation,
                    hits,
                    semantic,
                    error,
                } => {
                    if generation == self.generation {
                        let old = self.hits.get(self.selected).map(|h| h.entry.id);
                        self.hits = hits
                            .into_iter()
                            .filter(|h| match self.filter {
                                1 => h.entry.kind == Kind::Text,
                                2 => h.entry.kind == Kind::Image,
                                3 => h.entry.kind == Kind::Files,
                                _ => true,
                            })
                            .collect();
                        self.selected = old
                            .and_then(|id| self.hits.iter().position(|h| h.entry.id == id))
                            .unwrap_or(0);
                        self.semantic = semantic;
                        if let Some(e) = error {
                            self.status = e;
                        }
                    }
                }
                Event::Preview { .. } => {}
                Event::Thumb { id, image } => {
                    let tex = image.map(|(w, h, pixels)| {
                        ctx.load_texture(
                            format!("thumb-{id}"),
                            egui::ColorImage::from_rgba_unmultiplied([w, h], &pixels),
                            egui::TextureOptions::LINEAR,
                        )
                    });
                    self.thumbs.insert(
                        id,
                        Thumb {
                            tex,
                            used: self.frame,
                        },
                    );
                    while self.thumbs.len() > THUMB_CAP {
                        let Some((&old, _)) = self.thumbs.iter().min_by_key(|(_, t)| t.used) else {
                            break;
                        };
                        self.thumbs.remove(&old);
                        self.thumb_req.remove(&old);
                    }
                }
            }
        }
        while let Ok(s) = crate::clipboard::status_receiver().try_recv() {
            match s {
                crate::clipboard::ClipboardStatus::Message(s) => self.status = s,
                crate::clipboard::ClipboardStatus::RestoreFailed(s) => {
                    self.status = s;
                    self.show(ctx);
                }
            }
        }
    }
    fn card(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        i: usize,
        hit: &SearchHit,
        acts: &mut Vec<Act>,
    ) {
        use egui::{Align2, Color32, FontId, Sense, Stroke, StrokeKind, text::LayoutJob, vec2};
        let e = &hit.entry;
        let r = ui.interact(rect, ui.id().with(("card", e.id)), Sense::click());
        let (sel, hov) = (self.selected == i, r.hovered());
        let ctx = ui.ctx().clone();
        let ht = ctx.animate_bool_with_time(r.id.with("hover"), hov || sel, 0.14);
        let st = ctx.animate_bool_with_time_and_easing(
            r.id.with("sel"),
            sel,
            0.22,
            egui::emath::easing::back_out,
        );
        // Hovered and selected cards lift slightly.
        let rect = rect.translate(vec2(0.0, -3.0 * ht));
        let p = ui.painter().clone();
        if st > 0.01 {
            p.rect_filled(
                rect.expand(4.0 * st),
                RADIUS + 4,
                theme::ACCENT.gamma_multiply(0.16 * st.min(1.0)),
            );
        }
        p.rect_filled(
            rect,
            RADIUS,
            theme::SURFACE.lerp_to_gamma(theme::SURFACE_HI, ht),
        );
        p.rect_stroke(
            rect,
            RADIUS,
            Stroke::new(
                1.0 + st.clamp(0.0, 1.0),
                theme::LINE.lerp_to_gamma(theme::ACCENT, st.clamp(0.0, 1.0)),
            ),
            StrokeKind::Inside,
        );
        let body =
            egui::Rect::from_min_max(rect.min + vec2(12.0, 12.0), rect.max - vec2(12.0, 30.0));
        let bp = ui.painter_at(body);
        let draw_thumb = |ui: &egui::Ui, tex: &egui::TextureHandle, area: egui::Rect| {
            let s = tex.size_vec2();
            let k = (area.width() / s.x).min(area.height() / s.y);
            egui::Image::new(tex)
                .corner_radius(8)
                .paint_at(ui, egui::Rect::from_center_size(area.center(), s * k));
        };
        let wrapped = |text: String, size: f32, color: Color32, w: f32, rows: usize| {
            let mut job = LayoutJob::simple(text, FontId::proportional(size), color, w);
            job.wrap.max_rows = rows;
            job.wrap.break_anywhere = rows == 1;
            ui.painter().layout_job(job)
        };
        let meta = match e.kind {
            Kind::Text => {
                let t: String = e.text.chars().take(400).collect();
                let t = if t.trim().is_empty() {
                    e.title.clone()
                } else {
                    t.trim().to_owned()
                };
                let rows = ((body.height() / 18.0) as usize).max(1);
                bp.galley(
                    body.min,
                    wrapped(t, 13.5, theme::TEXT, body.width(), rows),
                    theme::TEXT,
                );
                format!("{} chars", e.text.chars().count())
            }
            Kind::Image => {
                self.want_thumb(e.id);
                match self.thumb(e.id) {
                    Some(t) => draw_thumb(ui, &t, body),
                    None => {
                        bp.rect_filled(body, 8, theme::SURFACE_HI);
                    }
                }
                e.title
                    .strip_prefix("Image · ")
                    .unwrap_or(&e.title)
                    .to_owned()
            }
            Kind::Files => {
                let (name, ext, count) = file_parts(&e.title);
                let has_img = is_image_ext(&ext);
                if has_img {
                    self.want_thumb(e.id);
                }
                let tex = if has_img { self.thumb(e.id) } else { None };
                if let Some(t) = tex {
                    draw_thumb(
                        ui,
                        &t,
                        egui::Rect::from_min_max(
                            body.min,
                            egui::pos2(body.max.x, body.max.y - 22.0),
                        ),
                    );
                    bp.galley(
                        egui::pos2(body.min.x, body.max.y - 17.0),
                        wrapped(name, 12.5, theme::TEXT, body.width(), 1),
                        theme::TEXT,
                    );
                } else {
                    let label: String = if ext.is_empty() {
                        "FILE".to_owned()
                    } else {
                        ext.to_uppercase().chars().take(4).collect()
                    };
                    let badge = egui::Rect::from_min_size(body.min, vec2(40.0, 48.0));
                    bp.rect_filled(badge, 8, badge_color(&ext).gamma_multiply(0.25));
                    bp.text(
                        badge.center(),
                        Align2::CENTER_CENTER,
                        label,
                        FontId::proportional(11.5),
                        badge_color(&ext),
                    );
                    bp.galley(
                        body.min + vec2(0.0, 58.0),
                        wrapped(name, 13.5, theme::TEXT, body.width(), 3),
                        theme::TEXT,
                    );
                }
                if count > 1 {
                    format!("{count} files · {}", human_size(e.bytes))
                } else {
                    human_size(e.bytes)
                }
            }
        };
        // Footer: quiet metadata, plus an error dot only when indexing actually failed.
        let failed = !matches!(e.index_status.as_str(), "ready" | "pending" | "extracting");
        let mut fx = rect.min.x + 12.0;
        if failed {
            p.circle_filled(egui::pos2(fx + 3.0, rect.max.y - 15.0), 3.0, theme::DANGER);
            fx += 12.0;
        }
        let g = wrapped(
            format!("{} · {meta}", age(crate::model::now() - e.last_seen)),
            11.0,
            theme::MUTED,
            rect.max.x - fx - 12.0,
            1,
        );
        p.galley(egui::pos2(fx, rect.max.y - 22.0), g, theme::MUTED);
        if e.pinned {
            p.text(
                rect.right_bottom() + vec2(-14.0, -15.0),
                Align2::CENTER_CENTER,
                "★",
                FontId::proportional(12.0),
                theme::ACCENT,
            );
        }
        // Pin/delete float in on hover.
        let at = ctx.animate_bool_with_time(r.id.with("acts"), hov, 0.12);
        let (pin, del) = (
            egui::Rect::from_center_size(rect.right_top() + vec2(-44.0, 16.0), vec2(22.0, 22.0)),
            egui::Rect::from_center_size(rect.right_top() + vec2(-18.0, 16.0), vec2(22.0, 22.0)),
        );
        if at > 0.01 {
            for (rc, glyph, tip, id) in [
                (
                    pin,
                    if e.pinned { "★" } else { "☆" },
                    if e.pinned { "Unpin" } else { "Pin" },
                    "pin",
                ),
                (del, "", "Delete", "del"),
            ] {
                let b = ui.interact(rc, ui.id().with((id, e.id)), Sense::click());
                let fill = if b.hovered() {
                    theme::LINE
                } else {
                    theme::BG.gamma_multiply(0.85)
                };
                p.circle_filled(
                    rc.center(),
                    11.0 * (0.7 + 0.3 * at),
                    fill.gamma_multiply(at),
                );
                if id == "del" {
                    cross(&p, rc.center(), 3.5, theme::TEXT.gamma_multiply(at));
                } else {
                    p.text(
                        rc.center(),
                        Align2::CENTER_CENTER,
                        glyph,
                        FontId::proportional(11.5),
                        theme::TEXT.gamma_multiply(at),
                    );
                }
                if b.on_hover_text(tip).clicked() {
                    acts.push(if id == "pin" {
                        Act::Pin(e.id, !e.pinned)
                    } else {
                        Act::Del(e.id)
                    });
                }
            }
        }
        if r.clicked()
            && !(hov
                && (ui
                    .input(|i| i.pointer.interact_pos())
                    .is_some_and(|q| pin.contains(q) || del.contains(q))))
        {
            acts.push(Act::Select(i));
        }
        if r.double_clicked() {
            acts.push(Act::Open(i));
        }
        r.context_menu(|ui| {
            for (label, act) in [
                ("Copy", Act::Copy(i)),
                ("Paste", Act::Paste(i)),
                (
                    if e.pinned { "Unpin" } else { "Pin" },
                    Act::Pin(e.id, !e.pinned),
                ),
                ("Delete", Act::Del(e.id)),
            ] {
                if ui.button(label).clicked() {
                    acts.push(act);
                    ui.close();
                }
            }
        });
    }
    /// A round search button that springs open into a text field, plus a mode chip once open.
    fn search_bar(&mut self, ui: &mut egui::Ui) {
        use egui::{Rect, Sense, Stroke, vec2};
        let ctx = ui.ctx().clone();
        let id = egui::Id::new("search-pill");
        let open = self.search_open || !self.query.is_empty();
        let (secs, ease): (f32, fn(f32) -> f32) = if open {
            (0.42, egui::emath::easing::back_out)
        } else {
            (0.2, egui::emath::easing::cubic_out)
        };
        let t = ctx.animate_bool_with_time_and_easing(id, open, secs, ease);
        let tc = t.clamp(0.0, 1.0);
        let full = (ui.available_width() * 0.4).clamp(220.0, 420.0);
        let w = (PILL + (full - PILL) * t).max(PILL);
        let (rect, r) = ui.allocate_exact_size(vec2(w, PILL), Sense::click());
        let focused = ctx.memory(|m| m.has_focus(search_id()));
        let h = ctx.animate_bool_with_time(id.with("h"), r.hovered() || focused, 0.12);
        // A slight vertical squash while the width is still moving gives the open a bubbly feel.
        let squash = if t > 0.0 && t < 1.0 {
            (t * std::f32::consts::PI).sin() * 2.5
        } else {
            0.0
        };
        let pill = rect.shrink2(vec2(0.0, squash));
        let p = ui.painter().clone();
        p.rect_filled(
            pill,
            (PILL / 2.0) as u8,
            theme::SURFACE.lerp_to_gamma(theme::SURFACE_HI, h * (1.0 - tc * 0.5)),
        );
        let edge = if focused {
            theme::LINE.lerp_to_gamma(theme::ACCENT, tc)
        } else {
            theme::LINE
        };
        p.rect_stroke(
            pill,
            (PILL / 2.0) as u8,
            Stroke::new(1.0_f32, edge),
            egui::StrokeKind::Inside,
        );
        magnifier(
            &p,
            egui::pos2(rect.min.x + PILL / 2.0, rect.center().y),
            theme::MUTED.lerp_to_gamma(theme::TEXT, h.max(tc)),
        );
        if r.clicked() && !open {
            self.search_open = true;
            self.focus_search = true;
        }
        r.on_hover_text_at_pointer("Search (or just start typing)");
        let mut area = rect;
        if tc > 0.25 {
            let has_text = !self.query.is_empty();
            let text_rect = Rect::from_min_max(
                rect.min + vec2(PILL - 4.0, 0.0),
                rect.max - vec2(if has_text { 30.0 } else { 12.0 }, 0.0),
            );
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(text_rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            child.multiply_opacity(((tc - 0.25) / 0.75).min(1.0));
            let resp = child.add(
                egui::TextEdit::singleline(&mut self.query)
                    .id(search_id())
                    .frame(false)
                    .hint_text("Search clipboard")
                    .font(egui::FontId::proportional(14.0))
                    .text_color(theme::TEXT)
                    .desired_width(text_rect.width()),
            );
            if self.focus_search {
                resp.request_focus();
                self.focus_search = false;
            }
            if resp.changed() {
                self.refresh();
            }
            if has_text {
                let x = Rect::from_center_size(
                    egui::pos2(rect.max.x - 17.0, rect.center().y),
                    vec2(20.0, 20.0),
                );
                let xr = ui.interact(x, id.with("clear"), Sense::click());
                if xr.hovered() {
                    p.circle_filled(x.center(), 10.0, theme::LINE);
                }
                cross(
                    &p,
                    x.center(),
                    3.5,
                    theme::MUTED.lerp_to_gamma(theme::TEXT, tc),
                );
                if xr.clicked() {
                    self.query.clear();
                    self.search_open = false;
                    ctx.memory_mut(|m| m.surrender_focus(search_id()));
                    self.refresh();
                }
            }
        }
        // Search mode lives behind one quiet chip that only shows while searching.
        if tc > 0.5 {
            let cloud = self.engine.config.read().unwrap().cloud_enabled;
            let label = match self.mode {
                SearchMode::Keyword => "Keyword",
                SearchMode::Hybrid => "Hybrid",
                SearchMode::Semantic => "Semantic",
            };
            let chip = ui
                .scope(|ui| {
                    ui.multiply_opacity(((tc - 0.5) * 2.0).min(1.0));
                    ui.add(
                        egui::Button::new(
                            egui::RichText::new(label).size(12.5).color(theme::MUTED),
                        )
                        .fill(theme::SURFACE)
                        .stroke(Stroke::new(1.0_f32, theme::LINE))
                        .corner_radius(14)
                        .min_size(vec2(0.0, 28.0)),
                    )
                })
                .inner;
            area = area.union(chip.rect);
            self.mode_menu = egui::Popup::menu(&chip).show(|ui| {
                for (mode, name, tip) in [
                    (SearchMode::Keyword, "Keyword", "Exact words in text and file contents"),
                    (SearchMode::Hybrid, "Hybrid", "Keywords first, then meaning-based matches"),
                    (SearchMode::Semantic, "Semantic", "Meaning-based matches from your embeddings"),
                ] {
                    if ui.selectable_value(&mut self.mode, mode, name).on_hover_text(tip).clicked() {
                        self.refresh();
                        self.focus_search = true;
                        ui.close();
                    }
                }
                if !cloud {
                    ui.separator();
                    ui.label(egui::RichText::new("Cloud processing is off, so semantic\nsearch falls back to keywords.").small().color(theme::MUTED));
                } else if !self.semantic && !self.query.is_empty() {
                    ui.separator();
                    ui.label(egui::RichText::new("Semantic results follow when ready.").small().color(theme::MUTED));
                }
            }).is_some();
        } else {
            self.mode_menu = false;
        }
        // Fold back into a button once the field is empty, unfocused and the pointer has moved on.
        if self.search_open
            && self.query.is_empty()
            && !focused
            && !self.focus_search
            && !self.mode_menu
            && !ui.rect_contains_pointer(area.expand(6.0))
        {
            self.search_open = false;
        }
    }
    fn settings(&mut self, ctx: &egui::Context) {
        let mut open = self.settings;
        egui::Window::new("Settings").open(&mut open).anchor(egui::Align2::CENTER_CENTER,[0.0,0.0]).default_width(520.0).resizable(true).show(ctx,|ui|{
            egui::ScrollArea::vertical().max_height((ctx.content_rect().height()-70.0).max(200.0)).show(ui,|ui|{
                ui.heading("Cloud semantic search");
                ui.label("Text, supported file contents, image thumbnails, and search queries are sent to your cloud provider. History and indexes stay on this device. No telemetry.");
                ui.checkbox(&mut self.draft.cloud_enabled,"Enable cloud processing of my history and queries");
                ui.horizontal(|ui|{ui.selectable_value(&mut self.draft.provider,Provider::OpenRouter,"OpenRouter");ui.selectable_value(&mut self.draft.provider,Provider::Fireworks,"Fireworks");});
                ui.label("Embedding model ID");ui.text_edit_singleline(&mut self.draft.embedding_model);
                ui.horizontal(|ui|{ui.selectable_value(&mut self.draft.image_embedding,ImageEmbedding::Native,"Native multimodal (OpenRouter)");ui.selectable_value(&mut self.draft.image_embedding,ImageEmbedding::Caption,"Vision caption + text (Fireworks compatible)");});
                ui.label("Vision model ID (Caption mode only)");ui.text_edit_singleline(&mut self.draft.vision_model);
                ui.label(format!("API key: set {} in your launch environment. Keys are never saved here.",self.draft.api_key_env()));
                ui.label(if std::env::var(self.draft.api_key_env()).is_ok_and(|v|!v.trim().is_empty()){"API key is present."}else{"API key is missing."});
                ui.separator();ui.heading("Capture and privacy");
                ui.checkbox(&mut self.draft.paused,"Pause capture and background indexing");
                ui.checkbox(&mut self.draft.capture_files,"Capture regular files (managed copies)");
                ui.checkbox(&mut self.draft.capture_when_app_unknown,"Capture when source app is unknown (exclusions cannot be enforced)");
                ui.label("Excluded app names / bundle IDs / executable paths (one substring per line, case-insensitive)");
                ui.add(egui::TextEdit::multiline(&mut self.exclusions).desired_rows(5));
                ui.horizontal(|ui|{ui.label("History entries");ui.add(egui::DragValue::new(&mut self.draft.max_entries).range(1..=1_000_000));});
                ui.horizontal(|ui|{ui.label("Keep days (0 = unlimited)");ui.add(egui::DragValue::new(&mut self.draft.max_age_days).range(0..=3650));});
                ui.horizontal(|ui|{ui.label("Storage MiB");ui.add(egui::DragValue::new(&mut self.draft.max_storage_mb).range(1..=102400));});
                ui.horizontal(|ui|{ui.label("Item size MiB");ui.add(egui::DragValue::new(&mut self.draft.max_item_mb).range(1..=100));});
                ui.horizontal(|ui|{ui.label("Poll interval ms");ui.add(egui::DragValue::new(&mut self.draft.poll_ms).range(200..=60000));});
                ui.label("Pinned entries survive retention cleanup and may exceed limits.");
                ui.separator();ui.heading("Desktop");ui.label("Global shortcut (restart required)");ui.text_edit_singleline(&mut self.draft.hotkey);
                ui.checkbox(&mut self.draft.auto_paste,"Paste automatically after Enter (requires input permissions)");
                if cfg!(target_os="linux"){ui.label("Wayland: global shortcuts, app identification, and simulated paste may be unavailable. Default capture fails closed. Use an X11 session for full support.");}
                if cfg!(target_os="macos"){ui.label("macOS: allow Accessibility for paste and Automation → System Events for source-app exclusions.");}
                ui.separator();
                if ui.button("Save settings").clicked(){
                    self.draft.excluded_apps=self.exclusions.lines().map(str::trim).filter(|s|!s.is_empty()).map(String::from).collect();
                    match self.draft.validate(){Ok(())=>self.command(Command::Save(self.draft.clone())),Err(e)=>self.status=e.to_string()}
                }
                if ui.button("Rebuild semantic index").clicked(){self.command(Command::Reindex);}
                ui.checkbox(&mut self.confirm_clear,"I understand clearing history also removes pinned entries and managed files");
                if ui.add_enabled(self.confirm_clear,egui::Button::new("Clear all history")).clicked(){self.command(Command::Clear);self.confirm_clear=false;self.thumbs.clear();self.thumb_req.clear();}
                ui.label(format!("Data: {}",self.paths.root.display()));
            });
        });
        self.settings = open;
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(100));
        self.frame += 1;
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
            if self.hotkey_id.is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.hide(ctx);
            } else {
                self.quitting = true;
            }
        }
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            if Some(event.id) == self.hotkey_id && event.state == HotKeyState::Pressed {
                self.show(ctx);
            }
        }
        self.receive(ctx);
        if self.pending_search
            && self.edited.elapsed() > Duration::from_millis(100)
            && self.engine.search(SearchRequest {
                generation: self.generation,
                query: self.query.clone(),
                mode: self.mode,
            })
        {
            self.pending_search = false;
        }
        if !self.visible {
            return;
        }
        if self.docked {
            if self.tall != self.settings {
                self.tall = self.settings;
                self.dock_frames = 3;
            }
            if self.dock_frames > 0 {
                self.dock(ctx);
                self.dock_frames -= 1;
                ctx.request_repaint();
            }
            if ctx.input(|i| i.viewport().focused).unwrap_or(true) {
                self.was_focused = true;
            } else if self.was_focused
                && !self.settings
                && self.shown_at.elapsed() > Duration::from_millis(300)
            {
                self.hide(ctx);
                return;
            }
        }
        let n = self.hits.len();
        if !self.settings {
            let editing = ctx.memory(|m| m.has_focus(search_id()));
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
                if !self.query.is_empty() {
                    self.query.clear();
                    self.refresh();
                } else if self.search_open {
                    self.search_open = false;
                    ctx.memory_mut(|m| m.surrender_focus(search_id()));
                } else if self.hotkey_id.is_some() {
                    self.hide(ctx);
                }
            }
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
                self.search_open = true;
                self.focus_search = true;
            }
            // Typing anywhere opens search with what was typed.
            if !editing {
                let typed: String = ctx.input(|i| {
                    i.events
                        .iter()
                        .filter_map(|e| match e {
                            egui::Event::Text(t) => Some(t.as_str()),
                            _ => None,
                        })
                        .collect()
                });
                if !typed.trim().is_empty() {
                    ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(_))));
                    self.query.push_str(&typed);
                    let mut state =
                        egui::TextEdit::load_state(ctx, search_id()).unwrap_or_default();
                    state
                        .cursor
                        .set_char_range(Some(egui::text::CCursorRange::one(
                            egui::text::CCursor::new(self.query.chars().count()),
                        )));
                    state.store(ctx, search_id());
                    self.search_open = true;
                    self.focus_search = true;
                    self.refresh();
                }
            }
            let key =
                |ctx: &egui::Context, k| ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, k));
            let before = self.selected;
            if key(ctx, egui::Key::ArrowRight) || key(ctx, egui::Key::ArrowDown) {
                self.selected = (self.selected + 1).min(n.saturating_sub(1));
            }
            if key(ctx, egui::Key::ArrowLeft) || key(ctx, egui::Key::ArrowUp) {
                self.selected = self.selected.saturating_sub(1);
            }
            if self.query.is_empty() {
                if key(ctx, egui::Key::Home) {
                    self.selected = 0;
                }
                if key(ctx, egui::Key::End) {
                    self.selected = n.saturating_sub(1);
                }
                if key(ctx, egui::Key::Delete)
                    && let Some(h) = self.hits.get(self.selected)
                {
                    let id = h.entry.id;
                    self.command(Command::Delete(id));
                    self.thumbs.remove(&id);
                    self.thumb_req.remove(&id);
                }
            }
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::P))
                && let Some(h) = self.hits.get(self.selected)
            {
                let (id, pinned) = (h.entry.id, h.entry.pinned);
                self.command(Command::Pin(id, !pinned));
            }
            if self.selected != before {
                self.scroll_to_selected = true;
            }
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
                let paste = self.engine.config.read().unwrap().auto_paste;
                self.restore(ctx, paste);
            }
        }
        let frame = egui::Frame::new()
            .fill(theme::BG)
            .inner_margin(egui::Margin::symmetric(16, 12))
            .stroke(egui::Stroke::new(1.0_f32, theme::LINE));
        let mut acts = Vec::new();
        if self.status != self.status_seen {
            self.status_seen = self.status.clone();
            self.status_at = Instant::now();
        }
        egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                self.search_bar(ui);
                let mut filter = self.filter;
                if segmented(ui, egui::Id::new("kind-filter"), &["All", "Text", "Images", "Files"], &mut filter) {
                    self.filter = filter;
                    self.refresh();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let gear = icon_button(ui, |p, r, c| {
                        p.text(r.center(), egui::Align2::CENTER_CENTER, "⚙", egui::FontId::proportional(15.0), c);
                    });
                    egui::Popup::menu(&gear).show(|ui| {
                        ui.set_min_width(190.0);
                        ui.label(egui::RichText::new(format!("{} items · {} indexed", self.counts.0, self.counts.1)).small().color(theme::MUTED));
                        ui.separator();
                        if ui.button("Settings").clicked() {
                            self.draft = self.engine.config.read().unwrap().clone();
                            self.exclusions = self.draft.excluded_apps.join("\n");
                            self.settings = true;
                            ui.close();
                        }
                        let paused = self.engine.config.read().unwrap().paused;
                        if ui.button(if paused { "Resume capture" } else { "Pause capture" }).clicked() {
                            let mut c = self.engine.config.read().unwrap().clone();
                            c.paused = !paused;
                            self.command(Command::Save(c));
                            ui.close();
                        }
                        if ui.button("Quit ClipVault").clicked() {
                            self.quitting = true;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        ui.separator();
                        ui.label(egui::RichText::new("Type to search · arrow keys select\nEnter paste · right-click for more · Esc hide").small().color(theme::MUTED));
                    });
                    // Status messages appear briefly, then shrink to a dot that keeps them on hover.
                    if !self.status.is_empty() {
                        let fade = 1.0 - ((self.status_at.elapsed().as_secs_f32() - 5.0) / 0.4).clamp(0.0, 1.0);
                        if fade > 0.0 {
                            ui.scope(|ui| {
                                ui.set_max_width((ui.available_width() - 40.0).clamp(0.0, 420.0));
                                ui.add(egui::Label::new(egui::RichText::new(&self.status).small().color(theme::MUTED.gamma_multiply(fade))).truncate());
                            });
                            ctx.request_repaint_after(Duration::from_millis(50));
                        } else {
                            let (r, resp) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                            ui.painter().circle_filled(r.center(), 3.5, theme::WARN);
                            resp.on_hover_text(&self.status);
                        }
                    }
                });
            });
            ui.add_space(4.0);
            let card_h = (ui.available_height() - 4.0).max(80.0);
            if n == 0 {
                ui.add_space(card_h / 2.0 - 12.0);
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new(if self.query.is_empty() { "Nothing here yet. Copy text, an image, or a file." } else { "No matches." }).color(theme::MUTED));
                });
            }
            let hits = std::mem::take(&mut self.hits);
            let (step, sel, scroll) = (CARD_W + GAP, self.selected, std::mem::take(&mut self.scroll_to_selected));
            if n > 0 {
                egui::ScrollArea::horizontal().id_salt("strip").auto_shrink([false, true]).max_height(card_h).scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded).show_viewport(ui, |ui, vp| {
                    // Top padding leaves room for the hover lift and selection glow.
                    let (area, _) = ui.allocate_exact_size(egui::vec2((n as f32 * step - GAP).max(1.0) + 8.0, card_h - 12.0), egui::Sense::hover());
                    let card_rect = |i: usize| egui::Rect::from_min_size(area.min + egui::vec2(4.0 + i as f32 * step, 6.0), egui::vec2(CARD_W, area.height() - 10.0));
                    let range = visible_range(vp.min.x, vp.max.x, step, n);
                    for (i, hit) in hits.iter().enumerate().skip(range.start).take(range.len()) {
                        self.card(ui, card_rect(i), i, hit, &mut acts);
                    }
                    if scroll {
                        ui.scroll_to_rect(card_rect(sel), Some(egui::Align::Center));
                    }
                });
            }
            self.hits = hits;
        });
        for act in acts {
            match act {
                Act::Select(i) => self.selected = i,
                Act::Open(i) => {
                    self.selected = i;
                    let paste = self.engine.config.read().unwrap().auto_paste;
                    self.restore(ctx, paste);
                }
                Act::Copy(i) | Act::Paste(i) => {
                    self.selected = i;
                    self.restore(ctx, matches!(act, Act::Paste(_)));
                }
                Act::Pin(id, pin) => self.command(Command::Pin(id, pin)),
                Act::Del(id) => {
                    self.command(Command::Delete(id));
                    self.thumbs.remove(&id);
                    self.thumb_req.remove(&id);
                }
            }
        }
        if self.settings {
            self.settings(ctx);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn visible_range_covers_viewport_with_slack() {
        assert_eq!(visible_range(0.0, 500.0, 210.0, 100), 0..4);
        assert_eq!(visible_range(2100.0, 2600.0, 210.0, 100), 9..14);
        assert_eq!(visible_range(0.0, 500.0, 210.0, 2), 0..2);
        assert_eq!(visible_range(9000.0, 9500.0, 210.0, 3), 3..3);
    }
    #[test]
    fn card_helpers() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(
            file_parts("a.PDF, b.txt"),
            ("a.PDF".into(), "pdf".into(), 2)
        );
        assert_eq!(age(120), "2m");
    }
}
