//! The phone remote: YouTube's "Lounge" protocol, screen side, the way
//! YouTube's own TV app speaks it. The YouTube app on a phone pairs with
//! the TV (Settings → "Link with TV code", or casting), then sends what to
//! play and play / pause / seek / volume; the TV reports back where it is,
//! so the phone's controls stay in step.
//!
//! Pure: request bodies, addresses and the parsing of replies; the app
//! sends them with `curl`. Facts (checked against YouTube's servers):
//!
//! | Step | Request (`https://www.youtube.com/api/lounge/…`) |
//! |---|---|
//! | A screen id (once, kept) | `GET pairing/generate_screen_id` → plain text |
//! | Its token | `POST pairing/get_lounge_token_batch` `screen_ids=` → `screens[0].loungeToken` |
//! | "Link with TV code" | `POST pairing/get_pairing_code?ctx=pair` → 12 digits (about 5 min) |
//! | A cast's pairing code | `POST pairing/register_pairing_code` |
//! | Open a session | `POST bc/bind?…RID=n&CVER=1` `count=0` → `c` (SID), `S` (gsessionid) |
//! | Listen | `GET bc/bind?…RID=rpc&SID&gsessionid&AID&CI=0&TYPE=xmlhttp` (stays open) |
//! | Report | `POST bc/bind?…SID&gsessionid&RID&AID` `count, ofs, req0__sc, req0_<field>` |
//!
//! Replies come in chunks: a line with a length, then a JSON array of
//! `[id, [name, payload?]]` messages spread over lines. Ids only grow; the
//! last one seen is sent back as `AID`.

use crate::mpv::{Json, json_string, parse_json};
use crate::util::percent_encode;

pub const BASE: &str = "https://www.youtube.com/api/lounge";
/// The app name the screen gives (shows in the phone's device list data).
pub const APP: &str = "cattube";

/// This TV as a Lounge screen; `id` and `device_id` are kept across runs
/// so a paired phone finds it again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    pub id: String,
    pub device_id: String,
    pub name: String,
}

impl Screen {
    pub fn parse(text: &str) -> Option<Screen> {
        let get = |key: &str| {
            text.lines().find_map(|l| {
                let (k, v) = l.split_once('=')?;
                (k.trim() == key).then(|| v.trim().to_string())
            })
        };
        let s = Screen {
            id: get("screen_id")?,
            device_id: get("device_id")?,
            name: get("name").unwrap_or_default(),
        };
        (is_token(&s.id) && is_token(&s.device_id)).then_some(s)
    }

    pub fn to_conf(&self) -> String {
        format!(
            "# CatTube phone remote: this TV's screen (kept so paired phones find it)\nscreen_id = {}\ndevice_id = {}\nname = {}\n",
            self.id, self.device_id, self.name
        )
    }
}

/// Ids, tokens and codes YouTube hands out: letters, digits and `-_.`.
fn is_token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 512
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

fn form(fields: &[(&str, &str)]) -> String {
    fields
        .iter()
        .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

pub fn generate_screen_id_url() -> String {
    format!("{BASE}/pairing/generate_screen_id")
}

/// `generate_screen_id`'s answer.
pub fn parse_screen_id(text: &str) -> Option<String> {
    let id = text.trim();
    is_token(id).then(|| id.to_string())
}

pub fn token_url() -> String {
    format!("{BASE}/pairing/get_lounge_token_batch")
}

pub fn token_body(screen: &Screen) -> String {
    form(&[("screen_ids", &screen.id)])
}

/// A lounge token and when to fetch a new one (ms since 1970).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub token: String,
    pub refresh_at_ms: u64,
}

pub fn parse_token(text: &str, now_ms: u64) -> Option<Token> {
    let j = parse_json(text)?;
    let s = j.get("screens")?.as_arr()?.first()?;
    let token = s.get("loungeToken")?.as_str()?.to_string();
    if !is_token(&token) {
        return None;
    }
    let every = s
        .get("refreshIntervalInMillis")
        .or_else(|| s.get("refreshIntervalMs"))
        .and_then(num)
        .unwrap_or(86_400_000.0);
    Some(Token {
        token,
        refresh_at_ms: now_ms + every.clamp(60_000.0, 14.0 * 86_400_000.0) as u64,
    })
}

/// Numbers come as numbers or as strings.
fn num(j: &Json) -> Option<f64> {
    j.as_f64().or_else(|| j.as_str()?.trim().parse().ok())
}

pub fn pairing_code_url() -> String {
    format!("{BASE}/pairing/get_pairing_code?ctx=pair")
}

pub fn pairing_code_body(screen: &Screen, token: &str) -> String {
    form(&[
        ("access_type", "permanent"),
        ("app", APP),
        ("lounge_token", token),
        ("screen_id", &screen.id),
        ("screen_name", &screen.name),
        ("device_id", &screen.device_id),
    ])
}

/// The 12-digit code for the phone's "Link with TV code".
pub fn parse_pairing_code(text: &str) -> Option<String> {
    let c = text.trim();
    (c.len() == 12 && c.bytes().all(|b| b.is_ascii_digit())).then(|| c.to_string())
}

/// "123456789012" → "123 456 789 012", the way TVs show it.
pub fn format_code(code: &str) -> String {
    code.as_bytes()
        .chunks(3)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn register_code_url() -> String {
    format!("{BASE}/pairing/register_pairing_code")
}

/// A cast (DIAL) brings a pairing code; registering it pairs that phone.
pub fn register_code_body(screen: &Screen, code: &str) -> Option<String> {
    is_token(code).then(|| {
        form(&[
            ("access_type", "permanent"),
            ("app", APP),
            ("pairing_code", code),
            ("screen_id", &screen.id),
            ("screen_name", &screen.name),
            ("device_id", &screen.device_id),
        ])
    })
}

// ---------------------------------------------------------------------------
// The session ("bind")
// ---------------------------------------------------------------------------

/// One message from the phone side (or the server).
#[derive(Debug, Clone, PartialEq)]
pub struct Msg {
    pub aid: i64,
    pub name: String,
    /// The first argument (an object of strings), or `Null`.
    pub payload: Json,
}

impl Msg {
    pub fn field(&self, key: &str) -> Option<&str> {
        self.payload.get(key)?.as_str()
    }
    fn num(&self, key: &str) -> Option<f64> {
        self.payload.get(key).and_then(num)
    }
}

fn parse_array(text: &str) -> Vec<Msg> {
    let Some(Json::Arr(items)) = parse_json(text) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|it| {
            let a = it.as_arr()?;
            let aid = a.first()?.as_f64()? as i64;
            let body = a.get(1)?.as_arr()?;
            Some(Msg {
                aid,
                name: body.first()?.as_str()?.to_string(),
                payload: body.get(1).cloned().unwrap_or(Json::Null),
            })
        })
        .collect()
}

/// Reads the chunked stream line by line (the listen request never ends).
#[derive(Debug, Default)]
pub struct Chunks {
    buf: String,
}

impl Chunks {
    pub fn push_line(&mut self, line: &str) -> Vec<Msg> {
        let line = line.trim_end_matches(['\r', '\n']);
        if self.buf.is_empty() {
            // A length line starts a chunk; the length itself is not
            // needed: the chunk ends where its JSON does.
            if line.trim().is_empty() || line.trim().bytes().all(|b| b.is_ascii_digit()) {
                return Vec::new();
            }
        }
        self.buf.push_str(line);
        self.buf.push('\n');
        if !self.buf.trim_end().ends_with(']') {
            return Vec::new();
        }
        if parse_json(&self.buf).is_some() {
            let out = parse_array(&self.buf);
            self.buf.clear();
            return out;
        }
        if self.buf.len() > 1 << 20 {
            self.buf.clear();
        }
        Vec::new()
    }
}

/// All messages in a whole reply.
pub fn parse_reply(text: &str) -> Vec<Msg> {
    let mut c = Chunks::default();
    text.lines().flat_map(|l| c.push_line(l)).collect()
}

/// Session state for the `bind` requests.
#[derive(Debug, Clone)]
pub struct Bind {
    pub token: String,
    pub sid: Option<String>,
    pub gsession: Option<String>,
    rid: u32,
    pub aid: i64,
    ofs: u64,
}

const DEVICE_INFO: &str = "{\"brand\":\"CatTube\",\"model\":\"Linux TV\",\"year\":0,\"os\":\"Linux\",\"osVersion\":\"\",\"chipset\":\"\",\"clientName\":\"TVHTML5\",\"dialAdditionalDataSupportLevel\":\"unsupported\",\"mdxDialServerType\":\"MDX_DIAL_SERVER_TYPE_UNKNOWN\"}";

impl Bind {
    /// `seed` picks the first request number (YouTube's screens start
    /// somewhere in 41000..50000).
    pub fn new(token: &str, seed: u32) -> Bind {
        Bind {
            token: token.to_string(),
            sid: None,
            gsession: None,
            rid: 41_000 + seed % 9_000,
            aid: 3,
            ofs: 0,
        }
    }

    pub fn ready(&self) -> bool {
        self.sid.is_some() && self.gsession.is_some()
    }

    fn common(&self, screen: &Screen, zx: &str) -> Vec<(&'static str, String)> {
        vec![
            ("device", "LOUNGE_SCREEN".into()),
            ("id", screen.device_id.clone()),
            ("obfuscatedGaiaId", String::new()),
            ("name", screen.name.clone()),
            ("app", APP.into()),
            ("theme", "cl".into()),
            ("capabilities", "dsp,mic,dpa,ntb".into()),
            ("cst", "m".into()),
            ("mdxVersion", "2".into()),
            ("loungeIdToken", self.token.clone()),
            ("VER", "8".into()),
            ("v", "2".into()),
            ("zx", zx.into()),
            ("t", "1".into()),
        ]
    }

    fn url(params: Vec<(&'static str, String)>) -> String {
        let q: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
        format!("{BASE}/bc/bind?{}", form(&q))
    }

    /// Opening a session (POST, body [`INIT_BODY`]).
    pub fn init_url(&mut self, screen: &Screen, zx: &str) -> String {
        self.sid = None;
        self.gsession = None;
        self.aid = 3;
        self.ofs = 0;
        let mut p = self.common(screen, zx);
        p.push(("deviceInfo", DEVICE_INFO.into()));
        p.push(("RID", self.rid.to_string()));
        p.push(("CVER", "1".into()));
        self.rid += 1;
        Self::url(p)
    }

    /// Listening (GET, stays open while messages stream in).
    pub fn listen_url(&self, screen: &Screen, zx: &str) -> Option<String> {
        let mut p = self.common(screen, zx);
        p.push(("RID", "rpc".into()));
        p.push(("SID", self.sid.clone()?));
        p.push(("CI", "0".into()));
        p.push(("AID", self.aid.to_string()));
        p.push(("gsessionid", self.gsession.clone()?));
        p.push(("TYPE", "xmlhttp".into()));
        Some(Self::url(p))
    }

    /// Reporting `out` (POST): the address and the body.
    pub fn send(&mut self, screen: &Screen, zx: &str, out: &[Out]) -> Option<(String, String)> {
        let mut p = self.common(screen, zx);
        p.push(("deviceInfo", DEVICE_INFO.into()));
        p.push(("SID", self.sid.clone()?));
        p.push(("RID", self.rid.to_string()));
        p.push(("AID", self.aid.to_string()));
        p.push(("gsessionid", self.gsession.clone()?));
        self.rid += 1;
        let mut fields = vec![
            ("count".to_string(), out.len().to_string()),
            ("ofs".to_string(), self.ofs.to_string()),
        ];
        for (i, o) in out.iter().enumerate() {
            fields.push((format!("req{i}__sc"), o.name.to_string()));
            for (k, v) in &o.fields {
                fields.push((format!("req{i}_{k}"), v.clone()));
            }
        }
        self.ofs += out.len() as u64;
        let f: Vec<(&str, &str)> = fields
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        Some((Self::url(p), form(&f)))
    }

    /// Note the session ids and the last message id.
    pub fn update(&mut self, msgs: &[Msg]) {
        for m in msgs {
            match m.name.as_str() {
                "c" => {
                    self.sid = m
                        .payload
                        .as_str()
                        .filter(|s| is_token(s))
                        .map(str::to_string)
                }
                "S" => {
                    self.gsession = m
                        .payload
                        .as_str()
                        .filter(|s| is_token(s))
                        .map(str::to_string)
                }
                _ => {}
            }
            self.aid = self.aid.max(m.aid);
        }
    }
}

pub const INIT_BODY: &str = "count=0";

/// What a failed bind means (HTTP status).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lost {
    /// The session is gone (400 "Unknown SID", 404, 410): open a new one.
    Session,
    /// The token expired (401): fetch a new one, then a new session.
    Token,
    /// Network trouble or anything else: wait, then try again.
    Retry,
}

pub fn lost(status: u16) -> Lost {
    match status {
        400 | 404 | 410 => Lost::Session,
        401 | 403 => Lost::Token,
        _ => Lost::Retry,
    }
}

// ---------------------------------------------------------------------------
// What the phone asks for
// ---------------------------------------------------------------------------

fn is_video_id(s: &str) -> bool {
    s.len() == 11
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The phone's queue: the videos in order and the one playing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Queue {
    pub list_id: String,
    pub ids: Vec<String>,
    pub index: usize,
}

impl Queue {
    pub fn current(&self) -> Option<&str> {
        self.ids.get(self.index).map(String::as_str)
    }
    pub fn has_next(&self) -> bool {
        self.index + 1 < self.ids.len()
    }
    pub fn has_previous(&self) -> bool {
        self.index > 0 && !self.ids.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Play `queue.current()` from `start` seconds.
    SetPlaylist {
        queue: Queue,
        start: f64,
    },
    /// The queue changed (videos added or removed), keep playing.
    UpdatePlaylist {
        queue: Queue,
    },
    Play,
    Pause,
    Stop,
    Seek(f64),
    SetVolume {
        volume: u32,
        muted: bool,
    },
    Next,
    Previous,
    /// Report what is playing / the volume.
    NowPlaying,
    Volume,
    /// A phone connected or left (its name).
    Connected(String),
    Disconnected(String),
}

fn queue_of(m: &Msg) -> Queue {
    let ids: Vec<String> = m
        .field("videoIds")
        .unwrap_or("")
        .split(',')
        .filter(|s| is_video_id(s))
        .map(str::to_string)
        .collect();
    let index = m
        .num("currentIndex")
        .map(|i| i.max(0.0) as usize)
        .unwrap_or(0);
    let mut q = Queue {
        list_id: m
            .field("listId")
            .filter(|s| is_token(s))
            .unwrap_or("")
            .to_string(),
        ids,
        index,
    };
    // The video asked for wins over a stale index.
    if let Some(v) = m.field("videoId").filter(|s| is_video_id(s)) {
        match q.ids.iter().position(|x| x == v) {
            Some(i) if q.current() != Some(v) => q.index = i,
            Some(_) => {}
            None => {
                q.ids = vec![v.to_string()];
                q.index = 0;
            }
        }
    }
    if q.index >= q.ids.len() {
        q.index = 0;
    }
    q
}

fn device_name(m: &Msg) -> String {
    m.field("name")
        .unwrap_or("A phone")
        .chars()
        .take(60)
        .collect()
}

pub fn command(m: &Msg) -> Option<Command> {
    Some(match m.name.as_str() {
        "setPlaylist" => {
            let queue = queue_of(m);
            queue.current()?;
            Command::SetPlaylist {
                queue,
                start: m.num("currentTime").unwrap_or(0.0).max(0.0),
            }
        }
        "updatePlaylist" => Command::UpdatePlaylist { queue: queue_of(m) },
        "play" => Command::Play,
        "pause" => Command::Pause,
        "stopVideo" => Command::Stop,
        "seekTo" => Command::Seek(m.num("newTime")?.max(0.0)),
        "setVolume" => Command::SetVolume {
            volume: m.num("volume")?.clamp(0.0, 100.0) as u32,
            muted: m.field("muted") == Some("true")
                || m.payload.get("muted").and_then(Json::as_bool) == Some(true),
        },
        "next" => Command::Next,
        "previous" => Command::Previous,
        "getNowPlaying" => Command::NowPlaying,
        "getVolume" => Command::Volume,
        "remoteConnected" => Command::Connected(device_name(m)),
        "remoteDisconnected" => Command::Disconnected(device_name(m)),
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// What the TV reports
// ---------------------------------------------------------------------------

/// Player states the phone knows.
pub const IDLE: i32 = -1;
pub const ENDED: i32 = 0;
pub const PLAYING: i32 = 1;
pub const PAUSED: i32 = 2;
pub const LOADING: i32 = 3;

/// One report.
#[derive(Debug, Clone, PartialEq)]
pub struct Out {
    pub name: &'static str,
    pub fields: Vec<(String, String)>,
}

/// Where playback is.
#[derive(Debug, Clone, PartialEq)]
pub struct Playing {
    pub video_id: String,
    pub time: f64,
    pub duration: f64,
    pub state: i32,
    pub cpn: String,
}

fn secs(x: f64) -> String {
    format!("{:.3}", x.max(0.0))
}

fn timing(p: &Playing) -> Vec<(String, String)> {
    let loaded = if matches!(p.state, PLAYING | PAUSED | LOADING) {
        p.duration
    } else {
        0.0
    };
    vec![
        ("currentTime".into(), secs(p.time)),
        ("duration".into(), secs(p.duration)),
        ("cpn".into(), p.cpn.clone()),
        ("loadedTime".into(), secs(loaded)),
        ("seekableStartTime".into(), "0".into()),
        ("seekableEndTime".into(), secs(p.duration)),
        ("state".into(), p.state.to_string()),
    ]
}

/// `nowPlaying`: the video and where it is (no video: nothing playing).
pub fn now_playing(p: Option<&Playing>, q: &Queue) -> Out {
    let mut fields = Vec::new();
    if let Some(p) = p {
        fields.push(("videoId".into(), p.video_id.clone()));
        fields.extend(timing(p));
        if !q.list_id.is_empty() {
            fields.push(("listId".into(), q.list_id.clone()));
        }
        fields.push(("currentIndex".into(), q.index.to_string()));
    }
    Out {
        name: "nowPlaying",
        fields,
    }
}

pub fn state_change(p: &Playing) -> Out {
    Out {
        name: "onStateChange",
        fields: timing(p),
    }
}

pub fn volume_changed(volume: u32, muted: bool) -> Out {
    Out {
        name: "onVolumeChanged",
        fields: vec![
            ("volume".into(), volume.min(100).to_string()),
            ("muted".into(), muted.to_string()),
        ],
    }
}

pub fn previous_next(q: &Queue) -> Out {
    Out {
        name: "onHasPreviousNextChanged",
        fields: vec![
            ("hasPrevious".into(), q.has_previous().to_string()),
            ("hasNext".into(), q.has_next().to_string()),
        ],
    }
}

/// The phone shows CatTube's own "up next" off: it follows its queue.
pub fn autoplay_off() -> Out {
    Out {
        name: "onAutoplayModeChanged",
        fields: vec![("autoplayMode".into(), "UNSUPPORTED".into())],
    }
}

/// A random "client playback nonce" for a new video (16 URL-safe chars).
pub fn cpn(random: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    random
        .iter()
        .take(16)
        .map(|b| A[(*b & 63) as usize] as char)
        .collect()
}

/// A JSON string, for logs.
pub fn quote(s: &str) -> String {
    json_string(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Screen {
        Screen {
            id: "abc123".into(),
            device_id: "da211247-63f0-4e52-8c34-95370ba7507a".into(),
            name: "Living room TV".into(),
        }
    }

    #[test]
    fn screen_conf_round_trip() {
        let s = screen();
        assert_eq!(Screen::parse(&s.to_conf()), Some(s));
        assert_eq!(Screen::parse("screen_id = a b\ndevice_id = x\n"), None);
    }

    #[test]
    fn pairing() {
        assert_eq!(
            parse_pairing_code("961234567890\n").as_deref(),
            Some("961234567890")
        );
        assert_eq!(parse_pairing_code("<html>"), None);
        assert_eq!(format_code("961234567890"), "961 234 567 890");
        let b = pairing_code_body(&screen(), "TOK");
        assert!(
            b.contains("screen_name=Living%20room%20TV")
                && b.contains("lounge_token=TOK")
                && b.contains("app=cattube")
        );
        assert!(register_code_body(&screen(), "a&b").is_none());
        let t = parse_token(
            "{\"screens\":[{\"screenId\":\"abc123\",\"loungeToken\":\"AGdO5p_x\",\"refreshIntervalInMillis\":1123200000,\"expiration\":1}]}",
            1000,
        )
        .unwrap();
        assert_eq!(
            (t.token.as_str(), t.refresh_at_ms),
            ("AGdO5p_x", 1000 + 1_123_200_000)
        );
    }

    // Trimmed from a real session.
    const INIT: &str = "952\n[[0,[\"c\",\"6FF97F0E91BD5767\",\"\",8]]\n,[1,[\"S\",\"HWlGZ0jL\"]]\n,[2,[\"loungeStatus\",{\"queueId\":\"RQ0P\",\"devices\":\"[]\"}]]\n,[3,[\"playlistModified\",{\"videoIds\":\"\"}]]\n]\n";

    #[test]
    fn session_ids_and_listening() {
        let msgs = parse_reply(INIT);
        assert_eq!(msgs.len(), 4);
        let mut b = Bind::new("TOK", 5);
        let first = b.init_url(&screen(), "zx1");
        assert!(
            first.contains("RID=41005")
                && first.contains("CVER=1")
                && first.contains("device=LOUNGE_SCREEN")
        );
        assert!(b.listen_url(&screen(), "z").is_none());
        b.update(&msgs);
        // `c` carries the SID as its first argument.
        assert_eq!(b.sid.as_deref(), Some("6FF97F0E91BD5767"));
        assert_eq!(b.gsession.as_deref(), Some("HWlGZ0jL"));
        assert_eq!(b.aid, 3);
        let l = b.listen_url(&screen(), "z").unwrap();
        assert!(
            l.contains("RID=rpc")
                && l.contains("SID=6FF97F0E91BD5767")
                && l.contains("AID=3")
                && l.contains("TYPE=xmlhttp")
        );
    }

    #[test]
    fn streamed_commands() {
        let mut c = Chunks::default();
        let mut got = Vec::new();
        for l in [
            "16",
            "[[5,[\"noop\"]]",
            "]",
            "271",
            "[[12,[\"setPlaylist\",{\"listId\":\"RQ0P\",\"currentTime\":\"12\",\"videoId\":\"WhWc3b3KhnY\",\"videoIds\":\"aqz-KE-bpKQ,WhWc3b3KhnY\",\"currentIndex\":\"1\"}]]",
            "]",
            "18",
            "[[13,[\"pause\"]]",
            ",[14,[\"seekTo\",{\"newTime\":\"95\"}]]",
            ",[15,[\"setVolume\",{\"volume\":\"40\",\"muted\":\"false\"}]]",
            ",[16,[\"remoteConnected\",{\"name\":\"Pixel 9\",\"app\":\"android-phone\"}]]",
            "]",
        ] {
            got.extend(c.push_line(l));
        }
        let cmds: Vec<Command> = got.iter().filter_map(command).collect();
        assert_eq!(
            cmds,
            vec![
                Command::SetPlaylist {
                    queue: Queue {
                        list_id: "RQ0P".into(),
                        ids: vec!["aqz-KE-bpKQ".into(), "WhWc3b3KhnY".into()],
                        index: 1
                    },
                    start: 12.0
                },
                Command::Pause,
                Command::Seek(95.0),
                Command::SetVolume {
                    volume: 40,
                    muted: false
                },
                Command::Connected("Pixel 9".into()),
            ]
        );
        assert_eq!(got.last().unwrap().aid, 16);
        // A video that is not in the list plays alone.
        let lone =
            parse_reply("[[1,[\"setPlaylist\",{\"videoId\":\"WhWc3b3KhnY\",\"videoIds\":\"\"}]]]");
        let Some(Command::SetPlaylist { queue, start }) = command(&lone[0]) else {
            panic!()
        };
        assert_eq!((queue.ids.len(), queue.index, start), (1, 0, 0.0));
        assert!(
            command(&parse_reply("[[1,[\"setPlaylist\",{\"videoId\":\"bad\"}]]]")[0]).is_none()
        );
    }

    #[test]
    fn reports() {
        let mut b = Bind::new("TOK", 0);
        b.update(&parse_reply(INIT));
        let q = Queue {
            list_id: "RQ0P".into(),
            ids: vec!["WhWc3b3KhnY".into()],
            index: 0,
        };
        let p = Playing {
            video_id: "WhWc3b3KhnY".into(),
            time: 12.5,
            duration: 464.0,
            state: PLAYING,
            cpn: "x".into(),
        };
        let (url, body) = b
            .send(
                &screen(),
                "z",
                &[now_playing(Some(&p), &q), volume_changed(40, false)],
            )
            .unwrap();
        assert!(
            url.contains("SID=6FF97F0E91BD5767")
                && url.contains("RID=41000")
                && !url.contains("RID=rpc")
        );
        assert!(body.starts_with(
            "count=2&ofs=0&req0__sc=nowPlaying&req0_videoId=WhWc3b3KhnY&req0_currentTime=12.500"
        ));
        assert!(
            body.contains("req0_state=1")
                && body.contains("req0_listId=RQ0P")
                && body.contains("req1__sc=onVolumeChanged&req1_volume=40&req1_muted=false")
        );
        let (_, body2) = b.send(&screen(), "z", &[state_change(&p)]).unwrap();
        assert!(body2.starts_with("count=1&ofs=2&req0__sc=onStateChange"));
        assert_eq!(now_playing(None, &q).fields, vec![]);
        assert_eq!(lost(400), Lost::Session);
        assert_eq!(lost(401), Lost::Token);
        assert_eq!(cpn(&[0, 1, 63, 62]), "AB_-");
    }
}
