//! CatTube's network and playback jobs (run on background threads). The
//! YouTube API, the sign-in and the screens are in `cattube_core`; this
//! file only runs `curl` / `mpv` / yt-dlp and turns the answers into data.

use cattube_core::account::{self, Client, DeviceCode, TokenReply};
use cattube_core::community;
use cattube_core::mpv::{self, Arg};
use cattube_core::player::{self, PlayOptions};
use cattube_core::process::{Cmd, Runner, SystemRunner};
use cattube_core::subs;
use cattube_core::youtube::{self as yt, Details, Page, Rating};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const TIMEOUT: Duration = Duration::from_secs(25);

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn run(cmd: &Cmd) -> Result<String, String> {
    match SystemRunner.run(cmd) {
        Ok(o) if o.success() => Ok(o.stdout),
        Ok(o) => Err(friendly(&format!("{} {}", o.stderr, o.stdout))),
        Err(e) => Err(friendly(&e.to_string())),
    }
}

/// A request's failure in a sentence for the TV.
pub fn friendly(t: &str) -> String {
    let t = t.to_ascii_lowercase();
    if t.contains("could not resolve")
        || t.contains("failed to connect")
        || t.contains("network is unreachable")
    {
        "No internet connection".into()
    } else if t.contains("timed out") || t.contains("timeout") {
        "YouTube took too long to answer".into()
    } else if t.contains("401") || t.contains("403") {
        "YouTube said no (sign in again?)".into()
    } else {
        "YouTube didn't answer properly".into()
    }
}

/// Request headers in a private file (0600) for `curl -H @file`, so a
/// token is never on a command line.
fn header_file(token: Option<&str>) -> Result<PathBuf, String> {
    header_file_for(token, false)
}

/// `web`: the web client's headers (live chat comes only through it).
fn header_file_for(token: Option<&str>, web: bool) -> Result<PathBuf, String> {
    let mut h = if web {
        format!(
            "Content-Type: application/json\nUser-Agent: {}\nX-YouTube-Client-Name: 1\nX-YouTube-Client-Version: {}\nOrigin: https://www.youtube.com\n",
            yt::WEB_USER_AGENT,
            yt::WEB_CLIENT_VERSION
        )
    } else {
        format!(
            "Content-Type: application/json\nUser-Agent: {}\nX-YouTube-Client-Name: {}\nX-YouTube-Client-Version: {}\nOrigin: https://www.youtube.com\nReferer: https://www.youtube.com/tv\n",
            yt::USER_AGENT,
            yt::CLIENT_NUMBER,
            yt::CLIENT_VERSION
        )
    };
    if let Some(t) = token {
        h.push_str(&format!("Authorization: Bearer {t}\n"));
    }
    private_file("headers", &h)
}

/// A file only this user can read, for what must not be on a command line.
fn private_file(kind: &str, text: &str) -> Result<PathBuf, String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = cattube_core::env::runtime_dir();
    let path = dir.join(format!("cattube-{kind}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| format!("CatTube could not prepare the request ({e})"))?;
    f.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    Ok(path)
}

/// A sign-in token request. The body (codes, tokens) goes in through a
/// private file. Answers to "not yet" come as HTTP errors: read them anyway.
fn token_request(body: &str) -> Result<cattube_core::process::Output, String> {
    let h = header_file(None)?;
    let b = private_file("body", body)?;
    let out = SystemRunner.run(
        &Cmd::new("curl")
            .args([
                "-sS".to_string(),
                "-m".to_string(),
                "20".to_string(),
                "-H".to_string(),
                format!("@{}", h.display()),
                "--data-binary".to_string(),
                format!("@{}", b.display()),
                "--".to_string(),
                account::TOKEN_URL.to_string(),
            ])
            .timeout(Duration::from_secs(25)),
    );
    let _ = std::fs::remove_file(&h);
    let _ = std::fs::remove_file(&b);
    out.map_err(|e| e.to_string())
}

/// Many plain GETs at once in one `curl` (one HTTP/2 connection, in
/// parallel). Each answer in order; empty when that one failed.
fn fetch_many(urls: &[String]) -> Vec<String> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir =
        cattube_core::env::runtime_dir().join(format!("cattube-many-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    if std::fs::create_dir(&dir).is_err() {
        return vec![String::new(); urls.len()];
    }
    // A curl config file: the addresses stay off the command line.
    let config: String = urls
        .iter()
        .enumerate()
        .map(|(i, u)| format!("url = \"{u}\"\noutput = \"{}/{i}\"\n", dir.display()))
        .collect();
    let list = dir.join("list");
    if std::fs::write(&list, config).is_ok() {
        let _ = SystemRunner.run(
            &Cmd::new("curl")
                .args([
                    "-sS".to_string(),
                    "--compressed".to_string(),
                    "--fail".to_string(),
                    "--parallel".to_string(),
                    "--parallel-max".to_string(),
                    "40".to_string(),
                    "-m".to_string(),
                    "20".to_string(),
                    "-K".to_string(),
                    list.display().to_string(),
                ])
                .timeout(Duration::from_secs(30)),
        );
    }
    let out = (0..urls.len())
        .map(|i| std::fs::read_to_string(dir.join(i.to_string())).unwrap_or_default())
        .collect();
    let _ = std::fs::remove_dir_all(&dir);
    out
}

/// One request; the header file is removed afterwards.
fn curl(url: &str, body: Option<&str>, token: Option<&str>) -> Result<String, String> {
    curl_as(url, body, token, false)
}

fn curl_as(
    url: &str,
    body: Option<&str>,
    token: Option<&str>,
    web: bool,
) -> Result<String, String> {
    let h = header_file_for(token, web)?;
    let mut args: Vec<String> = vec![
        "-sS".into(),
        "--compressed".into(),
        "--fail-with-body".into(),
        "-m".into(),
        TIMEOUT.as_secs().to_string(),
        "-H".into(),
        format!("@{}", h.display()),
    ];
    if let Some(b) = body {
        args.push("--data-binary".into());
        args.push(b.into());
    }
    args.push("--".into());
    args.push(url.into());
    let r = run(&Cmd::new("curl")
        .args(args)
        .timeout(TIMEOUT + Duration::from_secs(5)));
    let _ = std::fs::remove_file(&h);
    r
}

// ------------------------------------------------------------ signing in

static CLIENT: Mutex<Option<Client>> = Mutex::new(None);

/// The YouTube TV app's sign-in client, read from its own page once per run.
fn client() -> Result<Client, String> {
    if let Some(c) = CLIENT.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        return Ok(c);
    }
    let page = curl(account::TV_PAGE, None, None)?;
    let js = account::base_js_path(&page).ok_or("YouTube's TV page has changed")?;
    let base = curl(&format!("https://www.youtube.com{js}"), None, None)?;
    let c = account::parse_client(&base).ok_or("YouTube's TV app has changed")?;
    *CLIENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(c.clone());
    Ok(c)
}

/// A code to show on the TV.
pub fn sign_in_code(device_id: &str) -> Result<DeviceCode, String> {
    let c = client()?;
    let body = curl(
        account::CODE_URL,
        Some(&account::code_body(&c, &account::device_uuid(device_id))),
        None,
    )?;
    account::parse_code(&body).ok_or_else(|| "YouTube didn't give a code".into())
}

/// Wait for the code to be entered. `keep_going` turns false when the
/// screen is left. Returns (refresh token, access token, expires at).
pub fn sign_in_wait(
    code: &DeviceCode,
    keep_going: &dyn Fn() -> bool,
) -> Result<(String, String, u64), String> {
    let c = client()?;
    let started = Instant::now();
    let mut every = Duration::from_secs(code.interval);
    while keep_going() {
        std::thread::sleep(every);
        if !keep_going() {
            break;
        }
        if started.elapsed() > Duration::from_secs(code.expires_in) {
            return Err("The code expired. Press OK for a new one".into());
        }
        let out = token_request(&account::poll_body(&c, &code.device_code));
        let Ok(out) = out else { continue };
        match account::parse_token(&out.stdout) {
            TokenReply::Token {
                access,
                expires_in,
                refresh: Some(refresh),
            } => return Ok((refresh, access, now() + expires_in)),
            TokenReply::Token { .. } => return Err("YouTube didn't keep the sign-in".into()),
            TokenReply::Pending => {}
            TokenReply::SlowDown => every += Duration::from_secs(2),
            TokenReply::Failed(e) => return Err(e),
        }
    }
    Err("Cancelled".into())
}

/// Google's endpoint to cancel a sign-in.
const REVOKE_URL: &str = "https://oauth2.googleapis.com/revoke";

/// Signing out: the sign-in stops working at Google too (not only on this
/// TV), and its access token is forgotten at once. The token goes in
/// through a private file, never on a command line. Best effort: true when
/// Google confirmed it.
pub fn revoke(refresh: &str) -> bool {
    TOKENS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .remove(refresh);
    let Ok(body) = private_file(
        "body",
        &format!("token={}", cattube_core::util::percent_encode(refresh)),
    ) else {
        return false;
    };
    let out = SystemRunner.run(
        &Cmd::new("curl")
            .args([
                "-sS".to_string(),
                "-m".to_string(),
                "20".to_string(),
                "-o".to_string(),
                "/dev/null".to_string(),
                "-w".to_string(),
                "%{http_code}".to_string(),
                "-H".to_string(),
                "Content-Type: application/x-www-form-urlencoded".to_string(),
                "--data-binary".to_string(),
                format!("@{}", body.display()),
                "--".to_string(),
                REVOKE_URL.to_string(),
            ])
            .timeout(Duration::from_secs(25)),
    );
    let _ = std::fs::remove_file(&body);
    out.is_ok_and(|o| o.stdout.trim() == "200")
}

/// Access tokens by refresh token (they last an hour).
static TOKENS: Mutex<Option<HashMap<String, (String, u64)>>> = Mutex::new(None);

pub fn remember_token(refresh: &str, access: &str, expires_at: u64) {
    TOKENS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(refresh.to_string(), (access.to_string(), expires_at));
}

/// A working token for the account with this refresh token.
pub fn bearer(refresh: &str) -> Result<String, String> {
    if let Some((a, exp)) = TOKENS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|m| m.get(refresh).cloned())
        && now() + 60 < exp
    {
        return Ok(a);
    }
    let c = client()?;
    let out = token_request(&account::refresh_body(&c, refresh));
    let out = out.map_err(|e: String| friendly(&e))?;
    match account::parse_token(&out.stdout) {
        TokenReply::Token {
            access, expires_in, ..
        } => {
            remember_token(refresh, &access, now() + expires_in);
            Ok(access)
        }
        TokenReply::Failed(e) => Err(e),
        _ => Err("YouTube didn't renew the sign-in".into()),
    }
}

fn token(refresh: Option<&str>) -> Result<Option<String>, String> {
    refresh.map(bearer).transpose()
}

/// The signed-in account's name and picture.
pub fn account_name(ctx: &str, refresh: &str) -> Option<(String, String)> {
    let t = bearer(refresh).ok()?;
    let body = curl(
        &yt::endpoint_url("account/accounts_list"),
        Some(&account::accounts_list_body(ctx)),
        Some(&t),
    )
    .ok()?;
    account::parse_account_name(&body)
}

// --------------------------------------------------------------- lists

pub fn page(ctx: &str, browse_id: &str, refresh: Option<&str>) -> Result<Page, String> {
    if browse_id == yt::feeds::HOME && refresh.is_none() {
        return explore(ctx);
    }
    let t = token(refresh)?;
    let body = yt::browse_body(ctx, browse_id).ok_or("That can't be opened")?;
    let reply = curl(&yt::endpoint_url("browse"), Some(&body), t.as_deref())?;
    yt::parse_page(&reply).ok_or_else(|| "YouTube didn't answer properly".into())
}

/// A page of a video's comments: the first (`token` empty) or the next.
pub fn comments(
    ctx: &str,
    id: &str,
    token_: &str,
    refresh: Option<&str>,
) -> Result<yt::Comments, String> {
    let t = token(refresh)?;
    let first;
    let token_ = if token_.is_empty() {
        let body = yt::next_body(ctx, id).ok_or("not a video")?;
        let reply = curl(&yt::endpoint_url("next"), Some(&body), t.as_deref())?;
        first = yt::comments_token(&reply).ok_or("Comments are turned off")?;
        first.as_str()
    } else {
        token_
    };
    let body = yt::continuation_body(ctx, token_).ok_or("bad continuation")?;
    let reply = curl(&yt::endpoint_url("next"), Some(&body), t.as_deref())?;
    yt::parse_comments(&reply).ok_or_else(|| "YouTube didn't answer properly".into())
}

/// A live video's chat token (None: not live, or no chat).
pub fn live_chat_token(id: &str, region: &str) -> Option<String> {
    let ctx = yt::web_context("en", region);
    let body = yt::next_body(&ctx, id)?;
    let reply = curl_as(&yt::endpoint_url("next"), Some(&body), None, true).ok()?;
    yt::live_chat_token(&reply)
}

/// The chat messages since `token` (and the next token).
pub fn live_chat(token_: &str, region: &str) -> Result<yt::Chat, String> {
    let ctx = yt::web_context("en", region);
    let body = yt::continuation_body(&ctx, token_).ok_or("bad continuation")?;
    let reply = curl_as(
        &yt::endpoint_url("live_chat/get_live_chat"),
        Some(&body),
        None,
        true,
    )?;
    yt::parse_live_chat(&reply).ok_or_else(|| "The chat has ended".into())
}

/// Chat messages for the control bar, as JSON: `[{"a","t","p"}]`.
fn chat_json(msgs: &[yt::ChatMessage]) -> String {
    let items: Vec<String> = msgs
        .iter()
        .map(|m| {
            format!(
                "{{\"a\":{},\"t\":{},\"p\":{}}}",
                mpv::json_string(&m.author),
                mpv::json_string(&m.text),
                mpv::json_string(&m.paid)
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

/// Follow a live chat into the control bar while `on` stays set.
fn follow_chat(first: String, region: String, on: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    use std::sync::atomic::Ordering;
    std::thread::spawn(move || {
        let socket = player::player_socket();
        let mut token = first;
        let mut seen: std::collections::VecDeque<String> = Default::default();
        while on.load(Ordering::SeqCst) {
            let (wait, next) = match live_chat(&token, &region) {
                Ok(c) => {
                    let fresh: Vec<yt::ChatMessage> = c
                        .messages
                        .into_iter()
                        .filter(|m| m.id.is_empty() || !seen.contains(&m.id))
                        .collect();
                    for m in &fresh {
                        seen.push_back(m.id.clone());
                        if seen.len() > 500 {
                            seen.pop_front();
                        }
                    }
                    if !fresh.is_empty() && on.load(Ordering::SeqCst) {
                        let json = chat_json(&fresh);
                        if mpv::ipc_request(
                            &socket,
                            &[
                                Arg::Str("script-message"),
                                Arg::Str("cattube-chat-data"),
                                Arg::Str(&json),
                            ],
                        )
                        .is_err()
                        {
                            return; // the player is gone
                        }
                    }
                    match c.next {
                        Some((t, ms)) => (Duration::from_millis(ms), Some(t)),
                        None => (Duration::ZERO, None),
                    }
                }
                Err(_) => (Duration::from_secs(10), Some(token.clone())),
            };
            let Some(next) = next else { return };
            token = next;
            // Sleep in short steps so turning chat off is quick.
            let until = Instant::now() + wait;
            while Instant::now() < until && on.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    });
}

/// Comments for the control bar, as JSON: `{"list":[{"a","t","p","l","r"}],
/// "more":"…"}` or `{"error":"…"}`.
fn comments_json(r: Result<yt::Comments, String>) -> String {
    match r {
        Ok(c) => {
            let list = c
                .list
                .iter()
                .map(|x| {
                    format!(
                        "{{\"a\":{},\"t\":{},\"p\":{},\"l\":{},\"r\":{}}}",
                        mpv::json_string(&x.author),
                        mpv::json_string(&x.text),
                        mpv::json_string(&x.published),
                        mpv::json_string(&x.likes),
                        x.replies
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "{{\"list\":[{list}],\"more\":{}}}",
                mpv::json_string(c.more.as_deref().unwrap_or(""))
            )
        }
        Err(e) => format!("{{\"error\":{}}}", mpv::json_string(&e)),
    }
}

/// Subscriptions as a row per channel you follow, the newest first
/// (`cattube_core::subs`).
pub fn subscriptions(ctx: &str, refresh: Option<&str>) -> Result<Page, String> {
    let (channels, uploads) = subscription_feeds(ctx, refresh)?;
    Ok(subs::rows(&channels, &uploads))
}

/// The channels you follow, and each one's newest uploads from its public
/// feed (fetched a dozen at a time).
pub fn subscription_feeds(
    ctx: &str,
    refresh: Option<&str>,
) -> Result<(Vec<yt::Tile>, Vec<Vec<subs::Upload>>), String> {
    let mut list = page(ctx, yt::feeds::CHANNELS, refresh)?;
    let mut channels: Vec<yt::Tile> = Vec::new();
    for _ in 0..20 {
        for t in list.tiles() {
            if t.kind == yt::Kind::Channel
                && subs::feed_url(&t.id).is_some()
                && !channels.iter().any(|c| c.id == t.id)
            {
                channels.push(t);
            }
        }
        let Some(c) = list.continuation.clone() else {
            break;
        };
        list = more(ctx, &c, refresh)?;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let urls: Vec<String> = channels
        .iter()
        .filter_map(|c| subs::feed_url(&c.id))
        .collect();
    let uploads: Vec<Vec<subs::Upload>> = fetch_many(&urls)
        .iter()
        .map(|xml| subs::parse_feed(xml, now))
        .collect();
    if !channels.is_empty() && uploads.iter().all(Vec::is_empty) {
        return Err("YouTube didn't answer".into());
    }
    Ok((channels, uploads))
}

/// Home, signed out: YouTube's topic channels, fetched side by side.
fn explore(ctx: &str) -> Result<Page, String> {
    let handles: Vec<_> = yt::feeds::TOPICS
        .iter()
        .map(|(name, id)| {
            let (ctx, id) = (ctx.to_string(), id.to_string());
            (*name, std::thread::spawn(move || page(&ctx, &id, None)))
        })
        .collect();
    let mut topics = Vec::new();
    let mut error = None;
    for (name, h) in handles {
        match h.join() {
            Ok(Ok(p)) => topics.push((name, p)),
            Ok(Err(e)) => error = Some(e),
            Err(_) => {}
        }
    }
    let p = yt::explore(topics, 2);
    match (p.shelves.is_empty(), error) {
        (true, Some(e)) => Err(e),
        _ => Ok(p),
    }
}

pub fn more(ctx: &str, token_: &str, refresh: Option<&str>) -> Result<Page, String> {
    let t = token(refresh)?;
    let body = yt::continuation_body(ctx, token_).ok_or("bad continuation")?;
    let reply = curl(&yt::endpoint_url("browse"), Some(&body), t.as_deref())?;
    yt::parse_page(&reply).ok_or_else(|| "YouTube didn't answer properly".into())
}

/// The next results of a search (its continuation).
pub fn search_more(ctx: &str, token_: &str, refresh: Option<&str>) -> Result<Page, String> {
    let t = token(refresh)?;
    let body = yt::continuation_body(ctx, token_).ok_or("bad continuation")?;
    let reply = curl(&yt::endpoint_url("search"), Some(&body), t.as_deref())?;
    yt::parse_page(&reply).ok_or_else(|| "YouTube didn't answer properly".into())
}

pub fn search(ctx: &str, q: &str, refresh: Option<&str>) -> Result<Page, String> {
    let t = token(refresh)?;
    let reply = curl(
        &yt::endpoint_url("search"),
        Some(&yt::search_body(ctx, q)),
        t.as_deref(),
    )?;
    yt::parse_page(&reply).ok_or_else(|| "YouTube didn't answer properly".into())
}

pub fn suggestions(q: &str) -> Vec<String> {
    curl(&yt::suggest_url(q), None, None)
        .map(|b| yt::parse_suggestions(&b))
        .unwrap_or_default()
}

pub fn details(ctx: &str, id: &str, refresh: Option<&str>) -> Result<Details, String> {
    let t = token(refresh)?;
    let body = yt::next_body(ctx, id).ok_or("not a video")?;
    let reply = curl(&yt::endpoint_url("next"), Some(&body), t.as_deref())?;
    yt::parse_details(&reply).ok_or_else(|| "YouTube didn't answer properly".into())
}

pub fn rate(ctx: &str, id: &str, r: Rating, refresh: &str) -> Result<(), String> {
    let t = bearer(refresh)?;
    let (url, body) = yt::rate(ctx, id, r).ok_or("not a video")?;
    curl(&url, Some(&body), Some(&t)).map(|_| ())
}

pub fn subscribe(ctx: &str, channel: &str, on: bool, refresh: &str) -> Result<(), String> {
    let t = bearer(refresh)?;
    let (url, body) = yt::subscribe(ctx, channel, on).ok_or("not a channel")?;
    curl(&url, Some(&body), Some(&t)).map(|_| ())
}

pub fn watch_later(ctx: &str, id: &str, refresh: &str) -> Result<(), String> {
    let t = bearer(refresh)?;
    let body = format!(
        "{{\"context\":{ctx},\"playlistId\":\"WL\",\"actions\":[{{\"addedVideoId\":{},\"action\":\"ACTION_ADD_VIDEO\"}}]}}",
        mpv::json_string(id)
    );
    curl(
        &yt::endpoint_url("browse/edit_playlist"),
        Some(&body),
        Some(&t),
    )
    .map(|_| ())
}

/// Like and dislike counts (Return YouTube Dislike).
pub fn votes(id: &str) -> Option<(u64, u64)> {
    curl(&community::votes_url(id), None, None)
        .ok()
        .and_then(|b| community::parse_votes(&b))
}

// ------------------------------------------------------------- pictures

/// `images/<hash>.<ext>` in the cache folder for a picture address.
fn image_path(cache: &Path, url: &str) -> PathBuf {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in url.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    let ext = if url.contains(".webp") {
        "webp"
    } else if url.contains(".png") {
        "png"
    } else {
        "jpg"
    };
    cache.join("images").join(format!("{h:016x}.{ext}"))
}

/// A softened copy of a picture (backdrops): scaled down and blurred.
fn blur_into(src: &Path, out: &Path) -> Option<()> {
    let img = image::load_from_memory(&std::fs::read(src).ok()?).ok()?;
    let small = img.resize(480, 270, image::imageops::FilterType::Triangle);
    let soft = image::imageops::blur(&small.to_rgb8(), 7.0);
    soft.save_with_format(out, image::ImageFormat::Jpeg).ok()
}

/// Download a picture into the cache; returns its path. An address ending
/// in `#blur` gives the blurred copy (the picture behind a focused video).
pub fn fetch_image(cache: &Path, url: &str) -> Option<String> {
    if let Some(base) = url.strip_suffix("#blur") {
        let path = image_path(cache, url);
        if path.is_file() {
            return Some(path.to_string_lossy().into_owned());
        }
        let src = fetch_image(cache, base)?;
        blur_into(Path::new(&src), &path)?;
        return Some(path.to_string_lossy().into_owned());
    }
    if !cattube_core::mpv::is_playable_url(url) {
        return None;
    }
    let path = image_path(cache, url);
    if path.is_file() {
        return Some(path.to_string_lossy().into_owned());
    }
    let _ = std::fs::create_dir_all(path.parent()?);
    // Each download its own partial file: the card and the blurred
    // backdrop can ask for the same picture at once.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_extension(format!("{}-{n}.part", std::process::id()));
    let ok = SystemRunner
        .run(
            &Cmd::new("curl")
                .args([
                    "-sS".to_string(),
                    "--fail".to_string(),
                    "-m".to_string(),
                    "20".to_string(),
                    "-o".to_string(),
                    tmp.display().to_string(),
                    "--".to_string(),
                    url.to_string(),
                ])
                .timeout(Duration::from_secs(25)),
        )
        .is_ok_and(|o| o.success())
        && std::fs::rename(&tmp, &path).is_ok();
    if ok {
        Some(path.to_string_lossy().into_owned())
    } else {
        let _ = std::fs::remove_file(&tmp);
        None
    }
}

// ---------------------------------------------------------------- tools

/// Make sure yt-dlp (and a JavaScript runtime for it) are there, and keep
/// yt-dlp up to date. Returns a line for Settings.
pub fn ensure_tools(data: &Path) -> Result<String, String> {
    let _ = std::fs::create_dir_all(data);
    let ytdlp = player::yt_dlp_path(data);
    if !ytdlp.is_file() {
        let tmp = ytdlp.with_extension("part");
        run(&player::download_cmd(player::YT_DLP_URL, &tmp))
            .map_err(|_| "Could not download yt-dlp".to_string())?;
        set_exec(&tmp);
        std::fs::rename(&tmp, &ytdlp).map_err(|e| e.to_string())?;
        touch(&data.join("yt-dlp.checked"));
    } else if stale(&data.join("yt-dlp.checked")) {
        let _ = run(&player::update_cmd(&ytdlp));
        touch(&data.join("yt-dlp.checked"));
    }
    let find = |n: &str| cattube_core::process::find_executable(n);
    if player::js_runtime(data, &find).is_none() {
        let zip = data.join("deno.zip");
        run(&player::download_cmd(player::DENO_URL, &zip))
            .map_err(|_| "Could not download Deno (needed for full quality)".to_string())?;
        let r = run(&player::unzip_cmd(&zip, data));
        let _ = std::fs::remove_file(&zip);
        r.map_err(|_| "Could not unpack Deno".to_string())?;
        set_exec(&player::own_deno_path(data));
    }
    Ok(ytdlp_version(data))
}

/// Update yt-dlp now (Settings).
pub fn update_tools(data: &Path) -> Result<String, String> {
    let _ = std::fs::remove_file(data.join("yt-dlp.checked"));
    ensure_tools(data)
}

pub fn ytdlp_version(data: &Path) -> String {
    SystemRunner
        .run(&Cmd::new(player::yt_dlp_path(data).display().to_string()).arg("--version"))
        .ok()
        .filter(|o| o.success())
        .map(|o| format!("yt-dlp {}", o.stdout.trim()))
        .unwrap_or_else(|| "yt-dlp is not there yet".into())
}

fn set_exec(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755));
}

fn touch(p: &Path) {
    let _ = std::fs::write(p, now().to_string());
}

fn stale(p: &Path) -> bool {
    std::fs::read_to_string(p)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .is_none_or(|t| now().saturating_sub(t) > player::UPDATE_EVERY.as_secs())
}

// ---------------------------------------------------------------- player

/// Capture the actual player before replacing or closing it, not its last poll.
pub fn checkpoint_player(data: &Path) {
    let Ok(mut conn) = mpv::IpcConn::connect(&player::player_socket()) else {
        return;
    };
    let budget = Duration::from_millis(80);
    let Ok(path) = conn.command_with_timeout(&[Arg::Str("get_property"), Arg::Str("path")], budget)
    else {
        return;
    };
    let Some(path) = path.as_str() else { return };
    let id = path
        .split(['?', '&'])
        .find_map(|part| part.strip_prefix("v="))
        .or_else(|| path.split('?').next()?.rsplit('/').next())
        .unwrap_or("");
    if !yt::is_video_id(id) {
        return;
    }
    let get = |conn: &mut mpv::IpcConn, name: &str| {
        conn.command_with_timeout(&[Arg::Str("get_property"), Arg::Str(name)], budget)
            .ok()
            .and_then(|j| j.as_f64())
    };
    if let Some(position) = get(&mut conn, "time-pos") {
        let duration = get(&mut conn, "duration");
        let same_video = conn
            .command_with_timeout(&[Arg::Str("get_property"), Arg::Str("path")], budget)
            .ok()
            .is_some_and(|j| j.as_str() == Some(path));
        if !same_video {
            return;
        }
        let finished =
            player::checkpoint(data, id).is_some_and(|p| p.finished && position >= p.position);
        let _ = player::store_checkpoint(
            data,
            id,
            player::Checkpoint {
                position,
                duration,
                finished,
            },
        );
    }
}

/// Start mpv for a video (its own process group, stdin closed).
pub fn start_player(o: &PlayOptions, data: &Path) -> Result<std::process::Child, String> {
    std::fs::create_dir_all(data.join("positions"))
        .map_err(|e| format!("Could not save playback positions ({e})"))?;
    let find = |n: &str| cattube_core::process::find_executable(n);
    let js = player::js_runtime(data, &find);
    let argv = player::mpv_argv(
        o,
        data,
        &cattube_core::env::data_dir(),
        &player::player_socket(),
        js.as_deref(),
    );
    let _ = std::fs::remove_file(player::player_socket());
    std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(std::process::Stdio::null())
        // In CatTube's own process group: a TV shell that closes CatTube
        // (signalling the group) closes the player with it.
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => "mpv is not installed".to_string(),
            _ => format!("Could not start the player ({e})"),
        })
}

/// CatTube's own window (its Wayland app id) and the player's.
pub const APP_WINDOW: &str = "io.github.coderastic.CatTube";
pub const PLAYER_WINDOW: &str = "cattube-player";

/// Bring a window to the front through CatosTV (`catostv-ctl raise`).
pub fn raise_window(window: &str) -> bool {
    std::process::Command::new("catostv-ctl")
        .args(["raise", window])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// What the player loop needs from the app.
pub struct Session {
    pub ctx: String,
    pub refresh: Option<String>,
    pub cache: PathBuf,
    pub autoplay: crate::prefs::Autoplay,
    pub sponsorblock: Option<Vec<String>>,
    pub codec: cattube_core::player::Codec,
    pub hdr: bool,
    /// For live chat ("US").
    pub region: String,
    /// Videos after this one (a playlist or a row), in order.
    pub queue: Vec<yt::Tile>,
    /// Back in the player hid the video (`hide_on_back`): told the title
    /// of what plays on behind CatTube's list.
    pub on_hide: Option<Box<dyn Fn(yt::Tile) + Send>>,
    pub on_video: Option<Box<dyn Fn(yt::Tile) + Send>>,
}

/// How a playback ended, for the app.
#[derive(Debug, Default)]
pub struct Ended {
    /// (video, position, duration) for every video that played.
    pub positions: Vec<(String, f64, Option<f64>)>,
    pub speed: Option<f64>,
}

fn osd(conn: &mut mpv::IpcConn, text: &str) {
    let _ = conn.command(&[Arg::Str("show-text"), Arg::Str(text), Arg::Num(2500.0)]);
}

fn card(conn: &mut mpv::IpcConn, args: &[&str]) {
    let mut v = vec![
        Arg::Str("script-message-to"),
        Arg::Str("cattube_osd"),
        Arg::Str("cattube-card"),
    ];
    v.extend(args.iter().map(|a| Arg::Str(a)));
    let _ = conn.command(&v);
}

/// Raw thumbnail pixels for a player overlay.
fn bgra_thumbnail(
    cache: &Path,
    id: &str,
    osd_width: f64,
    width: f64,
    name: &str,
) -> Option<(String, u32, u32)> {
    let scale = (osd_width / 1920.0).clamp(0.5, 4.0);
    let (w, h) = ((width * scale) as u32, (width * 9.0 / 16.0 * scale) as u32);
    let pic = fetch_image(cache, &yt::video_thumb(id))?;
    let img = image::load_from_memory(&std::fs::read(pic).ok()?).ok()?;
    let mut px = img
        .resize_to_fill(w, h, image::imageops::FilterType::Triangle)
        .to_rgba8()
        .into_raw();
    for p in px.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
    }
    let path = player::player_socket().with_file_name(name);
    std::fs::write(&path, px).ok()?;
    Some((path.to_string_lossy().into_owned(), w, h))
}

/// The related browser shows up to six rows, loading pictures progressively.
const RELATED: usize = 24;
const RELATED_THUMB_W: f64 = 390.0;
const RELATED_STEP_X: usize = 430;
const RELATED_STEP_Y: usize = 320;
const RELATED_ATLAS_W: usize = 1680;

fn related_json(tiles: &[yt::Tile], atlas: Option<(&str, usize, usize, usize)>) -> String {
    let list: Vec<_> = tiles
        .iter()
        .map(|t| {
            format!(
                "{{\"id\":{},\"t\":{},\"c\":{}}}",
                mpv::json_string(&t.id),
                mpv::json_string(&t.title),
                mpv::json_string(&t.byline),
            )
        })
        .collect();
    let atlas = atlas.map_or_else(
        || "null".to_string(),
        |(p, w, h, v)| {
            format!(
                "{{\"p\":{},\"w\":{w},\"h\":{h},\"v\":{v}}}",
                mpv::json_string(p)
            )
        },
    );
    format!("{{\"list\":[{}],\"atlas\":{atlas}}}", list.join(","))
}

#[derive(Default)]
struct RelatedRequests {
    generation: std::sync::atomic::AtomicU64,
    paths: std::sync::Mutex<Vec<String>>,
}

impl RelatedRequests {
    fn begin(&self) -> u64 {
        let next = self
            .generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        for path in self
            .paths
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
        {
            let _ = std::fs::remove_file(path);
        }
        next
    }

    fn current(&self, expected: u64) -> bool {
        self.generation.load(std::sync::atomic::Ordering::Relaxed) == expected
    }

    fn keep(&self, path: &str, expected: u64) -> bool {
        let mut paths = self.paths.lock().unwrap_or_else(|e| e.into_inner());
        if self.current(expected) {
            paths.push(path.to_string());
            true
        } else {
            let _ = std::fs::remove_file(path);
            false
        }
    }
}

fn send_related(
    session: &Session,
    id: &str,
    request: &str,
    requests: &RelatedRequests,
    expected: u64,
) {
    let current = || requests.current(expected);
    let send = |json: &str| {
        if current() {
            let _ = mpv::ipc_request(
                &player::player_socket(),
                &[
                    Arg::Str("script-message"),
                    Arg::Str("cattube-related-data"),
                    Arg::Str(id),
                    Arg::Str(json),
                    Arg::Str(request),
                ],
            );
        }
    };
    let details = match details(&session.ctx, id, session.refresh.as_deref()) {
        Ok(d) => d,
        Err(e) => {
            send(&format!("{{\"error\":{}}}", mpv::json_string(&e)));
            return;
        }
    };
    let tiles: Vec<_> = details
        .related
        .into_iter()
        .filter(|t| t.playable())
        .take(RELATED)
        .collect();
    if tiles.is_empty() {
        send("{\"error\":\"No related videos\"}");
        return;
    }
    send(&related_json(&tiles, None));
    // One transparent image plane keeps every thumbnail in the same frame.
    // Its source stays at design resolution even on a 4K screen.
    let height = tiles.len().div_ceil(4) * RELATED_STEP_Y;
    let mut pixels = vec![0u8; RELATED_ATLAS_W * height * 4];
    let safe_id: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    let path =
        player::player_socket().with_file_name(format!("cattube-related-{safe_id}-{request}.bgra"));
    let tmp = path.with_extension("part");
    let name = path.to_string_lossy().into_owned();
    let mut tracked = false;
    let mut revision = 0;
    let mut changed = false;
    let mut sent_at = Instant::now();
    for (i, tile) in tiles.iter().enumerate() {
        if !current() {
            return;
        }
        if let Some((pic, w, h)) = bgra_thumbnail(
            &session.cache,
            &tile.id,
            1920.0,
            RELATED_THUMB_W,
            &format!("cattube-related-{safe_id}-{request}-thumb.bgra"),
        ) {
            let bytes = std::fs::read(&pic);
            let _ = std::fs::remove_file(&pic);
            if let Ok(bytes) = bytes
                && bytes.len() == w as usize * h as usize * 4
            {
                for row in 0..h as usize {
                    let dest = ((i / 4 * RELATED_STEP_Y + row) * RELATED_ATLAS_W
                        + i % 4 * RELATED_STEP_X)
                        * 4;
                    let source = row * w as usize * 4;
                    pixels[dest..dest + w as usize * 4]
                        .copy_from_slice(&bytes[source..source + w as usize * 4]);
                }
                changed = true;
            }
        }
        if changed && (sent_at.elapsed() >= Duration::from_millis(80) || i + 1 == tiles.len()) {
            if !current() {
                return;
            }
            if std::fs::write(&tmp, &pixels).is_ok() && std::fs::rename(&tmp, &path).is_ok() {
                if !current() {
                    let _ = std::fs::remove_file(&path);
                    return;
                }
                if !tracked {
                    if !requests.keep(&name, expected) {
                        return;
                    }
                    tracked = true;
                }
                revision += 1;
                send(&related_json(
                    &tiles,
                    Some((&name, RELATED_ATLAS_W, height, revision)),
                ));
                sent_at = Instant::now();
                changed = false;
            } else {
                let _ = std::fs::remove_file(&tmp);
            }
        }
    }
}

fn episode_controls(conn: &mut mpv::IpcConn, previous: bool, next: bool, waiting: bool) {
    let _ = conn.command(&[
        Arg::Str("script-message-to"),
        Arg::Str("cattube_osd"),
        Arg::Str("cattube-episodes"),
        Arg::Str(if previous { "yes" } else { "no" }),
        Arg::Str(if next { "yes" } else { "no" }),
    ]);
    let _ = conn.command(&[
        Arg::Str("set_property"),
        Arg::Str("keep-open"),
        Arg::Str(if next || waiting { "yes" } else { "no" }),
    ]);
}

struct CardThumbnail((String, u32, u32));
impl Drop for CardThumbnail {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0.0);
    }
}

fn fetch_card_thumbnail(
    cache: PathBuf,
    id: String,
    width: f64,
) -> std::sync::mpsc::Receiver<Option<CardThumbnail>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let serial = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let pic = if yt::is_video_id(&id) {
            bgra_thumbnail(
                &cache,
                &id,
                width,
                512.0,
                &format!("cattube-next-{}-{serial}.bgra", std::process::id()),
            )
            .map(CardThumbnail)
        } else {
            None
        };
        let _ = tx.send(pic);
    });
    rx
}

/// Watch the player until it goes away: SponsorBlock skips, the speed the
/// viewer picked, quality changes from the control bar, and what plays
/// next (an "Up next" card counting down, a loop, or nothing). Runs on
/// its own thread; `done` is called when the player has gone.
pub fn spawn_watcher(first: yt::Tile, s: Session, done: impl FnOnce(Ended) + Send + 'static) {
    let _ = std::thread::Builder::new()
        .name("cattube-player".into())
        .spawn(move || {
            let socket = player::player_socket();
            let mut ended = Ended::default();
            let started = Instant::now();
            let mut conn = loop {
                if let Ok(mut c) = mpv::IpcConn::connect(&socket)
                    && c.get_f64("time-pos").is_some()
                {
                    break c;
                }
                if started.elapsed() > Duration::from_secs(60) {
                    done(ended);
                    return;
                }
                std::thread::sleep(Duration::from_millis(400));
            };
            let mut queue = s.queue.clone();
            let mut history: Vec<yt::Tile> = Vec::new();
            let mut cur = first;
            let related_requests = std::sync::Arc::new(RelatedRequests::default());
            'videos: loop {
                cattube_core::watched::record(&player::data_home(), &cur);
                if let Some(on_video) = &s.on_video {
                    on_video(cur.clone());
                }
                related_requests.begin();
                let _ = conn.command(&[
                    Arg::Str("script-message"),
                    Arg::Str("cattube-video"),
                    Arg::Str(&cur.id),
                ]);
                // Network jobs never block Back, stop or the control bar.
                let mut next_id = if s.autoplay == crate::prefs::Autoplay::Next {
                    queue.first().cloned()
                } else {
                    None
                };
                let mut next_pending =
                    s.autoplay == crate::prefs::Autoplay::Next && next_id.is_none();
                let (next_tx, next_rx) = std::sync::mpsc::channel();
                if next_pending {
                    let (ctx, refresh, id) = (s.ctx.clone(), s.refresh.clone(), cur.id.clone());
                    std::thread::spawn(move || {
                        let next =
                            details(&ctx, &id, refresh.as_deref()).ok().and_then(|d| {
                                let id = d.autoplay?;
                                Some(d.related.into_iter().find(|t| t.id == id).unwrap_or(
                                    yt::Tile {
                                        thumb: yt::video_thumb(&id),
                                        id,
                                        title: "Next video".into(),
                                        ..Default::default()
                                    },
                                ))
                            });
                        let _ = next_tx.send(next);
                    });
                }
                episode_controls(
                    &mut conn,
                    !history.is_empty(),
                    next_id.is_some(),
                    next_pending,
                );
                let _ = conn.command(&[
                    Arg::Str("set_property"),
                    Arg::Str("loop-file"),
                    Arg::Str(if s.autoplay == crate::prefs::Autoplay::Loop {
                        "inf"
                    } else {
                        "no"
                    }),
                ]);
                // Live chat: offered on the bar when this is a stream with one.
                let chat_on = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                let chat_token: std::sync::Arc<Mutex<Option<String>>> = Default::default();
                let _ = conn.command(&[
                    Arg::Str("script-message"),
                    Arg::Str("cattube-chat-available"),
                    Arg::Str("no"),
                ]);
                // yt-dlp marks streams in the file's metadata.
                let is_live = conn
                    .command(&[
                        Arg::Str("get_property"),
                        Arg::Str("metadata/by-key/ytdl_is_live"),
                    ])
                    .ok()
                    .and_then(|j| j.as_str().map(|s| s == "true"))
                    .unwrap_or(false);
                if is_live || cur.kind == yt::Kind::Live {
                    let (id, region, slot) = (cur.id.clone(), s.region.clone(), chat_token.clone());
                    std::thread::spawn(move || {
                        if let Some(t) = live_chat_token(&id, &region) {
                            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(t);
                            let _ = mpv::ipc_request(
                                &player::player_socket(),
                                &[
                                    Arg::Str("script-message"),
                                    Arg::Str("cattube-chat-available"),
                                    Arg::Str("yes"),
                                ],
                            );
                        }
                    });
                }
                // The chat stops with the video.
                let stop_chat = || chat_on.store(false, std::sync::atomic::Ordering::SeqCst);
                let (segments_tx, segments_rx) = std::sync::mpsc::channel();
                if let Some(url) = s
                    .sponsorblock
                    .as_ref()
                    .and_then(|cats| community::segments_url(&cur.id, cats))
                {
                    std::thread::spawn(move || {
                        let segments = curl(&url, None, None)
                            .ok()
                            .map(|b| community::parse_segments(&b))
                            .unwrap_or_default();
                        let _ = segments_tx.send(segments);
                    });
                }
                let mut segments: Vec<community::Segment> = Vec::new();
                let _ = conn.command(&[
                    Arg::Str("script-message"),
                    Arg::Str("cattube-segments"),
                    Arg::Str(""),
                ]);
                let mut skipped: Vec<f64> = Vec::new();
                let mut last = (0.0f64, None::<f64>);
                let mut card_until: Option<Instant> = None;
                // A skip that runs to the end (end cards, outros) ends the
                // video here: seeking past the real end can leave mpv
                // paused without reporting it.
                let mut skipped_to_end = false;
                let mut thumb: Option<CardThumbnail> = None;
                let width = conn.get_f64("osd-width").unwrap_or(1920.0);
                let mut thumb_job = next_id
                    .as_ref()
                    .map(|n| fetch_card_thumbnail(s.cache.clone(), n.id.clone(), width));
                loop {
                    if let Ok(next) = next_rx.try_recv() {
                        next_id = next;
                        next_pending = false;
                        thumb_job = next_id
                            .as_ref()
                            .map(|n| fetch_card_thumbnail(s.cache.clone(), n.id.clone(), width));
                        episode_controls(&mut conn, !history.is_empty(), next_id.is_some(), false);
                    }
                    if let Ok(found) = segments_rx.try_recv() {
                        segments = found;
                        let marks = segments
                            .iter()
                            .map(|x| format!("{:.1}-{:.1}", x.start, x.end))
                            .collect::<Vec<_>>()
                            .join(",");
                        let _ = conn.command(&[
                            Arg::Str("script-message"),
                            Arg::Str("cattube-segments"),
                            Arg::Str(&marks),
                        ]);
                    }
                    if let Some(job) = &thumb_job
                        && let Ok(pic) = job.try_recv()
                    {
                        thumb = pic;
                        thumb_job = None;
                        if card_until.is_some()
                            && let Some(pic) = &thumb
                        {
                            let (p, w, h) = &pic.0;
                            card(&mut conn, &["thumb", p, &w.to_string(), &h.to_string()]);
                        }
                    }
                    let msgs = conn.messages(Duration::from_millis(400));
                    if conn.closed() {
                        ended.positions.push((cur.id.clone(), last.0, last.1));
                        stop_chat();
                        break 'videos;
                    }
                    let pos = conn.get_f64("time-pos");
                    let dur = conn.get_f64("duration").filter(|d| *d > 0.0);
                    if let Some(p) = pos {
                        last = (p, dur);
                    }
                    if let Some(sp) = conn.get_f64("speed") {
                        ended.speed = Some(sp);
                    }
                    // SponsorBlock: jump over the segment once.
                    if let Some(p) = pos
                        && let Some((to, cat)) = community::skip_at(&segments, p, dur, &skipped)
                    {
                        if let Some(seg) = segments.iter().find(|x| p >= x.start && p < x.end) {
                            skipped.push(seg.start);
                        }
                        let looping = s.autoplay == crate::prefs::Autoplay::Loop;
                        if looping && dur.is_some_and(|d| to >= d - 1.0) {
                            skipped.clear();
                            let _ = conn.command(&[
                                Arg::Str("seek"),
                                Arg::Num(0.0),
                                Arg::Str("absolute"),
                            ]);
                        } else if dur.is_some_and(|d| to >= d - 1.0) {
                            skipped_to_end = true;
                            let _ = conn.command(&[
                                Arg::Str("set_property"),
                                Arg::Str("pause"),
                                Arg::Str("yes"),
                            ]);
                        } else {
                            let _ = conn.command(&[
                                Arg::Str("seek"),
                                Arg::Num(to),
                                Arg::Str("absolute"),
                            ]);
                        }
                        osd(
                            &mut conn,
                            &format!("Skipped: {}", community::category_label(&cat)),
                        );
                    }
                    let want = |name: &str| {
                        msgs.iter()
                            .filter(|m| m.first().map(String::as_str) == Some(name))
                            .filter_map(|m| m.get(1).cloned())
                            .next_back()
                    };
                    // Back with the controls hidden: CatTube's list comes
                    // over the video, which plays on. Raised from here: the
                    // list's window, hidden behind the video, handles
                    // nothing until it shows. Stop it if that fails.
                    if msgs
                        .iter()
                        .any(|m| m.first().map(String::as_str) == Some("cattube-hide"))
                    {
                        if raise_window(APP_WINDOW) {
                            if let Some(f) = &s.on_hide {
                                f(cur.clone());
                            }
                        } else {
                            let _ = conn.command(&[Arg::Str("quit")]);
                        }
                    }
                    // Quality from the control bar: same video, same place.
                    if let Some(h) = want("cattube-quality").and_then(|h| h.parse::<u32>().ok()) {
                        let fmt = player::format_string(h, s.codec, s.hdr);
                        let _ = conn.command(&[
                            Arg::Str("set_property"),
                            Arg::Str("ytdl-format"),
                            Arg::Str(&fmt),
                        ]);
                        let opts = format!("start={:.3}", last.0);
                        let _ = conn.command(&[
                            Arg::Str("loadfile"),
                            Arg::Str(&cur.watch_url()),
                            Arg::Str("replace"),
                            Arg::Num(-1.0),
                            Arg::Str(&opts),
                        ]);
                        osd(
                            &mut conn,
                            &format!("Quality: {}", player::resolution_label(h)),
                        );
                    }
                    match want("cattube-chat").as_deref() {
                        Some("on") => {
                            let t = chat_token.lock().unwrap_or_else(|e| e.into_inner()).clone();
                            if let Some(t) = t
                                && !chat_on.swap(true, std::sync::atomic::Ordering::SeqCst)
                            {
                                follow_chat(t, s.region.clone(), chat_on.clone());
                            }
                        }
                        Some("off") => chat_on.store(false, std::sync::atomic::Ordering::SeqCst),
                        _ => {}
                    }
                    // Comments for the control bar's panel, fetched aside
                    // so skipping and the bar stay on time.
                    if let Some(arg) = want("cattube-comments") {
                        let token_ = if arg == "first" { String::new() } else { arg };
                        let (ctx, refresh, id) = (s.ctx.clone(), s.refresh.clone(), cur.id.clone());
                        std::thread::spawn(move || {
                            let json =
                                comments_json(comments(&ctx, &id, &token_, refresh.as_deref()));
                            let _ = mpv::ipc_request(
                                &player::player_socket(),
                                &[
                                    Arg::Str("script-message"),
                                    Arg::Str("cattube-comments-data"),
                                    Arg::Str(&json),
                                ],
                            );
                        });
                    }
                    // A new request supersedes prior work, including replies from
                    // the previous video. The Lua browser preserves its focus as
                    // each thumbnail arrives.
                    if let Some(request) = msgs
                        .iter()
                        .rev()
                        .find(|m| m.first().map(String::as_str) == Some("cattube-related"))
                        .and_then(|m| m.get(1))
                        .filter(|s| {
                            s.len() <= 20 && !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
                        })
                        .cloned()
                    {
                        let id = cur.id.clone();
                        let rs = Session {
                            ctx: s.ctx.clone(),
                            refresh: s.refresh.clone(),
                            cache: s.cache.clone(),
                            autoplay: s.autoplay,
                            sponsorblock: None,
                            codec: s.codec,
                            hdr: s.hdr,
                            region: s.region.clone(),
                            queue: Vec::new(),
                            on_hide: None,
                            on_video: None,
                        };
                        let requests = related_requests.clone();
                        let expected = requests.begin();
                        std::thread::spawn(move || {
                            send_related(&rs, &id, &request, &requests, expected)
                        });
                    }
                    // A related video picked: it plays next, right away.
                    let picked = msgs
                        .iter()
                        .filter(|m| m.first().map(String::as_str) == Some("cattube-related-pick"))
                        .filter_map(|m| {
                            let id = m.get(1)?;
                            yt::is_video_id(id).then(|| yt::Tile {
                                id: id.clone(),
                                title: m.get(2).cloned().unwrap_or_default(),
                                byline: m.get(3).cloned().unwrap_or_default(),
                                thumb: yt::video_thumb(id),
                                ..yt::Tile::default()
                            })
                        })
                        .next_back();
                    let jump = match want("cattube-episode").as_deref() {
                        Some("next") => next_id.clone(),
                        Some("prev") => history.pop(),
                        _ => None,
                    }
                    .or(picked);
                    let eof = conn.get_bool("eof-reached").unwrap_or(false) || skipped_to_end;
                    // The end: "Up next" counts down five seconds.
                    if eof && !next_pending && card_until.is_none() && jump.is_none() {
                        match &next_id {
                            Some(n) => {
                                let mut args: Vec<String> =
                                    vec!["next".into(), n.title.clone(), "5".into()];
                                if let Some(pic) = &thumb {
                                    let (p, w, h) = &pic.0;
                                    args.extend([p.clone(), w.to_string(), h.to_string()]);
                                }
                                let a: Vec<&str> = args.iter().map(String::as_str).collect();
                                card(&mut conn, &a);
                                card_until = Some(Instant::now() + Duration::from_secs(5));
                            }
                            None => {
                                let _ = conn.command(&[Arg::Str("quit")]);
                            }
                        }
                    }
                    let result = want("cattube-card-result");
                    if let Some(until) = card_until {
                        let left = until.saturating_duration_since(Instant::now()).as_secs();
                        card(&mut conn, &["count", &left.to_string()]);
                        if result.as_deref() == Some("cancel") || result.as_deref() == Some("stop")
                        {
                            card(&mut conn, &["hide"]);
                            let _ = conn.command(&[Arg::Str("quit")]);
                            card_until = None;
                        }
                    }
                    let go_next = jump.clone().or_else(|| {
                        card_until
                            .filter(|u| Instant::now() >= *u || result.as_deref() == Some("play"))
                            .and(next_id.clone())
                    });
                    if let Some(n) = go_next {
                        card(&mut conn, &["hide"]);
                        ended.positions.push((cur.id.clone(), last.0, last.1));
                        stop_chat();
                        let backwards = want("cattube-episode").as_deref() == Some("prev");
                        if !backwards {
                            history.push(cur.clone());
                            if queue.first().is_some_and(|q| q.id == n.id) {
                                queue.remove(0);
                            }
                        }
                        let title: String = n
                            .title
                            .chars()
                            .filter(|c| !c.is_control())
                            .take(120)
                            .collect();
                        let opts = format!("force-media-title={}", mpv::option_value(&title));
                        let _ = conn.command(&[
                            Arg::Str("loadfile"),
                            Arg::Str(&n.watch_url()),
                            Arg::Str("replace"),
                            Arg::Num(-1.0),
                            Arg::Str(&opts),
                        ]);
                        // The end left it paused on the last frame (for the card).
                        let _ = conn.command(&[
                            Arg::Str("set_property"),
                            Arg::Str("pause"),
                            Arg::Str("no"),
                        ]);
                        cur = n;
                        // Wait for it to load before watching it.
                        let t0 = Instant::now();
                        while conn.get_f64("time-pos").is_none()
                            && t0.elapsed() < Duration::from_secs(45)
                        {
                            if conn.closed() {
                                break 'videos;
                            }
                            std::thread::sleep(Duration::from_millis(300));
                        }
                        continue 'videos;
                    }
                }
            }
            related_requests.begin();
            done(ended);
        });
}
