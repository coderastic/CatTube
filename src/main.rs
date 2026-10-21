//! CatTube: a native Linux YouTube app for TV screens and remotes.
//!
//! One full-screen window, driven with arrows, OK (Enter) and Back (Esc).
//! The screens' logic lives in `cattube_core::tube`; this file runs the
//! window, the background jobs (YouTube requests, pictures, sign-in, the
//! yt-dlp updates) and the player.

mod canvas;
mod icons;
mod images;
mod jobs;
mod keys;
mod pointer;
mod prefs;
mod remote;
mod target;
mod theme;
mod ui;
mod widgets;
mod yt;

use canvas::Canvas;
use cattube_core::account::{self, Accounts};
use cattube_core::input::{Action, InputMapper};
use cattube_core::player::{self, Codec, PlayOptions, RESOLUTIONS, SPEEDS};
use cattube_core::scan::Scan;
use cattube_core::tube::{Ctx, Effect, Screen, Section, SettingRow, Tube};
use cattube_core::youtube::{self as ytc, Details, Page, Tile};
use eframe::egui;
use jobs::Jobs;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Child;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use target::Target;

enum Msg {
    Page(String, Result<Page, String>),
    More(String, String, Result<Page, String>),
    Search(Result<Page, String>),
    SearchMore(Result<Page, String>),
    Suggestions(String, Vec<String>),
    Details(String, Result<Details, String>),
    Votes(String, Option<(u64, u64)>),
    SignInCode(Result<(String, String), String>),
    TvCode(Result<String, String>),
    SignedIn(Result<(String, String, u64), String>),
    AccountName(String, Option<(String, String)>),
    Done(String),
    /// A background job with nothing to show.
    Quiet,
    Image(String, Option<String>),
    Tools(Result<String, String>),
    PlaybackDone(yt::Ended),
}

struct App {
    tube: Tube,
    accounts: Accounts,
    prefs: prefs::Prefs,
    config: PathBuf,
    cache: PathBuf,
    data: PathBuf,
    jobs: Jobs<Msg>,
    mapper: InputMapper,
    pointer: pointer::Pointer,
    hits: Vec<(egui::Rect, Target)>,
    images: images::Images,
    pictures: HashMap<String, Option<String>>,
    wanted: RefCell<Vec<String>>,
    fetching: HashSet<String>,
    toast: Option<(String, Instant)>,
    /// Bumped to stop waiting for a sign-in code.
    sign_in_wait: Arc<AtomicU64>,
    player: Option<Child>,
    positions: Vec<(String, f64)>,
    votes: HashMap<String, (u64, u64)>,
    tools: Scan<String>,
    /// Started with `--play`: close when the video ends.
    quit_after_play: bool,
    /// When the phone code on screen was asked for (a new one every 4 min).
    tv_code_at: Option<Instant>,
    was_focused: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, start: Start) -> Self {
        theme::setup(&cc.egui_ctx);
        let config = cattube_core::env::config_dir();
        let cache = cattube_core::env::cache_dir();
        let data = player::data_home();
        let prefs = prefs::load(&config);
        theme::set_accent(&prefs.accent);
        let accounts = account::load(&config);
        let positions = std::fs::read_to_string(data.join("positions.conf"))
            .map(|t| player::parse_positions(&t))
            .unwrap_or_default();
        let ctx = Ctx {
            signed_in: accounts.active().is_some(),
            accounts: accounts.list.len(),
            sponsorblock: prefs.sponsorblock,
        };
        let (mut tube, mut effects) = Tube::open(ctx);
        let mut app = App {
            tube: tube.clone(),
            accounts,
            prefs,
            config,
            cache,
            data: data.clone(),
            jobs: Jobs::new(&cc.egui_ctx),
            mapper: InputMapper::new(),
            pointer: pointer::Pointer::new(),
            hits: Vec::new(),
            images: images::Images::new(),
            pictures: HashMap::new(),
            wanted: RefCell::new(Vec::new()),
            fetching: HashSet::new(),
            toast: None,
            sign_in_wait: Arc::new(AtomicU64::new(0)),
            player: None,
            positions,
            votes: HashMap::new(),
            tools: Scan::Busy,
            quit_after_play: false,
            was_focused: true,
            tv_code_at: None,
        };
        app.jobs.spawn(move || Msg::Tools(yt::ensure_tools(&data)));
        match start {
            Start::Play(id, title) => {
                app.quit_after_play = true;
                effects.clear();
                let tile = Tile {
                    id: id.clone(),
                    title,
                    ..Tile::default()
                };
                app.play(&cc.egui_ctx, tile, false, Vec::new());
            }
            Start::Section(s) => {
                tube.header = true;
                tube.header_index = Section::ALL.iter().position(|x| *x == s).unwrap_or(0);
                effects.extend(tube.handle(Action::Ok, ctx, &|_| false));
            }
            Start::Home => {}
        }
        app.tube = tube;
        for e in effects {
            app.effect(&cc.egui_ctx, e);
        }
        app
    }

    fn ctx(&self) -> Ctx {
        Ctx {
            signed_in: self.accounts.active().is_some(),
            accounts: self.accounts.list.len(),
            sponsorblock: self.prefs.sponsorblock,
        }
    }

    fn yt_ctx(&self) -> String {
        ytc::context("en", &self.prefs.region)
    }

    fn refresh(&self) -> Option<String> {
        self.accounts.active().map(|a| a.refresh_token.clone())
    }

    fn toast(&mut self, ctx: &egui::Context, text: &str) {
        self.toast = Some((text.to_string(), Instant::now() + Duration::from_secs(3)));
        ctx.request_repaint();
    }

    fn save_accounts(&mut self, ctx: &egui::Context) {
        if let Err(e) = account::save(&self.config, &self.accounts) {
            self.toast(ctx, &format!("Could not save the account ({e})"));
        }
    }

    fn save_prefs(&mut self, ctx: &egui::Context) {
        if let Err(e) = prefs::save(&self.config, &self.prefs) {
            self.toast(ctx, &format!("Could not save the settings ({e})"));
        }
    }

    fn resume_at(&self, id: &str) -> Option<f64> {
        self.positions
            .iter()
            .find(|(x, _)| x == id)
            .map(|(_, s)| *s)
    }

    fn effect(&mut self, ctx: &egui::Context, e: Effect) {
        let yctx = self.yt_ctx();
        let refresh = self.refresh();
        match e {
            Effect::Load(id) => self.jobs.spawn(move || {
                let r = yt::page(&yctx, &id, refresh.as_deref());
                Msg::Page(id, r)
            }),
            Effect::LoadMore { browse_id, token } => self.jobs.spawn(move || {
                let r = yt::more(&yctx, &token, refresh.as_deref());
                Msg::More(browse_id, token, r)
            }),
            Effect::Search(q) => self
                .jobs
                .spawn(move || Msg::Search(yt::search(&yctx, &q, refresh.as_deref()))),
            Effect::SearchMore(t) => self
                .jobs
                .spawn(move || Msg::SearchMore(yt::search_more(&yctx, &t, refresh.as_deref()))),
            Effect::Suggest(q) => self.jobs.spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                let s = yt::suggestions(&q);
                Msg::Suggestions(q, s)
            }),
            Effect::Details(id) => {
                let id2 = id.clone();
                self.jobs
                    .spawn(move || Msg::Votes(id2.clone(), yt::votes(&id2)));
                self.jobs.spawn(move || {
                    let r = yt::details(&yctx, &id, refresh.as_deref());
                    Msg::Details(id, r)
                })
            }
            Effect::Play { tile, from_start } => {
                // The rest of the row it came from plays after it.
                let queue = self.queue_after(&tile);
                self.play(ctx, *tile, from_start, queue);
            }
            Effect::SignInStart => {
                let generation = self.sign_in_wait.fetch_add(1, Ordering::SeqCst) + 1;
                let wait = self.sign_in_wait.clone();
                let device = self.accounts.device_id.clone();
                let (tx, rctx) = self.jobs.sender();
                std::thread::spawn(move || {
                    let send = |m: Msg| {
                        let _ = tx.send(m);
                        rctx.request_repaint();
                    };
                    let code = match yt::sign_in_code(&device) {
                        Ok(c) => c,
                        Err(e) => return send(Msg::SignInCode(Err(e))),
                    };
                    let url = code
                        .url
                        .trim_start_matches("https://")
                        .trim_start_matches("www.")
                        .to_string();
                    send(Msg::SignInCode(Ok((code.user_code.clone(), url))));
                    let keep = || wait.load(Ordering::SeqCst) == generation;
                    let r = yt::sign_in_wait(&code, &keep);
                    if keep() {
                        send(Msg::SignedIn(r));
                    }
                });
            }
            Effect::TvCode => {
                self.tv_code_at = Some(Instant::now());
                let config = cattube_core::env::config_dir();
                self.jobs.spawn(move || {
                    // The service the phone talks to (CatosTV runs it already).
                    remote::ensure_running();
                    Msg::TvCode(remote::tv_code(&config))
                });
            }
            Effect::SignInStop => {
                self.sign_in_wait.fetch_add(1, Ordering::SeqCst);
            }
            Effect::SwitchAccount(i) => {
                if i < self.accounts.list.len() {
                    self.accounts.active = i;
                    self.save_accounts(ctx);
                    let c = self.ctx();
                    for e in self.tube.reset(c) {
                        self.effect(ctx, e);
                    }
                }
            }
            Effect::SignOut(i) => {
                // Cancelled at Google as well, in the background.
                if let Some(refresh) = self.accounts.list.get(i).map(|a| a.refresh_token.clone()) {
                    self.jobs.spawn(move || {
                        if !yt::revoke(&refresh) {
                            eprintln!("cattube: Google did not confirm the sign-out");
                        }
                        Msg::Quiet
                    });
                }
                self.accounts.remove(i);
                self.save_accounts(ctx);
                self.toast(ctx, "Signed out");
                let c = self.ctx();
                for e in self.tube.reset(c) {
                    self.effect(ctx, e);
                }
            }
            Effect::Rate(id, r) => {
                if let Some(rt) = refresh {
                    self.jobs.spawn(move || {
                        Msg::Done(match yt::rate(&yctx, &id, r, &rt) {
                            Ok(()) => match r {
                                ytc::Rating::Like => "Liked".into(),
                                ytc::Rating::Dislike => "Disliked".into(),
                                ytc::Rating::None => "Rating removed".into(),
                            },
                            Err(e) => e,
                        })
                    });
                }
            }
            Effect::Subscribe { channel_id, on } => {
                if let Some(rt) = refresh {
                    self.jobs.spawn(move || {
                        Msg::Done(match yt::subscribe(&yctx, &channel_id, on, &rt) {
                            Ok(()) if on => "Subscribed".into(),
                            Ok(()) => "Unsubscribed".into(),
                            Err(e) => e,
                        })
                    });
                }
            }
            Effect::WatchLater(id) => {
                if let Some(rt) = refresh {
                    self.jobs.spawn(move || {
                        Msg::Done(match yt::watch_later(&yctx, &id, &rt) {
                            Ok(()) => "Saved to Watch later".into(),
                            Err(e) => e,
                        })
                    });
                }
            }
            Effect::Setting(row, d) => self.setting(ctx, row, d),
            Effect::Close => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Effect::Toast(t) => self.toast(ctx, &t),
        }
    }

    /// The tiles after `tile` in the row it was picked from (a playlist
    /// plays on in order).
    fn queue_after(&self, tile: &Tile) -> Vec<Tile> {
        let page = match self.tube.screen() {
            Some(Screen::Page { page, .. }) => page.items().first().cloned(),
            Some(Screen::Search { results, .. }) => results.items().first().cloned(),
            _ => self.tube.current().and_then(|p| p.items().first().cloned()),
        };
        let Some(p) = page else { return Vec::new() };
        // Only a playlist plays on by itself; other rows leave it to
        // YouTube's own "up next".
        let playlist = matches!(self.tube.screen(), Some(Screen::Page { browse_id, .. }) if browse_id.starts_with("VL"));
        if !playlist {
            return Vec::new();
        }
        p.shelves
            .iter()
            .find(|s| s.tiles.iter().any(|t| t.id == tile.id))
            .map(|s| {
                s.tiles
                    .iter()
                    .skip_while(|t| t.id != tile.id)
                    .skip(1)
                    .filter(|t| t.playable())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    fn setting(&mut self, ctx: &egui::Context, row: SettingRow, d: i32) {
        match row {
            SettingRow::Quality => {
                self.prefs.max_height = prefs::cycle(&RESOLUTIONS, self.prefs.max_height, -d)
            }
            SettingRow::Codec => self.prefs.codec = prefs::cycle(&Codec::ALL, self.prefs.codec, d),
            SettingRow::Hdr => self.prefs.hdr = !self.prefs.hdr,
            SettingRow::Speed => self.prefs.speed = prefs::cycle(&SPEEDS, self.prefs.speed, d),
            SettingRow::Subtitles => self.prefs.subs_on = !self.prefs.subs_on,
            SettingRow::History => self.prefs.history = !self.prefs.history,
            SettingRow::Buffer => {
                self.prefs.buffer = prefs::cycle(&player::BUFFERS, self.prefs.buffer, d)
            }
            SettingRow::Region => {
                let cur = player::REGIONS
                    .iter()
                    .copied()
                    .find(|r| *r == self.prefs.region)
                    .unwrap_or("US");
                self.prefs.region = prefs::cycle(&player::REGIONS, cur, d).to_string();
                // The lists are for the old country: load them again, but
                // stay on this row.
                let at = match self.tube.screen() {
                    Some(Screen::Settings { index, .. }) => *index,
                    _ => 0,
                };
                let c = self.ctx();
                let effects = self.tube.reset(c);
                self.tube.stack.push(Screen::Settings {
                    index: at,
                    groups: false,
                });
                for e in effects {
                    self.effect(ctx, e);
                }
            }
            SettingRow::Autoplay => {
                self.prefs.autoplay = prefs::cycle(&prefs::Autoplay::ALL, self.prefs.autoplay, d)
            }
            SettingRow::SponsorBlock => self.prefs.sponsorblock = !self.prefs.sponsorblock,
            SettingRow::Category(i) => {
                let key = cattube_core::community::CATEGORIES[i].0.to_string();
                if self.prefs.skips(&key) {
                    self.prefs.sb_categories.retain(|c| *c != key);
                } else {
                    self.prefs.sb_categories.push(key);
                }
            }
            // Opens its own screen (the tube handles it).
            SettingRow::LinkPhone => {}
            SettingRow::UpdateYtDlp => {
                self.tools = Scan::Busy;
                let data = self.data.clone();
                self.jobs.spawn(move || Msg::Tools(yt::update_tools(&data)));
                return;
            }
            SettingRow::Account(_) | SettingRow::AddAccount => return,
        }
        self.save_prefs(ctx);
    }

    fn play(&mut self, ctx: &egui::Context, tile: Tile, from_start: bool, queue: Vec<Tile>) {
        if let Some(mut old) = self.player.take() {
            let _ = old.kill();
            let _ = old.wait();
        }
        let (r, g, b) = theme::accent_rgb();
        let start = if from_start {
            0.0
        } else {
            self.resume_at(&tile.id).unwrap_or(0.0)
        };
        let o = PlayOptions {
            video_id: tile.id.clone(),
            title: tile.title.clone(),
            start,
            speed: self.prefs.speed,
            max_height: self.prefs.max_height,
            codec: self.prefs.codec,
            hdr: self.prefs.hdr,
            sub_langs: self.prefs.sub_lang.clone(),
            subs_on: self.prefs.subs_on,
            accent: format!("{r:02x}{g:02x}{b:02x}"),
            buffer_mib: self.prefs.buffer,
        };
        if !player::yt_dlp_path(&self.data).is_file() {
            return self.toast(ctx, "Getting the player ready, try again in a moment");
        }
        match yt::start_player(&o, &self.data) {
            Ok(child) => {
                self.player = Some(child);
                let (tx, rctx) = self.jobs.sender();
                let s = yt::Session {
                    ctx: self.yt_ctx(),
                    refresh: self.refresh(),
                    cache: self.cache.clone(),
                    autoplay: self.prefs.autoplay,
                    sponsorblock: self
                        .prefs
                        .sponsorblock
                        .then(|| self.prefs.sb_categories.clone()),
                    codec: self.prefs.codec,
                    hdr: self.prefs.hdr,
                    history: self.prefs.history,
                    region: self.prefs.region.clone(),
                    queue,
                };
                yt::spawn_watcher(tile, s, move |ended| {
                    let _ = tx.send(Msg::PlaybackDone(ended));
                    rctx.request_repaint();
                });
            }
            Err(e) => self.toast(ctx, &e),
        }
    }

    fn message(&mut self, ctx: &egui::Context, msg: Msg) {
        let c = self.ctx();
        match msg {
            Msg::Page(id, r) => self.tube.on_page(&id, r),
            Msg::More(id, tok, r) => {
                if let Ok(p) = r {
                    self.tube.on_more(&id, &tok, p);
                }
            }
            Msg::Search(r) => self.tube.on_search(r),
            Msg::SearchMore(r) => self.tube.on_search_more(r),
            Msg::Suggestions(q, s) => self.tube.on_suggestions(&q, s),
            Msg::Details(id, r) => self.tube.on_details(&id, r),
            Msg::Votes(id, v) => {
                if let Some(v) = v {
                    self.votes.insert(id, v);
                }
            }
            Msg::SignInCode(r) => self.tube.on_sign_in_code(r),
            Msg::TvCode(r) => self.tube.on_tv_code(r),
            Msg::SignedIn(r) => match r {
                Ok((refresh, access, expires_at)) => {
                    yt::remember_token(&refresh, &access, expires_at);
                    self.accounts.add(account::Account {
                        name: "YouTube account".into(),
                        refresh_token: refresh.clone(),
                        ..account::Account::default()
                    });
                    self.save_accounts(ctx);
                    self.toast(ctx, "Signed in");
                    let yctx = self.yt_ctx();
                    self.jobs.spawn(move || {
                        let n = yt::account_name(&yctx, &refresh);
                        Msg::AccountName(refresh, n)
                    });
                    let c = self.ctx();
                    for e in self.tube.on_signed_in(Ok(()), c) {
                        self.effect(ctx, e);
                    }
                }
                Err(e) => {
                    for e in self.tube.on_signed_in(Err(e), c) {
                        self.effect(ctx, e);
                    }
                }
            },
            Msg::AccountName(refresh, n) => {
                if let Some((name, photo)) = n
                    && let Some(a) = self
                        .accounts
                        .list
                        .iter_mut()
                        .find(|a| a.refresh_token == refresh)
                {
                    a.name = name;
                    a.photo = photo;
                    self.save_accounts(ctx);
                }
            }
            Msg::Done(t) => self.toast(ctx, &t),
            Msg::Quiet => {}
            Msg::Image(url, path) => {
                self.fetching.remove(&url);
                self.pictures.insert(url, path);
            }
            Msg::Tools(r) => {
                self.tools = match r {
                    Ok(v) => Scan::Done(vec![v]),
                    Err(e) => {
                        self.toast(ctx, &e);
                        Scan::Failed(e)
                    }
                };
            }
            Msg::PlaybackDone(ended) => {
                if let Some(mut p) = self.player.take() {
                    let _ = p.wait();
                }
                for (id, pos, dur) in ended.positions {
                    player::remember(&mut self.positions, &id, pos, dur);
                }
                let _ = std::fs::create_dir_all(&self.data);
                let _ = std::fs::write(
                    self.data.join("positions.conf"),
                    player::positions_text(&self.positions),
                );
                // The speed picked in the player is kept for the next video.
                if let Some(sp) = ended.speed
                    && SPEEDS.iter().any(|x| (x - sp).abs() < 1e-6)
                    && (sp - self.prefs.speed).abs() > 1e-6
                {
                    self.prefs.speed = sp;
                    self.save_prefs(ctx);
                }
                if self.quit_after_play {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    return;
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                // Fresh History and progress after watching.
                if matches!(
                    self.tube.section,
                    Section::History | Section::Home | Section::Subscriptions
                ) {
                    for e in self.tube.reload(c) {
                        self.effect(ctx, e);
                    }
                }
            }
        }
    }

    fn fetch_pictures(&mut self) {
        let wanted = std::mem::take(&mut *self.wanted.borrow_mut());
        for url in wanted {
            if self.pictures.contains_key(&url)
                || self.fetching.contains(&url)
                || self.fetching.len() >= 8
            {
                continue;
            }
            self.fetching.insert(url.clone());
            let cache = self.cache.clone();
            self.jobs.spawn(move || {
                let path = yt::fetch_image(&cache, &url);
                Msg::Image(url, path)
            });
        }
    }

    fn loading(&self) -> bool {
        matches!(self.tube.current(), Some(Scan::Busy))
            || matches!(self.tools, Scan::Busy)
            || matches!(
                self.tube.screen(),
                Some(
                    Screen::Page {
                        page: Scan::Busy,
                        ..
                    } | Screen::Search {
                        results: Scan::Busy,
                        ..
                    } | Screen::SignIn { .. }
                        | Screen::Menu {
                            details: Scan::Busy,
                            ..
                        }
                )
            )
    }
}

fn hit_test(hits: &[(egui::Rect, Target)], pos: egui::Pos2) -> Option<Target> {
    hits.iter()
        .rev()
        .find(|(r, _)| r.contains(pos))
        .map(|(_, t)| *t)
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        for msg in self.jobs.drain() {
            self.message(ctx, msg);
        }
        if let Some(Screen::TvCode { code }) = self.tube.screen() {
            let stale = self
                .tv_code_at
                .is_none_or(|t| t.elapsed() > Duration::from_secs(240));
            if stale && !matches!(code, Scan::Busy) {
                self.effect(ctx, Effect::TvCode);
            }
            ctx.request_repaint_after(Duration::from_secs(5));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let focused = keys::window_focused(&ctx);
        if !focused && self.was_focused {
            self.mapper.reset();
        }
        self.was_focused = focused;
        let events = keys::collect(&ctx);
        let actions = self
            .mapper
            .map(&events, keys::now(&ctx), self.tube.text_mode());
        let p = self.pointer.collect(&ctx);
        let c = self.ctx();
        let positions = self.positions.clone();
        let resumable = move |id: &str| positions.iter().any(|(x, _)| x == id);
        let mut effects = Vec::new();
        for action in actions {
            effects.extend(self.tube.handle(action, c, &resumable));
        }
        // The pointer focuses what it is over; a click is OK on it.
        let point = |tube: &mut Tube, t: Option<Target>| match t {
            Some(Target::Tube(r, col)) => ui::tube::focus(tube, r, col),
            Some(t @ (Target::Key(..) | Target::Suggestion(_))) => ui::tube::focus_search(tube, t),
            _ => false,
        };
        if let Some(pos) = p.moved_to {
            point(&mut self.tube, hit_test(&self.hits, pos));
        }
        if let Some(pos) = p.click
            && point(&mut self.tube, hit_test(&self.hits, pos))
        {
            effects.extend(self.tube.handle(Action::Ok, c, &resumable));
        }
        if p.back {
            effects.extend(self.tube.handle(Action::Back, c, &resumable));
        }
        let (n, action) = if p.wheel > 0 {
            (p.wheel, Action::Up)
        } else {
            (-p.wheel, Action::Down)
        };
        for _ in 0..n.min(5) {
            effects.extend(self.tube.handle(action, c, &resumable));
        }
        for e in effects {
            self.effect(&ctx, e);
        }
        if self.mapper.needs_tick() {
            ctx.request_repaint();
        }

        let canvas: Canvas<Target> = Canvas::new(ui, 0);
        let now = keys::now(&ctx);
        canvas.background_at(None, false, now);
        let pics = ui::tube::Pics {
            images: &self.images,
            cache: &self.pictures,
            wanted: &self.wanted,
        };
        let view = ui::tube::View {
            tube: &self.tube,
            accounts: &self.accounts,
            prefs: &self.prefs,
            pics: &pics,
            positions: &self.positions,
            votes: &self.votes,
            tools: &self.tools,
            now,
        };
        ui::tube::draw(&canvas, &view);
        let at = Instant::now();
        if matches!(&self.toast, Some((_, until)) if at >= *until) {
            self.toast = None;
        }
        if let Some((text, until)) = &self.toast {
            let left = until.saturating_duration_since(at).as_secs_f32();
            widgets::toast(&canvas, text, (left / 0.3).clamp(0.0, 1.0));
        }
        self.hits = canvas.take_hits();
        self.fetch_pictures();
        if self.toast.is_some() || self.loading() || !self.fetching.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.sign_in_wait.fetch_add(1, Ordering::SeqCst);
        if let Some(mut p) = self.player.take() {
            let _ = p.kill();
            let _ = p.wait();
        }
    }
}

/// Where CatTube starts.
enum Start {
    Home,
    /// `--section subscriptions`: open on a tab.
    Section(Section),
    /// `--play VIDEO [--title TITLE]`: play one video, close when it ends.
    Play(String, String),
}

/// `--feed`: the newest videos of a list, one per line, for a TV home
/// screen. Exit 2 when not signed in, 1 when YouTube did not answer.
fn feed(which: &str) -> i32 {
    let browse = match which {
        "home" | "recommended" => ytc::feeds::HOME,
        _ => ytc::feeds::SUBSCRIPTIONS,
    };
    let config = cattube_core::env::config_dir();
    let accounts = account::load(&config);
    let Some(a) = accounts.active() else {
        return 2;
    };
    let p = prefs::load(&config);
    let ctx = ytc::context("en", &p.region);
    let page = match yt::page(&ctx, browse, Some(&a.refresh_token)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("cattube: {e}");
            return 1;
        }
    };
    let clean = |s: &str| {
        s.chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect::<String>()
    };
    for t in page.tiles().into_iter().filter(|t| t.playable()).take(24) {
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}",
            t.id,
            clean(&t.title),
            clean(&t.byline),
            clean(&t.meta),
            clean(&t.badge),
            clean(&t.thumb)
        );
    }
    0
}

fn renderer() -> (eframe::Renderer, bool) {
    match std::env::var("CATTUBE_RENDERER").ok().as_deref() {
        Some("glow") => (eframe::Renderer::Glow, true),
        Some("wgpu") => (eframe::Renderer::Wgpu, true),
        _ => (eframe::Renderer::Wgpu, false),
    }
}

const HELP: &str = "cattube {VERSION}: a native Linux YouTube app for TV screens
and remotes. https://github.com/coderastic/CatTube

Usage: cattube [--windowed] [--section NAME | --play VIDEO_ID]
       cattube --feed subscriptions|home

  --section NAME   open on home, subscriptions, history, playlists,
                   channels or search
  --play VIDEO_ID  play one video, then close (--title TITLE names it)
  --remote         keep this TV online for the YouTube phone app: a phone
                   paired with 'Link with TV code' (the code is in
                   Settings, or --tv-code) or by casting picks what plays
  --tv-code        print the code for the phone's 'Link with TV code'
  --pair CODE      pair the phone that cast to this TV (its pairing code)
  --stop           close the player
  --feed LIST      print the newest videos of a list (signed in) as
                   tab-separated lines: id, title, channel, facts, length,
                   picture; no window (for TV home screens)

Keys: arrows move, Enter = OK (hold for the video menu), Esc/Backspace = Back.
Settings and accounts live in ~/.config/cattube/; yt-dlp, Deno and
watch positions in ~/.local/share/cattube/.";

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("cattube {}", cattube_core::env::version());
        return Ok(());
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "{}",
            HELP.replace("{VERSION}", cattube_core::env::version())
        );
        return Ok(());
    }
    if args.iter().any(|a| a == "--remote") {
        std::process::exit(remote::run());
    }
    if args.iter().any(|a| a == "--stop") {
        // Close the player (a cast was ended from the phone).
        let _ = cattube_core::mpv::ipc_request(
            &cattube_core::player::player_socket(),
            &[cattube_core::mpv::Arg::Str("quit")],
        );
        return Ok(());
    }
    if args.iter().any(|a| a == "--tv-code") {
        match remote::tv_code(&cattube_core::env::config_dir()) {
            Ok(c) => println!("{}", cattube_core::lounge::format_code(&c)),
            Err(e) => {
                eprintln!("cattube: {e}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }
    if let Some(i) = args.iter().position(|a| a == "--pair") {
        let code = args.get(i + 1).map(String::as_str).unwrap_or("");
        if let Err(e) = remote::register(&cattube_core::env::config_dir(), code) {
            eprintln!("cattube: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if let Some(i) = args.iter().position(|a| a == "--feed") {
        std::process::exit(feed(
            args.get(i + 1)
                .map(String::as_str)
                .unwrap_or("subscriptions"),
        ));
    }
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let start = if let Some(id) = value("--play").filter(|v| {
        v.len() == 11
            && v.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    }) {
        let title = value("--title").unwrap_or_default();
        Start::Play(id, title)
    } else if let Some(s) = value("--section") {
        Section::ALL
            .into_iter()
            .find(|x| x.label().eq_ignore_ascii_case(&s))
            .map_or(Start::Home, Start::Section)
    } else {
        Start::Home
    };
    let windowed = args.iter().any(|a| a == "--windowed")
        || std::env::var("CATTUBE_WINDOWED").is_ok_and(|v| v == "1");
    let (renderer, explicit) = renderer();
    let viewport = egui::ViewportBuilder::default()
        .with_title("CatTube")
        .with_app_id("io.github.coderastic.CatTube")
        .with_decorations(windowed)
        .with_fullscreen(!windowed)
        .with_inner_size([1280.0, 720.0]);
    let options = eframe::NativeOptions {
        viewport,
        renderer,
        ..Default::default()
    };
    let start = std::cell::Cell::new(Some(start));
    let result = eframe::run_native(
        "CatTube",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, start.take().unwrap_or(Start::Home))))),
    );
    if let Err(e) = &result
        && !explicit
    {
        eprintln!("cattube: wgpu renderer failed ({e}); retrying with OpenGL");
        if let Ok(exe) = std::env::current_exe() {
            use std::os::unix::process::CommandExt;
            let err = std::process::Command::new(exe)
                .args(std::env::args_os().skip(1))
                .env("CATTUBE_RENDERER", "glow")
                .exec();
            eprintln!("cattube: could not restart: {err}");
        }
    }
    result
}
