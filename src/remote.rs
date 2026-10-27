//! `cattube --remote`: the TV side of the YouTube phone remote. It keeps
//! this TV online as a YouTube screen; a phone paired with it ("Link with
//! TV code", or casting) picks what plays, and play / pause / seek / volume
//! / next stay in sync both ways. Runs without a window: it starts the
//! player itself (CatTube's player: control bar, SponsorBlock).
//!
//! The protocol is in [`cattube_core::lounge`]; this file sends it with
//! `curl` (addresses and bodies go in through stdin, so the session's
//! token is never on a command line) and drives mpv over its socket.

use crate::{prefs, theme, yt};
use cattube_core::account;
use cattube_core::lounge::{self, Bind, Command, Lost, Msg, Out, Playing, Queue, Screen, Token};
use cattube_core::mpv::{self, Arg, IpcConn};
use cattube_core::player::{self, PlayOptions};
use cattube_core::youtube as ytc;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn log(text: &str) {
    eprintln!("cattube-remote: {text}");
}

fn casting_allowed(text: &str) -> bool {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .rfind(|(key, _)| key.trim() == "cast_enabled")
        .is_none_or(|(_, value)| value.trim().trim_matches('"') != "false")
}

/// A host shell may share its casting switch with CatTube. Elsewhere the
/// phone remote keeps its usual independent behaviour.
fn casting_enabled() -> bool {
    let path = std::env::var_os("CATTUBE_CAST_ENABLED_FILE")
        .map(PathBuf::from)
        .or_else(|| {
            (std::env::var("CATOSTV_APPLIANCE").as_deref() == Ok("1")).then(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                    .join(".config/catostv/ui.conf")
            })
        });
    path.is_none_or(|path| match std::fs::read_to_string(path) {
        Ok(text) => casting_allowed(&text),
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    })
}

fn random(n: usize) -> String {
    cattube_core::util::random_hex(n).unwrap_or_else(|_| format!("{:x}", now_ms()))
}

// ------------------------------------------------------------ requests

/// A string for curl's config syntax.
fn cfg_quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' | '\r' => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn curl_config(url: &str, body: Option<&str>) -> String {
    let mut c = format!("url = {}\n", cfg_quote(url));
    if let Some(b) = body {
        c.push_str(&format!("data-binary = {}\n", cfg_quote(b)));
    }
    c
}

const STATUS: &str = "CATTUBE-HTTP ";

fn spawn_curl(url: &str, body: Option<&str>, max_secs: u64, stream: bool) -> Result<Child, String> {
    let mut cmd = std::process::Command::new("curl");
    cmd.args([
        "-sS",
        "-m",
        &max_secs.to_string(),
        "-w",
        &format!("\n{STATUS}%{{http_code}}\n"),
        "-K",
        "-",
    ]);
    if stream {
        cmd.arg("-N");
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("curl: {e}"))?;
    if let Some(mut i) = child.stdin.take() {
        let _ = i.write_all(curl_config(url, body).as_bytes());
    }
    Ok(child)
}

/// One request: (HTTP status, body). Status 0: no answer.
fn request(url: &str, body: Option<&str>) -> (u16, String) {
    let Ok(mut child) = spawn_curl(url, body, 30, false) else {
        return (0, String::new());
    };
    let mut out = String::new();
    if let Some(mut o) = child.stdout.take() {
        let _ = o.read_to_string(&mut out);
    }
    let _ = child.wait();
    match out.rfind(STATUS) {
        Some(i) => {
            let status = out[i + STATUS.len()..].trim().parse().unwrap_or(0);
            out.truncate(i);
            (status, out)
        }
        None => (0, out),
    }
}

// ------------------------------------------------------------ the screen

pub fn screen_path(config: &Path) -> PathBuf {
    config.join("remote.conf")
}

/// This TV's name on the phone: the machine's pretty name or host name.
fn tv_name() -> String {
    let pretty = std::fs::read_to_string("/etc/machine-info")
        .ok()
        .and_then(|t| {
            t.lines()
                .find_map(|l| l.strip_prefix("PRETTY_HOSTNAME="))
                .map(|v| v.trim_matches('"').to_string())
        });
    let host = pretty
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_string())
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "TV".into());
    format!("CatTube on {host}").chars().take(60).collect()
}

/// The kept screen, or a new one from YouTube.
pub fn screen(config: &Path) -> Result<Screen, String> {
    // The pairing is the TV's identity to the phone app: only for this user.
    cattube_core::util::make_private(&screen_path(config));
    if let Some(s) = std::fs::read_to_string(screen_path(config))
        .ok()
        .and_then(|t| Screen::parse(&t))
    {
        return Ok(s);
    }
    let (status, body) = request(&lounge::generate_screen_id_url(), None);
    let id = lounge::parse_screen_id(&body)
        .filter(|_| status == 200)
        .ok_or_else(|| yt::friendly(&format!("{status} {body}")))?;
    let device = random(16);
    let s = Screen {
        id,
        device_id: format!(
            "{}-{}-{}-{}-{}",
            &device[0..8],
            &device[8..12],
            &device[12..16],
            &device[16..20],
            &device[20..32]
        ),
        name: tv_name(),
    };
    let _ = std::fs::create_dir_all(config);
    cattube_core::util::write_private(&screen_path(config), &s.to_conf())
        .map_err(|e| e.to_string())?;
    Ok(s)
}

fn token(screen: &Screen) -> Result<Token, String> {
    let (status, body) = request(&lounge::token_url(), Some(&lounge::token_body(screen)));
    lounge::parse_token(&body, now_ms())
        .filter(|_| status == 200)
        .ok_or_else(|| yt::friendly(&format!("{status} {body}")))
}

/// The code to type in the phone's YouTube app (Settings → "Link with TV
/// code"); good for about five minutes.
pub fn tv_code(config: &Path) -> Result<String, String> {
    if !casting_enabled() {
        return Err("Casting is turned off in the TV's settings".into());
    }
    let s = screen(config)?;
    let t = token(&s)?;
    let (status, body) = request(
        &lounge::pairing_code_url(),
        Some(&lounge::pairing_code_body(&s, &t.token)),
    );
    lounge::parse_pairing_code(&body)
        .filter(|_| status == 200)
        .ok_or_else(|| yt::friendly(&format!("{status} {body}")))
}

/// Pair the phone that cast to this TV (DIAL's `pairingCode`).
pub fn register(config: &Path, code: &str) -> Result<(), String> {
    if !casting_enabled() {
        return Err("Casting is turned off in the TV's settings".into());
    }
    let s = screen(config)?;
    let body = lounge::register_code_body(&s, code).ok_or("That pairing code is not valid")?;
    match request(&lounge::register_code_url(), Some(&body)) {
        (200, _) => Ok(()),
        (st, b) => Err(yt::friendly(&format!("{st} {b}"))),
    }
}

/// Is `cattube --remote` running (for this user)?
fn running() -> bool {
    let me = std::process::id();
    let uid = std::fs::metadata("/proc/self")
        .map(|m| std::os::unix::fs::MetadataExt::uid(&m))
        .ok();
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return false;
    };
    dir.flatten().any(|e| {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
            return false;
        };
        if pid == me
            || std::fs::metadata(e.path())
                .map(|m| std::os::unix::fs::MetadataExt::uid(&m))
                .ok()
                != uid
        {
            return false;
        }
        let cmd = std::fs::read(e.path().join("cmdline")).unwrap_or_default();
        let args: Vec<&[u8]> = cmd.split(|b| *b == 0).collect();
        args.first().is_some_and(|a| a.ends_with(b"cattube"))
            && args.iter().any(|a| *a == b"--remote")
    })
}

/// Start the remote service in the background unless it runs already
/// (CatosTV starts it with the session; elsewhere CatTube does, the first
/// time a phone is linked, and it keeps running after CatTube closes).
pub fn ensure_running() {
    use std::os::unix::process::CommandExt;
    if !casting_enabled() || running() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let _ = std::process::Command::new(exe)
        .arg("--remote")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn();
}

// ------------------------------------------------------------ the service

enum Event {
    /// Messages from listen round `round`.
    Msgs(u64, Vec<Msg>),
    /// That request ended (HTTP status, 0 = no answer).
    Closed(u64, u16),
    /// The player of play number `seq` closed.
    Ended(u64, yt::Ended),
}

fn listen(url: String, round: u64, tx: Sender<Event>) -> Option<Child> {
    // Keep the child with the service so disabling casting can close a
    // quiet long-poll connection immediately.
    let mut child = spawn_curl(&url, None, 330, true).ok()?;
    let out = child.stdout.take()?;
    std::thread::spawn(move || {
        let mut status = 0;
        let mut chunks = lounge::Chunks::default();
        for line in BufReader::new(out).lines() {
            let Ok(line) = line else { break };
            if let Some(s) = line.strip_prefix(STATUS) {
                status = s.trim().parse().unwrap_or(0);
                continue;
            }
            let msgs = chunks.push_line(&line);
            if !msgs.is_empty() && tx.send(Event::Msgs(round, msgs)).is_err() {
                break;
            }
        }
        let _ = tx.send(Event::Closed(round, status));
    });
    Some(child)
}

struct Player {
    seq: u64,
    video: String,
    child: Child,
    conn: Option<IpcConn>,
    started: Instant,
    /// The phone's volume was given to this player.
    volume_set: bool,
}

struct Remote {
    config: PathBuf,
    data: PathBuf,
    cache: PathBuf,
    screen: Screen,
    token: Option<Token>,
    bind: Bind,
    round: u64,
    tx: Sender<Event>,
    queue: Queue,
    player: Option<Player>,
    seq: u64,
    cpn: String,
    /// What was last reported, to report changes only.
    reported: Option<Playing>,
    reported_at: Instant,
    volume: (u32, bool),
    listener: Option<Child>,
}

impl Remote {
    fn send(&mut self, out: &[Out]) {
        let zx = random(6);
        let Some((url, body)) = self.bind.send(&self.screen, &zx, out) else {
            return;
        };
        let (status, _) = request(&url, Some(&body));
        if status != 200 {
            log(&format!("report not taken ({status})"));
        }
    }

    /// Open a session (a new token first when needed). Err: wait and retry.
    fn connect(&mut self) -> Result<(), String> {
        if !casting_enabled() {
            return Err("Casting is turned off".into());
        }
        if self
            .token
            .as_ref()
            .is_none_or(|t| now_ms() >= t.refresh_at_ms)
        {
            self.token = Some(token(&self.screen)?);
        }
        let t = self
            .token
            .as_ref()
            .map(|t| t.token.clone())
            .unwrap_or_default();
        self.bind.token = t;
        if !casting_enabled() {
            return Err("Casting is turned off".into());
        }
        let url = self.bind.init_url(&self.screen, &random(6));
        let (status, body) = request(&url, Some(lounge::INIT_BODY));
        if status != 200 {
            if lounge::lost(status) == Lost::Token {
                self.token = None;
            }
            return Err(format!("session not opened ({status})"));
        }
        let msgs = lounge::parse_reply(&body);
        self.bind.update(&msgs);
        if !self.bind.ready() {
            return Err("session not opened (no session id)".into());
        }
        self.round += 1;
        log(&format!("online as {}", lounge::quote(&self.screen.name)));
        self.handle(&msgs);
        self.relisten();
        Ok(())
    }

    fn relisten(&mut self) {
        self.stop_listening();
        if !casting_enabled() {
            return;
        }
        if let Some(url) = self.bind.listen_url(&self.screen, &random(6)) {
            self.listener = listen(url, self.round, self.tx.clone());
            if self.listener.is_none() {
                let _ = self.tx.send(Event::Closed(self.round, 0));
            }
        }
    }

    fn stop_listening(&mut self) {
        if let Some(mut child) = self.listener.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn disable_casting(&mut self) {
        self.stop_listening();
        self.round += 1;
        self.bind = Bind::new("", 0);
        self.token = None;
        self.queue = Queue::default();
        self.close_player();
        self.reported = None;
    }

    fn handle(&mut self, msgs: &[Msg]) {
        self.bind.update(msgs);
        for m in msgs {
            if let Some(c) = lounge::command(m) {
                self.command(c);
            }
        }
    }

    fn conn(&mut self) -> Option<&mut IpcConn> {
        let p = self.player.as_mut()?;
        if p.conn.as_ref().is_none_or(|c| c.closed()) {
            p.conn = IpcConn::connect(&player::player_socket()).ok();
        }
        p.conn.as_mut()
    }

    fn mpv(&mut self, args: &[Arg]) {
        if let Some(c) = self.conn() {
            let _ = c.command(args);
        }
    }

    fn command(&mut self, c: Command) {
        if !casting_enabled() {
            return;
        }
        match c {
            Command::SetPlaylist { queue, start } => {
                self.queue = queue;
                self.play(start);
            }
            Command::UpdatePlaylist { queue } => {
                let current = self.player.as_ref().map(|p| p.video.clone());
                self.queue = queue;
                if let Some(i) = current.and_then(|v| self.queue.ids.iter().position(|x| *x == v)) {
                    self.queue.index = i;
                }
                let r = self.reported.clone();
                self.send(&[
                    lounge::previous_next(&self.queue),
                    lounge::now_playing(r.as_ref(), &self.queue),
                ]);
            }
            Command::Play => self.mpv(&[Arg::Str("set"), Arg::Str("pause"), Arg::Str("no")]),
            Command::Pause => self.mpv(&[Arg::Str("set"), Arg::Str("pause"), Arg::Str("yes")]),
            Command::Seek(t) => self.mpv(&[Arg::Str("seek"), Arg::Num(t), Arg::Str("absolute")]),
            Command::Stop => self.stop(),
            Command::SetVolume { volume, muted } => {
                self.volume = (volume, muted);
                self.mpv(&[
                    Arg::Str("set"),
                    Arg::Str("volume"),
                    Arg::Str(&volume.to_string()),
                ]);
                self.mpv(&[
                    Arg::Str("set"),
                    Arg::Str("mute"),
                    Arg::Str(if muted { "yes" } else { "no" }),
                ]);
                self.send(&[lounge::volume_changed(volume, muted)]);
            }
            Command::Next if self.queue.has_next() => {
                self.queue.index += 1;
                self.play(0.0);
            }
            Command::Previous if self.queue.has_previous() => {
                self.queue.index -= 1;
                self.play(0.0);
            }
            Command::Next | Command::Previous => {}
            Command::NowPlaying => {
                let r = self.reported.clone();
                self.send(&[lounge::now_playing(r.as_ref(), &self.queue)]);
            }
            Command::Volume => {
                let (v, m) = self.volume;
                self.send(&[lounge::volume_changed(v, m)]);
            }
            Command::Connected(name) => {
                log(&format!("{} connected", lounge::quote(&name)));
                let r = self.reported.clone();
                let (v, m) = self.volume;
                self.send(&[
                    lounge::previous_next(&self.queue),
                    lounge::autoplay_off(),
                    lounge::now_playing(r.as_ref(), &self.queue),
                    lounge::volume_changed(v, m),
                ]);
                let text = format!("{name} is connected");
                self.mpv(&[Arg::Str("show-text"), Arg::Str(&text), Arg::Num(2500.0)]);
            }
            Command::Disconnected(name) => log(&format!("{} left", lounge::quote(&name))),
        }
    }

    fn close_player(&mut self) {
        if let Some(mut p) = self.player.take() {
            yt::checkpoint_player(&player::data_home());
            if let Some(c) = p.conn.as_mut() {
                let _ = c.command(&[Arg::Str("quit")]);
            }
            std::thread::sleep(Duration::from_millis(300));
            let _ = p.child.kill();
            let _ = p.child.wait();
        }
    }

    fn stop(&mut self) {
        self.close_player();
        self.reported = None;
        let q = self.queue.clone();
        self.send(&[lounge::now_playing(None, &q)]);
    }

    /// Play the queue's current video from `start` (on the TV screen,
    /// over whatever is there; CatTube's own player closes first).
    fn play(&mut self, start: f64) {
        if !casting_enabled() {
            return;
        }
        let Some(id) = self.queue.current().map(str::to_string) else {
            return self.stop();
        };
        self.close_player();
        // A player the app started is told to close too (one screen).
        let _ = mpv::ipc_request(&player::player_socket(), &[Arg::Str("quit")]);
        let p = prefs::load(&self.config);
        let (r, g, b) = theme::accent_rgb_of(&p.accent);
        let o = PlayOptions {
            video_id: id.clone(),
            title: String::new(),
            start,
            speed: p.speed,
            max_height: p.max_height,
            codec: p.codec,
            hdr: p.hdr,
            sub_langs: p.sub_lang.clone(),
            subs_on: p.subs_on,
            accent: format!("{r:02x}{g:02x}{b:02x}"),
            buffer_mib: p.buffer,
            // Played from the phone: nothing of CatTube's to go back to.
            hide_on_back: false,
        };
        if let Err(e) = yt::ensure_tools(&self.data) {
            log(&format!("player not ready: {e}"));
            return;
        }
        if !casting_enabled() {
            return;
        }
        let child = match yt::start_player(&o, &self.data) {
            Ok(c) => c,
            Err(e) => return log(&e),
        };
        self.seq += 1;
        let seq = self.seq;
        let accounts = account::load(&self.config);
        let s = yt::Session {
            ctx: ytc::context("en", &p.region),
            refresh: accounts.active().map(|a| a.refresh_token.clone()),
            cache: self.cache.clone(),
            // The phone's queue decides what comes next.
            autoplay: prefs::Autoplay::Off,
            sponsorblock: p.sponsorblock.then(|| p.sb_categories.clone()),
            codec: p.codec,
            hdr: p.hdr,
            region: p.region.clone(),
            queue: Vec::new(),
            on_hide: None,
            on_video: None,
        };
        let tile = ytc::Tile {
            id: id.clone(),
            title: String::new(),
            ..ytc::Tile::default()
        };
        let tx = self.tx.clone();
        yt::spawn_watcher(tile, s, move |ended| {
            let _ = tx.send(Event::Ended(seq, ended));
        });
        self.player = Some(Player {
            seq,
            video: id.clone(),
            child,
            conn: None,
            started: Instant::now(),
            volume_set: false,
        });
        self.cpn = lounge::cpn(random(16).as_bytes());
        let loading = Playing {
            video_id: id,
            time: start,
            duration: 0.0,
            state: lounge::LOADING,
            cpn: self.cpn.clone(),
        };
        self.reported = Some(loading.clone());
        self.reported_at = Instant::now();
        let q = self.queue.clone();
        self.send(&[
            lounge::now_playing(Some(&loading), &q),
            lounge::previous_next(&q),
        ]);
    }

    /// The player closed: keep its place, play the next in the queue when
    /// the video ran to its end.
    fn ended(&mut self, seq: u64, ended: yt::Ended) {
        let data = &self.data;
        let mut positions = std::fs::read_to_string(data.join("positions.conf"))
            .map(|t| player::parse_positions(&t))
            .unwrap_or_default();
        let mut finished = false;
        for (id, pos, dur) in &ended.positions {
            player::remember(&mut positions, id, *pos, *dur);
            finished = dur.is_some_and(|d| *pos >= d - 5.0);
        }
        let _ = std::fs::write(
            data.join("positions.conf"),
            player::positions_text(&positions),
        );
        if self.player.as_ref().is_none_or(|p| p.seq != seq) {
            return; // an older player that was replaced
        }
        if let Some(mut p) = self.player.take() {
            yt::checkpoint_player(&player::data_home());
            let _ = p.child.wait();
        }
        if finished && self.queue.has_next() {
            self.queue.index += 1;
            return self.play(0.0);
        }
        let q = self.queue.clone();
        match (finished, self.reported.clone()) {
            (true, Some(mut r)) => {
                r.state = lounge::ENDED;
                r.time = r.duration;
                self.send(&[lounge::state_change(&r)]);
                self.reported = None;
            }
            _ => {
                self.reported = None;
                self.send(&[lounge::now_playing(None, &q)]);
            }
        }
    }

    /// Report the player's state when it changed (or every 15 s).
    fn poll(&mut self) {
        if self.player.is_none() {
            return;
        }
        let (v, m) = self.volume;
        let set_volume = self.player.as_ref().is_some_and(|p| !p.volume_set);
        let Some(c) = self.conn() else { return };
        if set_volume {
            // A new player starts at full volume: keep the phone's.
            let _ = c.command(&[
                Arg::Str("set"),
                Arg::Str("volume"),
                Arg::Str(&v.to_string()),
            ]);
            let _ = c.command(&[
                Arg::Str("set"),
                Arg::Str("mute"),
                Arg::Str(if m { "yes" } else { "no" }),
            ]);
            if let Some(p) = self.player.as_mut() {
                p.volume_set = true;
            }
            return;
        }
        let time = c.get_f64("time-pos");
        let duration = c.get_f64("duration").unwrap_or(0.0);
        let paused = c.get_bool("pause").unwrap_or(false);
        let buffering = c.get_bool("paused-for-cache").unwrap_or(false);
        let volume = c
            .get_f64("volume")
            .map(|v| v.round().clamp(0.0, 100.0) as u32);
        let muted = c.get_bool("mute").unwrap_or(false);
        let Some(p) = self.player.as_ref() else {
            return;
        };
        if time.is_none() && p.started.elapsed() < Duration::from_secs(60) {
            return; // still loading
        }
        let state = match (time, paused, buffering) {
            (None, ..) | (_, false, true) => lounge::LOADING,
            (_, true, _) => lounge::PAUSED,
            _ => lounge::PLAYING,
        };
        let now = Playing {
            video_id: p.video.clone(),
            time: time.unwrap_or(0.0),
            duration,
            state,
            cpn: self.cpn.clone(),
        };
        let elapsed = self.reported_at.elapsed().as_secs_f64();
        let changed = match &self.reported {
            None => true,
            Some(r) => {
                let expected = if r.state == lounge::PLAYING {
                    r.time + elapsed
                } else {
                    r.time
                };
                r.state != now.state
                    || r.video_id != now.video_id
                    || (r.duration - now.duration).abs() > 1.0
                    || (expected - now.time).abs() > 2.5
                    || elapsed > 15.0
            }
        };
        let first = self
            .reported
            .as_ref()
            .is_none_or(|r| r.state == lounge::LOADING)
            && state != lounge::LOADING;
        if changed {
            let q = self.queue.clone();
            let out = if first {
                lounge::now_playing(Some(&now), &q)
            } else {
                lounge::state_change(&now)
            };
            self.reported = Some(now);
            self.reported_at = Instant::now();
            self.send(&[out]);
        }
        if let Some(v) = volume
            && (v, muted) != self.volume
        {
            self.volume = (v, muted);
            self.send(&[lounge::volume_changed(v, muted)]);
        }
    }
}

/// `cattube --remote`: stay online for paired phones until stopped.
pub fn run() -> i32 {
    let config = cattube_core::env::config_dir();
    let data = player::data_home();
    let cache = cattube_core::env::cache_dir();
    let (tx, rx): (Sender<Event>, Receiver<Event>) = mpsc::channel();
    let mut wait = Duration::from_secs(5);
    let screen = loop {
        if !casting_enabled() {
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }
        match screen(&config) {
            Ok(s) => break s,
            Err(e) => {
                log(&e);
                std::thread::sleep(wait);
                wait = (wait * 2).min(Duration::from_secs(300));
            }
        }
    };
    let seed = u32::from_str_radix(&random(2), 16).unwrap_or(0);
    let mut r = Remote {
        config,
        data,
        cache,
        bind: Bind::new("", seed),
        screen,
        token: None,
        round: 0,
        tx,
        queue: Queue::default(),
        player: None,
        seq: 0,
        cpn: String::new(),
        reported: None,
        reported_at: Instant::now(),
        volume: (100, false),
        listener: None,
    };
    let mut retry_at: Option<Instant> = Some(Instant::now());
    let mut wait = Duration::from_secs(5);
    let mut enabled = true;
    loop {
        if !casting_enabled() {
            if enabled {
                r.disable_casting();
                retry_at = Some(Instant::now());
                wait = Duration::from_secs(5);
                enabled = false;
            }
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }
        enabled = true;
        if let Some(at) = retry_at
            && Instant::now() >= at
        {
            match r.connect() {
                Ok(()) => {
                    retry_at = None;
                    wait = Duration::from_secs(5);
                }
                Err(e) => {
                    log(&e);
                    retry_at = Some(Instant::now() + wait);
                    wait = (wait * 2).min(Duration::from_secs(300));
                }
            }
        }
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Event::Msgs(round, msgs)) if round == r.round => r.handle(&msgs),
            Ok(Event::Closed(round, status)) if round == r.round => {
                match (status, lounge::lost(status)) {
                    // A listen YouTube ended on its own: listen again.
                    (200, _) => r.relisten(),
                    (_, Lost::Token) => {
                        r.token = None;
                        retry_at = Some(Instant::now());
                    }
                    (_, Lost::Session) => retry_at = Some(Instant::now()),
                    (_, Lost::Retry) => retry_at = Some(Instant::now() + Duration::from_secs(5)),
                }
            }
            Ok(Event::Ended(seq, ended)) => r.ended(seq, ended),
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return 1,
        }
        r.poll();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_casting_switch_ignores_other_settings_and_keeps_the_last_choice() {
        assert!(casting_allowed(""));
        assert!(casting_allowed("cast_enabled = true\n"));
        assert!(casting_allowed("# cast_enabled = false\n"));
        assert!(casting_allowed("cast_enabled = junk\n"));
        assert!(!casting_allowed("theme = flat\ncast_enabled = false\n"));
        assert!(!casting_allowed("cast_enabled = \"false\"\n"));
        assert!(casting_allowed(
            "cast_enabled = false\ncast_enabled = true\n"
        ));
    }

    #[test]
    fn curl_config_quotes() {
        assert_eq!(
            curl_config("https://x/?a=\"b\"", Some("c\\d\ne")),
            "url = \"https://x/?a=\\\"b\\\"\"\ndata-binary = \"c\\\\de\"\n"
        );
    }
}
