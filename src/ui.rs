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
    egui::Color32::from(egui::ecolor::Hsva::new(hue, 0.55, 0.65, 1.0))
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
        cc.egui_ctx.set_style(egui::Style {
            spacing: egui::Spacing {
                item_spacing: egui::vec2(8.0, 8.0),
                button_padding: egui::vec2(10.0, 5.0),
                ..Default::default()
            },
            always_scroll_the_only_direction: true,
            ..Default::default()
        });
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
            focus_search: true,
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
        self.focus_search = true;
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
        let v = ui.visuals().clone();
        let p = ui.painter_at(rect);
        p.rect_filled(
            rect,
            8,
            if hov {
                v.widgets.hovered.bg_fill
            } else {
                v.widgets.inactive.bg_fill
            },
        );
        p.rect_stroke(
            rect,
            8,
            if sel {
                Stroke::new(2.0_f32, v.selection.stroke.color)
            } else {
                Stroke::new(1.0_f32, v.widgets.noninteractive.bg_stroke.color)
            },
            StrokeKind::Inside,
        );
        let dim = v.weak_text_color();
        let (kind, color) = match e.kind {
            Kind::Text => ("TEXT", dim),
            Kind::Image => ("IMAGE", dim),
            Kind::Files => ("FILES", dim),
        };
        p.text(
            rect.min + vec2(10.0, 7.0),
            Align2::LEFT_TOP,
            format!("{}{kind}", if e.pinned { "★ " } else { "" }),
            FontId::proportional(11.0),
            color,
        );
        let body =
            egui::Rect::from_min_max(rect.min + vec2(10.0, 26.0), rect.max - vec2(10.0, 26.0));
        let bp = ui.painter_at(body);
        let draw_thumb = |bp: &egui::Painter, tex: &egui::TextureHandle, area: egui::Rect| {
            let s = tex.size_vec2();
            let k = (area.width() / s.x).min(area.height() / s.y);
            let img = egui::Rect::from_center_size(area.center(), s * k);
            bp.image(
                tex.id(),
                img,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        };
        let one_line = |text: String, font: FontId, color: Color32, w: f32| {
            let mut job = LayoutJob::simple(text, font, color, w);
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = true;
            ui.painter().layout_job(job)
        };
        let mut footer = format!("{} · {}", human_size(e.bytes), e.source_app);
        match e.kind {
            Kind::Text => {
                let t: String = e.text.chars().take(400).collect();
                let t = if t.trim().is_empty() {
                    e.title.clone()
                } else {
                    t.trim().to_owned()
                };
                let g = ui.painter().layout(
                    t,
                    FontId::proportional(13.0),
                    v.text_color(),
                    body.width(),
                );
                bp.galley(body.min, g, v.text_color());
                footer = format!("{} chars · {}", e.text.chars().count(), e.source_app);
            }
            Kind::Image => {
                self.want_thumb(e.id);
                match self.thumb(e.id) {
                    Some(t) => draw_thumb(&bp, &t, body),
                    None => {
                        bp.text(
                            body.center(),
                            Align2::CENTER_CENTER,
                            "…",
                            FontId::proportional(20.0),
                            dim,
                        );
                    }
                }
                footer = format!(
                    "{} · {}",
                    e.title.strip_prefix("Image · ").unwrap_or(&e.title),
                    human_size(e.bytes)
                );
            }
            Kind::Files => {
                let (name, ext, count) = file_parts(&e.title);
                let has_img = is_image_ext(&ext);
                if has_img {
                    self.want_thumb(e.id);
                }
                let tex = if has_img { self.thumb(e.id) } else { None };
                let label = if ext.is_empty() {
                    "FILE".to_owned()
                } else {
                    ext.to_uppercase().chars().take(5).collect()
                };
                let mut text_area = body;
                if let Some(t) = tex {
                    draw_thumb(
                        &bp,
                        &t,
                        egui::Rect::from_min_max(
                            body.min,
                            egui::pos2(body.max.x, body.max.y - 18.0),
                        ),
                    );
                    text_area.min.y = body.max.y - 16.0;
                    let g = one_line(
                        name,
                        FontId::proportional(12.0),
                        v.text_color(),
                        body.width(),
                    );
                    bp.galley(text_area.min, g, v.text_color());
                } else {
                    let badge = egui::Rect::from_min_size(body.min, vec2(48.0, 60.0));
                    bp.rect_filled(badge, 6, badge_color(&ext));
                    bp.text(
                        badge.center(),
                        Align2::CENTER_CENTER,
                        label,
                        FontId::proportional(13.0),
                        Color32::WHITE,
                    );
                    let w = body.width() - 56.0;
                    let mut job =
                        LayoutJob::simple(name, FontId::proportional(13.0), v.text_color(), w);
                    job.wrap.max_rows = 3;
                    job.wrap.break_anywhere = true;
                    let g = ui.painter().layout_job(job);
                    bp.galley(body.min + vec2(56.0, 0.0), g, v.text_color());
                }
                footer = if count > 1 {
                    format!("+{} more · {}", count - 1, human_size(e.bytes))
                } else {
                    format!(
                        "{} · {}",
                        if ext.is_empty() { "file" } else { &ext },
                        human_size(e.bytes)
                    )
                };
            }
        }
        let dot = match e.index_status.as_str() {
            "ready" => Color32::from_rgb(80, 190, 110),
            "pending" => Color32::from_rgb(220, 170, 60),
            _ => Color32::from_rgb(210, 80, 80),
        };
        p.circle_filled(egui::pos2(rect.min.x + 14.0, rect.max.y - 13.0), 3.5, dot);
        let g = one_line(
            format!("{footer} · {}", age(crate::model::now() - e.last_seen)),
            FontId::proportional(10.5),
            dim,
            rect.width() - 34.0,
        );
        p.galley(egui::pos2(rect.min.x + 24.0, rect.max.y - 20.0), g, dim);
        let (pin, del) = (
            egui::Rect::from_min_size(rect.right_top() + vec2(-48.0, 3.0), vec2(22.0, 20.0)),
            egui::Rect::from_min_size(rect.right_top() + vec2(-25.0, 3.0), vec2(22.0, 20.0)),
        );
        if hov || sel {
            for (rc, glyph, tip, id) in [
                (
                    pin,
                    if e.pinned { "★" } else { "☆" },
                    if e.pinned { "Unpin" } else { "Pin" },
                    "pin",
                ),
                (del, "✖", "Delete", "del"),
            ] {
                let b = ui.interact(rc, ui.id().with((id, e.id)), Sense::click());
                if b.hovered() {
                    p.rect_filled(rc, 4, v.widgets.hovered.bg_fill);
                }
                p.text(
                    rc.center(),
                    Align2::CENTER_CENTER,
                    glyph,
                    FontId::proportional(13.0),
                    v.text_color(),
                );
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
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
                if !self.query.is_empty() {
                    self.query.clear();
                    self.refresh();
                } else if self.hotkey_id.is_some() {
                    self.hide(ctx);
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
        let accent = ctx.style().visuals.selection.bg_fill;
        let frame = egui::Frame::new()
            .fill(ctx.style().visuals.panel_fill)
            .inner_margin(egui::Margin::symmetric(14, 8))
            .stroke(egui::Stroke::new(2.0_f32, accent));
        let mut acts = Vec::new();
        egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
            ui.horizontal(|ui| {
                let response = ui.add(egui::TextEdit::singleline(&mut self.query).hint_text("Search clipboard…").desired_width((ui.available_width() * 0.4).clamp(180.0, 520.0)));
                if self.focus_search {
                    response.request_focus();
                    self.focus_search = false;
                }
                if response.changed() {
                    self.refresh();
                }
                let cloud = self.engine.config.read().unwrap().cloud_enabled;
                let tip = if !cloud { "Cloud processing is off: semantic search falls back to keywords" } else if self.semantic { "Semantic index ready" } else { "Keyword results shown; semantic results follow when ready" };
                for (mode, label) in [(SearchMode::Keyword, "Keyword"), (SearchMode::Hybrid, "Hybrid"), (SearchMode::Semantic, "Semantic")] {
                    if ui.selectable_value(&mut self.mode, mode, label).on_hover_text(tip).changed() {
                        self.refresh();
                    }
                }
                ui.separator();
                for (i, label) in ["All", "Text", "Images", "Files"].into_iter().enumerate() {
                    if ui.selectable_value(&mut self.filter, i, label).changed() {
                        self.refresh();
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.menu_button("⚙", |ui| {
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
                    });
                    ui.small(format!("{} items · {} indexed", self.counts.0, self.counts.1));
                });
            });
            let card_h = (ui.available_height() - 24.0).max(80.0);
            if n == 0 {
                ui.add_space(card_h / 3.0);
                ui.vertical_centered(|ui| {
                    ui.label(if self.query.is_empty() { "Copy text, an image, or files to get started. Check capture permissions in Settings (⚙)." } else { "No matching items." });
                });
            }
            let hits = std::mem::take(&mut self.hits);
            let (step, sel, scroll) = (CARD_W + GAP, self.selected, std::mem::take(&mut self.scroll_to_selected));
            if n > 0 {
                egui::ScrollArea::horizontal().id_salt("strip").auto_shrink([false, true]).max_height(card_h).show_viewport(ui, |ui, vp| {
                    let (area, _) = ui.allocate_exact_size(egui::vec2((n as f32 * step - GAP).max(1.0), card_h - 14.0), egui::Sense::hover());
                    let card_rect = |i: usize| egui::Rect::from_min_size(area.min + egui::vec2(i as f32 * step, 0.0), egui::vec2(CARD_W, area.height()));
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
            ui.small(if self.status.is_empty() { "←/→ select · Enter paste · Esc hide · right-click for more".to_owned() } else { self.status.clone() });
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
