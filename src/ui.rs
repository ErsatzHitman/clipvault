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
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

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
    preview_id: Option<i64>,
    preview_text: String,
    texture: Option<egui::TextureHandle>,
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
        cc.egui_ctx.set_style(egui::Style {
            spacing: egui::Spacing {
                item_spacing: egui::vec2(10.0, 9.0),
                button_padding: egui::vec2(10.0, 6.0),
                ..Default::default()
            },
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
            preview_id: None,
            preview_text: String::new(),
            texture: None,
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
                        self.visible = false;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                    }
                }
                Err(_) => self.status = "Clipboard worker is busy. Try again.".into(),
            }
        }
    }
    fn show(&mut self, ctx: &egui::Context) {
        self.visible = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        self.focus_search = true;
        self.refresh();
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
                Event::Preview { id, image, text } => {
                    if self.preview_id == Some(id) {
                        self.preview_text = text;
                        self.texture = image.map(|(w, h, pixels)| {
                            ctx.load_texture(
                                format!("preview-{id}"),
                                egui::ColorImage::from_rgba_unmultiplied([w, h], &pixels),
                                egui::TextureOptions::LINEAR,
                            )
                        });
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
    fn settings(&mut self, ctx: &egui::Context) {
        let mut open = self.settings;
        egui::Window::new("Settings").open(&mut open).default_width(520.0).resizable(true).show(ctx,|ui|{
            egui::ScrollArea::vertical().max_height(620.0).show(ui,|ui|{
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
                if ui.add_enabled(self.confirm_clear,egui::Button::new("Clear all history")).clicked(){self.command(Command::Clear);self.confirm_clear=false;self.preview_id=None;self.texture=None;self.preview_text.clear();}
                ui.label(format!("Data: {}",self.paths.root.display()));
            });
        });
        self.settings = open;
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(100));
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
            if self.hotkey_id.is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                self.visible = false;
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
        if !self.settings {
            if self.hotkey_id.is_some()
                && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
            {
                self.visible = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)) {
                self.selected = (self.selected + 1).min(self.hits.len().saturating_sub(1));
            }
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)) {
                self.selected = self.selected.saturating_sub(1);
            }
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
                let paste = self.engine.config.read().unwrap().auto_paste;
                self.restore(ctx, paste);
            }
        }
        egui::TopBottomPanel::top("search").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("ClipVault");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Quit").clicked() {
                        self.quitting = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if ui.button("Settings").clicked() {
                        self.draft = self.engine.config.read().unwrap().clone();
                        self.exclusions = self.draft.excluded_apps.join("\n");
                        self.settings = true;
                    }
                    let paused = self.engine.config.read().unwrap().paused;
                    if ui.button(if paused { "Resume" } else { "Pause" }).clicked() {
                        let mut c = self.engine.config.read().unwrap().clone();
                        c.paused = !paused;
                        self.command(Command::Save(c));
                    }
                });
            });
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("Search your clipboard…")
                    .desired_width(f32::INFINITY),
            );
            if self.focus_search {
                response.request_focus();
                self.focus_search = false;
            }
            if response.changed() {
                self.refresh();
            }
            ui.horizontal(|ui| {
                for (mode, label) in [
                    (SearchMode::Keyword, "Keyword"),
                    (SearchMode::Hybrid, "Hybrid"),
                    (SearchMode::Semantic, "Semantic"),
                ] {
                    if ui.selectable_value(&mut self.mode, mode, label).changed() {
                        self.refresh();
                    }
                }
                ui.separator();
                for (i, label) in ["All", "Text", "Images", "Files"].into_iter().enumerate() {
                    if ui.selectable_value(&mut self.filter, i, label).changed() {
                        self.refresh();
                    }
                }
            });
        });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.small(format!(
                    "{} items · {} indexed · {}",
                    self.counts.0,
                    self.counts.1,
                    if self.semantic {
                        "semantic ready"
                    } else {
                        "keyword results"
                    }
                ));
                ui.small("↑↓ select · Enter paste · Esc hide");
            });
            if !self.status.is_empty() {
                ui.small(&self.status);
            }
        });
        egui::SidePanel::left("history").default_width(360.0).min_width(240.0).resizable(true).show(ctx,|ui|{
            egui::ScrollArea::vertical().auto_shrink([false,false]).show(ui,|ui|{
                if self.hits.is_empty(){ui.add_space(25.0);ui.label(if self.query.is_empty(){"Copy text, an image, or files to get started. Check capture permissions in Settings."}else{"No matching items."});}
                for (i,hit) in self.hits.iter().enumerate(){let e=&hit.entry;let icon=match e.kind{Kind::Text=>"Text",Kind::Image=>"Image",Kind::Files=>"Files"};
                    let label=format!("{}{} · {}\n{}",if e.pinned{"★ "}else{""},icon,e.title,e.source_app);
                    let r=ui.add(egui::Button::selectable(self.selected==i,label).min_size(egui::vec2(ui.available_width(),56.0)));
                    if r.clicked(){self.selected=i;}if r.double_clicked(){self.selected=i;}
                    if self.selected==i && ctx.input(|i|i.key_pressed(egui::Key::ArrowDown)||i.key_pressed(egui::Key::ArrowUp)){r.scroll_to_me(Some(egui::Align::Center));}
                }
            });
        });
        egui::CentralPanel::default().show(ctx,|ui|{
            if let Some(e)=self.hits.get(self.selected).map(|h|h.entry.clone()) {
                if self.preview_id!=Some(e.id){self.preview_id=Some(e.id);self.texture=None;self.preview_text="Loading preview…".into();self.command(Command::Preview(e.id));}
                ui.heading(&e.title);ui.small(format!("{} · {} bytes · {}",e.source_app,e.bytes,e.index_status));if let Some(error)=&e.index_error{ui.small(error);}
                ui.horizontal(|ui|{if ui.button("Copy").clicked(){self.restore(ctx,false);}if ui.button("Paste").clicked(){self.restore(ctx,true);}if ui.button(if e.pinned{"Unpin"}else{"Pin"}).clicked(){self.command(Command::Pin(e.id,!e.pinned));}if ui.button("Delete").clicked(){self.command(Command::Delete(e.id));self.preview_id=None;}});
                ui.separator();egui::ScrollArea::both().show(ui,|ui|{if let Some(texture)=&self.texture{ui.add(egui::Image::new(texture).max_width(ui.available_width()).shrink_to_fit());}else{ui.add(egui::Label::new(egui::RichText::new(&self.preview_text).monospace()).wrap());}});
            }else{ui.heading("Your clipboard, searchable.");ui.label("History is stored on this device. Enable cloud processing in Settings for meaning-based search over text and images.");}
        });
        if self.settings {
            self.settings(ctx);
        }
    }
}
