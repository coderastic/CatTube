//! Talking to mpv, the player CatTube uses, over its JSON IPC socket.
//!
//! The IPC protocol (mpv manual, "JSON IPC"): one JSON object per line,
//! `{"command":[...],"request_id":N}` → `{"request_id":N,"error":"success",
//! "data":...}`, interleaved with `{"event":...}` lines that we skip.
//!
//! Also here: a small JSON parser (the app has no other JSON needs) and the
//! URL check for anything handed to mpv.

use crate::error::{Error, Result};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Longest URL accepted for playback.
pub const MAX_URL_LEN: usize = 4096;
/// Largest IPC line we read.
const MAX_IPC_BYTES: u64 = 64 * 1024;
const IPC_TIMEOUT: Duration = Duration::from_secs(2);

/// Only absolute `http://` / `https://` URLs with a host, no whitespace or
/// control characters, at most [`MAX_URL_LEN`] bytes.
pub fn is_playable_url(url: &str) -> bool {
    if url.is_empty() || url.len() > MAX_URL_LEN {
        return false;
    }
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    let rest = if lower.starts_with("http://") {
        &url[7..]
    } else if lower.starts_with("https://") {
        &url[8..]
    } else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    !host.is_empty() && !host.starts_with(':') && !host.starts_with('@')
}

// ---------------------------------------------------------------------------
// Tiny JSON (just enough for mpv replies)
// ---------------------------------------------------------------------------

/// A parsed JSON value.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(kv) => kv.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }
}

/// YouTube's replies nest some 60 levels deep; still bounded.
const MAX_JSON_DEPTH: usize = 128;

/// Parse one JSON document (trailing whitespace allowed). Returns `None` on
/// any syntax error; never panics, depth-limited.
pub fn parse_json(s: &str) -> Option<Json> {
    let mut p = JsonParser {
        b: s.as_bytes(),
        i: 0,
    };
    let v = p.value(0)?;
    p.ws();
    (p.i == p.b.len()).then_some(v)
}

struct JsonParser<'a> {
    b: &'a [u8],
    i: usize,
}

impl JsonParser<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\r' | b'\n') {
            self.i += 1;
        }
    }
    fn eat(&mut self, c: u8) -> bool {
        if self.b.get(self.i) == Some(&c) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn lit(&mut self, word: &[u8]) -> bool {
        if self.b[self.i..].starts_with(word) {
            self.i += word.len();
            true
        } else {
            false
        }
    }
    fn value(&mut self, depth: usize) -> Option<Json> {
        if depth > MAX_JSON_DEPTH {
            return None;
        }
        self.ws();
        match *self.b.get(self.i)? {
            b'n' => self.lit(b"null").then_some(Json::Null),
            b't' => self.lit(b"true").then_some(Json::Bool(true)),
            b'f' => self.lit(b"false").then_some(Json::Bool(false)),
            b'"' => self.string().map(Json::Str),
            b'[' => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.eat(b']') {
                    return Some(Json::Arr(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    self.ws();
                    if self.eat(b']') {
                        return Some(Json::Arr(items));
                    }
                    if !self.eat(b',') {
                        return None;
                    }
                }
            }
            b'{' => {
                self.i += 1;
                let mut kv = Vec::new();
                self.ws();
                if self.eat(b'}') {
                    return Some(Json::Obj(kv));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    self.ws();
                    if !self.eat(b':') {
                        return None;
                    }
                    let v = self.value(depth + 1)?;
                    kv.push((k, v));
                    self.ws();
                    if self.eat(b'}') {
                        return Some(Json::Obj(kv));
                    }
                    if !self.eat(b',') {
                        return None;
                    }
                }
            }
            b'-' | b'0'..=b'9' => {
                let start = self.i;
                while self.i < self.b.len()
                    && matches!(
                        self.b[self.i],
                        b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'
                    )
                {
                    self.i += 1;
                }
                let text = std::str::from_utf8(&self.b[start..self.i]).ok()?;
                text.parse::<f64>()
                    .ok()
                    .filter(|n| n.is_finite())
                    .map(Json::Num)
            }
            _ => None,
        }
    }
    fn hex4(&mut self) -> Option<u32> {
        let h = self.b.get(self.i..self.i + 4)?;
        let s = std::str::from_utf8(h).ok()?;
        let v = u32::from_str_radix(s, 16).ok()?;
        self.i += 4;
        Some(v)
    }
    fn string(&mut self) -> Option<String> {
        if !self.eat(b'"') {
            return None;
        }
        let mut out: Vec<u8> = Vec::new();
        loop {
            let c = *self.b.get(self.i)?;
            self.i += 1;
            match c {
                b'"' => return String::from_utf8(out).ok(),
                b'\\' => {
                    let e = *self.b.get(self.i)?;
                    self.i += 1;
                    let ch = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let hi = self.hex4()?;
                            let code = if (0xd800..0xdc00).contains(&hi) {
                                if !(self.eat(b'\\') && self.eat(b'u')) {
                                    return None;
                                }
                                let lo = self.hex4()?;
                                if !(0xdc00..0xe000).contains(&lo) {
                                    return None;
                                }
                                0x10000 + ((hi - 0xd800) << 10) + (lo - 0xdc00)
                            } else {
                                hi
                            };
                            char::from_u32(code)?
                        }
                        _ => return None,
                    };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
                c if c < 0x20 => return None,
                c => out.push(c),
            }
        }
    }
}

/// JSON string literal (with quotes) for `s`.
pub fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{2028}' || c == '\u{2029}' => {
                out.push_str(&format!("\\u{:04x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// One argument of an mpv command.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Arg<'a> {
    Str(&'a str),
    Num(f64),
    Bool(bool),
}

/// `{"command":[...],"request_id":N}\n`
pub fn command_line(args: &[Arg], request_id: u64) -> String {
    let parts: Vec<String> = args
        .iter()
        .map(|a| match a {
            Arg::Str(s) => json_string(s),
            Arg::Num(n) if n.is_finite() => format!("{n}"),
            Arg::Num(_) => "0".into(),
            Arg::Bool(b) => b.to_string(),
        })
        .collect();
    format!(
        "{{\"command\":[{}],\"request_id\":{request_id}}}\n",
        parts.join(",")
    )
}

/// A value for mpv's `key=value,…` option lists (`loadfile` per-file
/// options) that may contain commas, quotes or `=`: mpv's length-prefixed
/// quoting, `%N%text` with N in bytes.
pub fn option_value(s: &str) -> String {
    format!("%{}%{s}", s.len())
}

/// A command reply (event lines give `None`).
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub request_id: Option<u64>,
    pub error: String,
    pub data: Json,
}

pub fn parse_reply(line: &str) -> Option<Reply> {
    let v = parse_json(line.trim())?;
    if v.get("event").is_some() {
        return None;
    }
    let error = v.get("error")?.as_str()?.to_string();
    Some(Reply {
        request_id: v
            .get("request_id")
            .and_then(Json::as_f64)
            .filter(|n| *n >= 0.0)
            .map(|n| n as u64),
        error,
        data: v.get("data").cloned().unwrap_or(Json::Null),
    })
}

static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// Send one command over the IPC socket and wait for its reply.
pub fn ipc_request(socket: &Path, args: &[Arg]) -> Result<Json> {
    let id = REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    let mut stream = UnixStream::connect(socket)
        .map_err(|e| Error::msg(format!("player is not reachable: {e}")))?;
    stream.set_read_timeout(Some(IPC_TIMEOUT))?;
    stream.set_write_timeout(Some(IPC_TIMEOUT))?;
    stream.write_all(command_line(args, id).as_bytes())?;
    let deadline = Instant::now() + IPC_TIMEOUT;
    let mut reader = BufReader::new((&stream).take(MAX_IPC_BYTES));
    let mut line = String::new();
    while Instant::now() < deadline {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                if let Some(reply) = parse_reply(&line)
                    && reply.request_id == Some(id)
                {
                    return if reply.error == "success" {
                        Ok(reply.data)
                    } else {
                        Err(Error::msg(format!("player: {}", reply.error)))
                    };
                }
            }
            Err(_) => break,
        }
    }
    Err(Error::msg("player did not answer"))
}

/// A long-lived IPC connection: commands get their replies, and the
/// `script-message`s scripts send (mpv delivers them to IPC clients as
/// `client-message` events) are kept for [`IpcConn::messages`]. CatTube's
/// player loop uses it to hear the "Next episode" card's buttons.
pub struct IpcConn {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    messages: Vec<Vec<String>>,
    eof: bool,
}

impl IpcConn {
    pub fn connect(socket: &Path) -> Result<Self> {
        let stream = UnixStream::connect(socket)
            .map_err(|e| Error::msg(format!("player is not reachable: {e}")))?;
        Self::from_stream(stream)
    }

    pub fn from_stream(stream: UnixStream) -> Result<Self> {
        stream.set_write_timeout(Some(IPC_TIMEOUT))?;
        let writer = stream.try_clone()?;
        Ok(Self {
            reader: BufReader::new(stream),
            writer,
            messages: Vec::new(),
            eof: false,
        })
    }

    /// The player closed the connection (it quit).
    pub fn closed(&self) -> bool {
        self.eof
    }

    /// One line, or `None` after `timeout` without one.
    fn line(&mut self, timeout: Duration) -> Option<String> {
        let _ = self
            .reader
            .get_ref()
            .set_read_timeout(Some(timeout.max(Duration::from_millis(1))));
        let mut line = String::new();
        match (&mut self.reader).take(MAX_IPC_BYTES).read_line(&mut line) {
            Ok(0) => {
                self.eof = true;
                None
            }
            Ok(_) => Some(line),
            Err(_) => None,
        }
    }

    fn keep_event(&mut self, line: &str) {
        if let Some(v) = parse_json(line.trim())
            && v.get("event").and_then(Json::as_str) == Some("client-message")
            && let Some(args) = v.get("args").and_then(Json::as_arr)
        {
            self.messages.push(
                args.iter()
                    .filter_map(|a| a.as_str().map(str::to_string))
                    .collect(),
            );
        }
    }

    /// Send a command and wait (≤ [`IPC_TIMEOUT`]) for its reply.
    pub fn command(&mut self, args: &[Arg]) -> Result<Json> {
        self.command_with_timeout(args, IPC_TIMEOUT)
    }

    pub fn command_with_timeout(&mut self, args: &[Arg], timeout: Duration) -> Result<Json> {
        let id = REQUEST_ID.fetch_add(1, Ordering::Relaxed);
        self.writer.write_all(command_line(args, id).as_bytes())?;
        let deadline = Instant::now() + timeout;
        while !self.eof {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let Some(line) = self.line(left) else {
                continue;
            };
            match parse_reply(&line) {
                Some(r) if r.request_id == Some(id) => {
                    return if r.error == "success" {
                        Ok(r.data)
                    } else {
                        Err(Error::msg(format!("player: {}", r.error)))
                    };
                }
                Some(_) => {}
                None => self.keep_event(&line),
            }
        }
        Err(Error::msg("player did not answer"))
    }

    /// Wait while idle, then return an input burst without delaying its action.
    pub fn messages(&mut self, wait: Duration) -> Vec<Vec<String>> {
        let burst = Duration::from_millis(1);
        let mut deadline = Instant::now()
            + if self.messages.is_empty() {
                wait
            } else {
                wait.min(burst)
            };
        while !self.eof {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            match self.line(left) {
                Some(line) => {
                    if parse_reply(&line).is_none() {
                        self.keep_event(&line);
                        if !self.messages.is_empty() {
                            deadline = deadline.min(Instant::now() + burst);
                        }
                    }
                }
                None => break,
            }
        }
        std::mem::take(&mut self.messages)
    }

    pub fn get_f64(&mut self, name: &str) -> Option<f64> {
        self.command(&[Arg::Str("get_property"), Arg::Str(name)])
            .ok()?
            .as_f64()
    }

    pub fn get_bool(&mut self, name: &str) -> Option<bool> {
        self.command(&[Arg::Str("get_property"), Arg::Str(name)])
            .ok()?
            .as_bool()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn url_validation() {
        assert!(is_playable_url("http://192.168.1.5:8200/MediaItems/22.mp4"));
        assert!(is_playable_url("HTTPS://example.com/a.m3u8?x=1&y=2"));
        assert!(!is_playable_url("file:///etc/passwd"));
        assert!(!is_playable_url("/etc/passwd"));
        assert!(!is_playable_url("--script=/tmp/x.lua"));
        assert!(!is_playable_url("http://"));
        assert!(!is_playable_url("http:///path"));
        assert!(!is_playable_url("http://a b/"));
        assert!(!is_playable_url("http://a/\n--x"));
        assert!(!is_playable_url("ytdl://x"));
        assert!(!is_playable_url(""));
        assert!(!is_playable_url(&format!(
            "http://a/{}",
            "x".repeat(MAX_URL_LEN)
        )));
    }

    #[test]
    fn json_parsing() {
        let v = parse_json(r#"{"data":1.468135,"request_id":100,"error":"success"}"#).unwrap();
        assert_eq!(v.get("data").unwrap().as_f64(), Some(1.468135));
        assert_eq!(
            parse_json(r#" {"a":[1,{"b":null}],"s":"x\"\\\u00e9\ud83d\ude00\n"} "#)
                .unwrap()
                .get("s")
                .unwrap()
                .as_str(),
            Some("x\"\\é😀\n")
        );
        assert_eq!(parse_json("true"), Some(Json::Bool(true)));
        assert_eq!(parse_json("-1.5e3"), Some(Json::Num(-1500.0)));
        for bad in [
            "",
            "{",
            "{\"a\"}",
            "[1,]",
            "tru",
            "\"\\x\"",
            "\"a",
            "{\"a\":1}x",
            "1e999",
            "\"\\ud800\"",
            "\"\u{1}\"",
            "[[[[[[[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]]]]]]",
        ] {
            assert!(parse_json(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn json_garbage_never_panics() {
        let mut seed: u32 = 7;
        let alphabet = b"{}[]\":,0123456789.-+eEtrufalsn\\u ";
        for len in 0..400 {
            let s: String = (0..len)
                .map(|_| {
                    seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                    alphabet[(seed >> 16) as usize % alphabet.len()] as char
                })
                .collect();
            let _ = parse_json(&s);
            let _ = parse_reply(&s);
        }
    }

    #[test]
    fn replies_and_events() {
        let r = parse_reply(r#"{"error":"success","data":true,"request_id":7}"#).unwrap();
        assert_eq!(r.request_id, Some(7));
        assert_eq!(r.data, Json::Bool(true));
        let r = parse_reply(r#"{"error":"property unavailable","request_id":8}"#).unwrap();
        assert_eq!(r.error, "property unavailable");
        assert_eq!(r.data, Json::Null);
        assert!(parse_reply(r#"{"event":"end-file","reason":"eof"}"#).is_none());
        assert!(parse_reply("garbage").is_none());
    }

    #[test]
    fn command_encoding() {
        assert_eq!(
            command_line(
                &[
                    Arg::Str("loadfile"),
                    Arg::Str("http://a/\"b\"\n"),
                    Arg::Str("replace")
                ],
                3
            ),
            "{\"command\":[\"loadfile\",\"http://a/\\\"b\\\"\\n\",\"replace\"],\"request_id\":3}\n"
        );
        assert_eq!(
            command_line(&[Arg::Str("seek"), Arg::Num(12.5), Arg::Bool(true)], 1),
            "{\"command\":[\"seek\",12.5,true],\"request_id\":1}\n"
        );
        assert_eq!(
            command_line(&[Arg::Num(f64::NAN)], 1),
            "{\"command\":[0],\"request_id\":1}\n"
        );
        // Round trip through our own parser.
        let line = command_line(&[Arg::Str("é\u{1}")], 9);
        let v = parse_json(line.trim()).unwrap();
        assert_eq!(v.get("request_id").unwrap().as_f64(), Some(9.0));
    }

    #[test]
    fn option_values_are_length_quoted() {
        assert_eq!(option_value("a,b=c"), "%5%a,b=c");
        assert_eq!(
            option_value("S1 · E2"),
            format!("%{}%S1 · E2", "S1 · E2".len())
        );
    }

    #[test]
    fn player_input_wakes_the_watcher_without_waiting_out_its_poll() {
        let (a, mut peer) = UnixStream::pair().unwrap();
        let mut conn = IpcConn::from_stream(a).unwrap();
        writeln!(
            peer,
            r#"{{"event":"client-message","args":["cattube-hide"]}}"#
        )
        .unwrap();
        let at = Instant::now();
        assert_eq!(
            conn.messages(Duration::from_secs(1)),
            vec![vec!["cattube-hide".to_string()]]
        );
        assert!(at.elapsed() < Duration::from_millis(200));
    }

    #[test]
    fn short_command_deadline_does_not_wait_for_the_default_timeout() {
        let (a, _peer) = UnixStream::pair().unwrap();
        let mut conn = IpcConn::from_stream(a).unwrap();
        let at = Instant::now();
        assert!(
            conn.command_with_timeout(
                &[Arg::Str("get_property"), Arg::Str("time-pos")],
                Duration::from_millis(20)
            )
            .is_err()
        );
        assert!(at.elapsed() < Duration::from_millis(300));
    }

    #[test]
    fn ipc_conn_keeps_script_messages_between_replies() {
        let (a, b) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let mut s = b;
            let mut reader = BufReader::new(s.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let id = parse_json(line.trim())
                .unwrap()
                .get("request_id")
                .unwrap()
                .as_f64()
                .unwrap() as u64;
            writeln!(
                s,
                r#"{{"event":"client-message","args":["cattube-card-result","play"]}}"#
            )
            .unwrap();
            writeln!(s, r#"{{"event":"property-change","name":"pause"}}"#).unwrap();
            writeln!(s, r#"{{"error":"success","data":12.5,"request_id":{id}}}"#).unwrap();
            writeln!(
                s,
                r#"{{"event":"client-message","args":["cattube-input"]}}"#
            )
            .unwrap();
            // Keep the socket open until the client has read everything.
            line.clear();
            let _ = reader.read_line(&mut line);
        });
        let mut c = IpcConn::from_stream(a).unwrap();
        assert_eq!(c.get_f64("time-pos"), Some(12.5));
        let mut got = c.messages(Duration::from_millis(300));
        if got.len() < 2 {
            got.extend(c.messages(Duration::from_millis(300)));
        }
        assert_eq!(
            got,
            vec![
                vec!["cattube-card-result".to_string(), "play".to_string()],
                vec!["cattube-input".to_string()],
            ]
        );
        assert!(c.messages(Duration::from_millis(10)).is_empty());
        drop(c);
        server.join().unwrap();
    }

    #[test]
    fn ipc_request_skips_events_and_matches_id() {
        let path =
            std::env::temp_dir().join(format!("cattube-mpv-test-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(s.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let req = parse_json(line.trim()).unwrap();
            let id = req.get("request_id").unwrap().as_f64().unwrap() as u64;
            let cmd = req.get("command").cloned().unwrap();
            writeln!(s, "{{\"event\":\"playback-restart\"}}").unwrap();
            writeln!(
                s,
                "{{\"error\":\"success\",\"data\":0,\"request_id\":{}}}",
                id + 1000
            )
            .unwrap();
            writeln!(
                s,
                "{{\"error\":\"success\",\"data\":42.5,\"request_id\":{id}}}"
            )
            .unwrap();
            cmd
        });
        let v = ipc_request(&path, &[Arg::Str("get_property"), Arg::Str("time-pos")]).unwrap();
        assert_eq!(v, Json::Num(42.5));
        assert_eq!(
            server.join().unwrap(),
            Json::Arr(vec![
                Json::Str("get_property".into()),
                Json::Str("time-pos".into())
            ])
        );
        let _ = std::fs::remove_file(&path);
    }
}
