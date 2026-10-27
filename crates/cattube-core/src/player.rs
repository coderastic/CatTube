//! Playing a video: mpv with yt-dlp finding the streams.
//!
//! YouTube changes often, and a yt-dlp from a distribution goes stale
//! within weeks, so CatTube keeps its own copy (the official single-file
//! release, run with the system's Python) up to date in its data folder.
//! Full quality (above 360p) needs YouTube's
//! script challenges solved, which yt-dlp does with a JavaScript runtime:
//! the system's Deno or Node if there is one, else CatTube's own Deno.
//!
//! The quality choice: a highest resolution, a preferred codec (AV1, VP9 or AVC) and HDR on or off;
//! yt-dlp falls back to the next best format when that one is missing.

use crate::process::Cmd;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const YT_DLP_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp";
pub const DENO_URL: &str =
    "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-unknown-linux-gnu.zip";
/// How often CatTube asks yt-dlp to update itself.
pub const UPDATE_EVERY: Duration = Duration::from_secs(24 * 3600);

/// `~/.local/share/cattube`: yt-dlp, Deno, positions.
pub fn data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/tmp".into()))
                .join(".local/share")
        })
        .join("cattube")
}

pub fn yt_dlp_path(data: &Path) -> PathBuf {
    data.join("yt-dlp")
}

pub fn own_deno_path(data: &Path) -> PathBuf {
    data.join("deno")
}

/// `curl` saving `url` to `out` (follows GitHub's redirects).
pub fn download_cmd(url: &str, out: &Path) -> Cmd {
    Cmd::new("curl")
        .args([
            "-sSL".to_string(),
            "--fail".to_string(),
            "-m".to_string(),
            "300".to_string(),
            "-o".to_string(),
            out.display().to_string(),
            "--".to_string(),
            url.to_string(),
        ])
        .timeout(Duration::from_secs(320))
}

/// Unpack Deno's zip with the system's Python (yt-dlp needs Python anyway).
pub fn unzip_cmd(zip: &Path, dir: &Path) -> Cmd {
    Cmd::new("python3")
        .args([
            "-I".to_string(),
            "-c".to_string(),
            "import sys,zipfile;zipfile.ZipFile(sys.argv[1]).extract('deno',sys.argv[2])"
                .to_string(),
            zip.display().to_string(),
            dir.display().to_string(),
        ])
        .timeout(Duration::from_secs(120))
}

pub fn update_cmd(yt_dlp: &Path) -> Cmd {
    Cmd::new(yt_dlp.display().to_string())
        .args(["-U", "--no-colors"])
        .timeout(Duration::from_secs(180))
}

/// The JavaScript runtime yt-dlp is given: `deno:/path` (or `node:`).
pub fn js_runtime(data: &Path, find: &dyn Fn(&str) -> Option<PathBuf>) -> Option<String> {
    if let Some(p) = find("deno") {
        return Some(format!("deno:{}", p.display()));
    }
    let own = own_deno_path(data);
    if own.is_file() {
        return Some(format!("deno:{}", own.display()));
    }
    find("node").map(|p| format!("node:{}", p.display()))
}

// --------------------------------------------------------------- quality

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Codec {
    /// Best on newer graphics (Intel 11th gen+, AMD RDNA2+).
    Av1,
    /// Plays well on most TVs and boxes.
    #[default]
    Vp9,
    /// For old or slow hardware.
    Avc,
}

impl Codec {
    pub const ALL: [Codec; 3] = [Codec::Av1, Codec::Vp9, Codec::Avc];

    pub fn key(self) -> &'static str {
        match self {
            Self::Av1 => "av1",
            Self::Vp9 => "vp9",
            Self::Avc => "avc",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Av1 => "AV1",
            Self::Vp9 => "VP9",
            Self::Avc => "AVC (H.264)",
        }
    }

    pub fn from_key(s: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|c| c.key() == s)
            .unwrap_or_default()
    }

    fn vcodec_prefix(self) -> &'static str {
        match self {
            Self::Av1 => "av01",
            Self::Vp9 => "vp09",
            Self::Avc => "avc1",
        }
    }
}

/// The highest resolutions Settings offers (0 = no limit).
pub const RESOLUTIONS: [u32; 7] = [0, 4320, 2160, 1440, 1080, 720, 480];

pub fn resolution_label(h: u32) -> String {
    match h {
        0 => "Best".into(),
        4320 => "8K".into(),
        2160 => "4K".into(),
        h => format!("{h}p"),
    }
}

/// yt-dlp's format choice: the preferred codec at most `max_h` high, then
/// any codec at that height, then anything. `hdr = false` keeps to SDR.
pub fn format_string(max_h: u32, codec: Codec, hdr: bool) -> String {
    let h = if max_h == 0 {
        String::new()
    } else {
        format!("[height<={max_h}]")
    };
    let sdr = if hdr { "" } else { "[dynamic_range=SDR]" };
    let p = codec.vcodec_prefix();
    // VP9 also shows up as plain "vp9".
    let alt = if codec == Codec::Vp9 {
        format!("/bv*{h}{sdr}[vcodec=vp9]+ba")
    } else {
        String::new()
    };
    format!("bv*{h}{sdr}[vcodec^={p}]+ba{alt}/bv*{h}{sdr}+ba/bv*{h}+ba/b{h}/b")
}

/// Playback speeds the player offers.
/// Buffer sizes offered (MiB): small for little memory, large for shaky
/// connections.
pub const BUFFERS: [u32; 3] = [64, 256, 768];

/// Countries for recommendations (YouTube's region codes).
pub const REGIONS: [&str; 16] = [
    "US", "GB", "IN", "CA", "AU", "DE", "FR", "ES", "IT", "NL", "BR", "MX", "JP", "KR", "PL", "TR",
];

pub const SPEEDS: [f64; 9] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 3.0];

// ------------------------------------------------------------------ mpv

/// Everything that shapes one playback.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayOptions {
    /// How much to load ahead, in MiB (the Buffer setting).
    pub buffer_mib: u32,
    pub video_id: String,
    pub title: String,
    pub start: f64,
    pub speed: f64,
    pub max_height: u32,
    pub codec: Codec,
    pub hdr: bool,
    /// Subtitle languages to fetch ("en"); empty = none.
    pub sub_langs: String,
    pub subs_on: bool,
    /// `RRGGBB` for the control bar.
    pub accent: String,
    /// Back hides the video (it plays on behind CatTube's list) instead
    /// of stopping it: on CatosTV, which can bring it back.
    pub hide_on_back: bool,
}

/// mpv's command line for a video. `data` holds yt-dlp, `ui` the control
/// bar script and key bindings, `js` the runtime for yt-dlp.
pub fn mpv_argv(
    o: &PlayOptions,
    data: &Path,
    ui: &Path,
    socket: &Path,
    js: Option<&str>,
) -> Vec<String> {
    let mut v: Vec<String> = [
        "mpv",
        "--fullscreen",
        "--force-window=immediate",
        "--idle=no",
        "--keep-open=no",
        // CatTube keeps its own places; a system mpv.conf that saves and
        // restores them (volume too) would fight the start time and the
        // phone remote's volume.
        "--save-position-on-quit=no",
        "--resume-playback=no",
        "--no-terminal",
        "--wayland-app-id=cattube-player",
        "--osc=no",
        "--cache=yes",
        // YouTube's video addresses only work a few seconds after they are
        // handed out (yt-dlp's "available_at"); until then they answer
        // 403. Ask again for up to 12 s instead of giving up.
        "--stream-lavf-o=reconnect=1,reconnect_on_http_error=403,reconnect_delay_max=12",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    v.push(format!(
        "--demuxer-max-bytes={}MiB",
        o.buffer_mib.clamp(16, 1024)
    ));
    v.push(format!(
        "--demuxer-max-back-bytes={}MiB",
        (o.buffer_mib / 4).clamp(8, 256)
    ));
    v.push(format!("--input-ipc-server={}", socket.display()));
    v.push(format!(
        "--input-conf={}",
        ui.join("cattube-input.conf").display()
    ));
    v.push(format!("--script={}", ui.join("cattube-osd.lua").display()));
    let accent: String = o
        .accent
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(6)
        .collect();
    let mut opts = vec![
        format!("ytdl_hook-ytdl_path={}", yt_dlp_path(data).display()),
        "ytdl_hook-try_ytdl_first=yes".to_string(),
    ];
    if accent.len() == 6 {
        opts.push(format!("cattube_osd-accent={accent}"));
    }
    if o.hide_on_back {
        opts.push("cattube_osd-hide=yes".to_string());
    }
    opts.push(format!(
        "cattube_osd-position_dir={}",
        data.join("positions").display()
    ));
    v.push(format!("--script-opts={}", opts.join(",")));
    v.push(format!(
        "--ytdl-format={}",
        format_string(o.max_height, o.codec, o.hdr)
    ));
    let mut raw: Vec<String> = Vec::new();
    if let Some(js) = js {
        raw.push(format!("js-runtimes={js}"));
    }
    let langs: String = o
        .sub_langs
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(16)
        .collect();
    if !langs.is_empty() {
        // The language and its regional forms (en-GB), not YouTube's
        // machine translations into it (en-de, "English from German").
        // yt-dlp ignores case, so the region part is matched with (?-i).
        raw.push(format!(
            "sub-langs=\"{langs},{langs}-(?-i:[A-Z]).*,{langs}-orig\""
        ));
        raw.push("write-subs=".into());
        raw.push("write-auto-subs=".into());
    }
    if !raw.is_empty() {
        v.push(format!("--ytdl-raw-options={}", raw.join(",")));
    }
    v.push(if o.subs_on {
        "--sid=auto".into()
    } else {
        "--sid=no".into()
    });
    v.push(format!("--speed={}", o.speed.clamp(0.25, 4.0)));
    let title: String = o
        .title
        .chars()
        .filter(|c| !c.is_control())
        .take(120)
        .collect();
    // No title known: yt-dlp's (the video's own) shows.
    if !title.trim().is_empty() {
        v.push(format!("--force-media-title={title}"));
    }
    if o.start.is_finite() && o.start > 0.0 {
        v.push(format!("--start={:.3}", o.start));
    }
    v.push("--osd-duration=1500".into());
    v.push("--osd-bar-align-y=0.86".into());
    v.push("--".into());
    v.push(crate::youtube::watch_url(&o.video_id));
    v
}

pub fn player_socket() -> PathBuf {
    crate::env::runtime_dir().join("cattube-mpv.sock")
}

// ------------------------------------------------------------- positions

/// Where each video was left (seconds), for resuming: `positions.conf`,
/// newest last, at most `MAX_POSITIONS`.
pub const MAX_POSITIONS: usize = 500;

pub fn parse_positions(text: &str) -> Vec<(String, f64)> {
    text.lines()
        .filter_map(|l| {
            let (id, s) = l.trim().split_once(' ')?;
            let s: f64 = s.trim().parse().ok()?;
            (id.len() == 11 && s.is_finite() && s >= 0.0).then(|| (id.to_string(), s))
        })
        .collect()
}

/// Preserve partial playback, including short videos and their final seconds.
pub fn remember(list: &mut Vec<(String, f64)>, id: &str, pos: f64, _duration: Option<f64>) {
    list.retain(|(x, _)| x != id);
    if pos.is_finite() && pos > 0.0 {
        list.push((id.to_string(), pos));
    }
    let n = list.len();
    if n > MAX_POSITIONS {
        list.drain(..n - MAX_POSITIONS);
    }
}

pub fn positions_text(list: &[(String, f64)]) -> String {
    list.iter()
        .map(|(id, s)| format!("{id} {s:.3}\n"))
        .collect()
}

pub fn position_clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Written atomically by the player itself, including on pause, seek and exit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Checkpoint {
    pub position: f64,
    pub duration: Option<f64>,
    pub finished: bool,
}

pub fn store_checkpoint(data: &Path, id: &str, point: Checkpoint) -> std::io::Result<()> {
    if !crate::youtube::is_video_id(id) || !point.position.is_finite() || point.position < 0.0 {
        return Err(std::io::Error::other("invalid playback position"));
    }
    let folder = data.join("positions");
    std::fs::create_dir_all(&folder)?;
    let path = folder.join(format!("{id}.pos"));
    let tmp = folder.join(format!("{id}.{}.tmp", std::process::id()));
    std::fs::write(
        &tmp,
        format!(
            "{:.3} {:.3} {}\n",
            point.position,
            point.duration.unwrap_or(0.0),
            u8::from(point.finished)
        ),
    )?;
    std::fs::rename(tmp, path)
}

pub fn checkpoint(data: &Path, id: &str) -> Option<Checkpoint> {
    if !crate::youtube::is_video_id(id) {
        return None;
    }
    let text = std::fs::read_to_string(data.join("positions").join(format!("{id}.pos"))).ok()?;
    let mut fields = text.split_whitespace();
    let position: f64 = fields.next()?.parse().ok()?;
    let duration: f64 = fields.next()?.parse().ok()?;
    let finished = match fields.next()? {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    if !position.is_finite() || position < 0.0 || !duration.is_finite() {
        return None;
    }
    Some(Checkpoint {
        position,
        duration: (duration > 0.0).then_some(duration),
        finished,
    })
}

/// Player checkpoints take precedence over the older watcher snapshots.
pub fn load_positions(data: &Path) -> Vec<(String, f64)> {
    let mut list = std::fs::read_to_string(data.join("positions.conf"))
        .map(|s| parse_positions(&s))
        .unwrap_or_default();
    let mut files: Vec<_> = std::fs::read_dir(data.join("positions"))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension()?.to_str()? != "pos" {
                return None;
            }
            let id = path.file_stem()?.to_str()?.to_string();
            if !crate::youtube::is_video_id(&id) {
                return None;
            }
            Some((entry.metadata().ok()?.modified().ok()?, id, path))
        })
        .collect();
    files.sort_by_key(|f| f.0);
    let stale = files.len().saturating_sub(MAX_POSITIONS);
    for (_, _, path) in files.drain(..stale) {
        let _ = std::fs::remove_file(path);
    }
    for (_, id, _) in files {
        if let Some(point) = checkpoint(data, &id) {
            if point.finished {
                list.retain(|(video, _)| *video != id);
            } else {
                remember(&mut list, &id, point.position, point.duration);
            }
        }
    }
    if list.len() > MAX_POSITIONS {
        list.drain(..list.len() - MAX_POSITIONS);
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(
            format_string(1080, Codec::Avc, false),
            "bv*[height<=1080][dynamic_range=SDR][vcodec^=avc1]+ba/bv*[height<=1080][dynamic_range=SDR]+ba/bv*[height<=1080]+ba/b[height<=1080]/b"
        );
        assert_eq!(
            format_string(0, Codec::Vp9, true),
            "bv*[vcodec^=vp09]+ba/bv*[vcodec=vp9]+ba/bv*+ba/bv*+ba/b/b"
        );
        assert_eq!(Codec::from_key("av1"), Codec::Av1);
        assert_eq!(Codec::from_key("?"), Codec::Vp9);
        assert_eq!(resolution_label(2160), "4K");
        assert_eq!(resolution_label(720), "720p");
    }

    #[test]
    fn mpv_command_line() {
        let o = PlayOptions {
            buffer_mib: 256,
            video_id: "WhWc3b3KhnY".into(),
            title: "Spring\n".into(),
            start: 61.4,
            speed: 1.5,
            max_height: 2160,
            codec: Codec::Vp9,
            hdr: true,
            sub_langs: "en".into(),
            subs_on: false,
            accent: "7c6cf0".into(),
            hide_on_back: false,
        };
        let v = mpv_argv(
            &o,
            Path::new("/d"),
            Path::new("/ui"),
            Path::new("/run/s.sock"),
            Some("deno:/d/deno"),
        );
        // Waits for YouTube's address to become usable instead of failing.
        assert!(v.iter().any(
            |a| a.starts_with("--stream-lavf-o=") && a.contains("reconnect_on_http_error=403")
        ));
        assert_eq!(
            v.last().unwrap(),
            "https://www.youtube.com/watch?v=WhWc3b3KhnY"
        );
        assert!(v.contains(&"--start=61.400".to_string()));
        let mut short = o.clone();
        short.start = 0.5;
        assert!(
            mpv_argv(
                &short,
                Path::new("/d"),
                Path::new("/ui"),
                Path::new("/sock"),
                None
            )
            .contains(&"--start=0.500".to_string())
        );
        assert!(v.contains(&"--resume-playback=no".to_string()));
        assert!(v.contains(&"--speed=1.5".to_string()));
        assert!(v.contains(&"--force-media-title=Spring".to_string()));
        assert!(v.contains(&"--script-opts=ytdl_hook-ytdl_path=/d/yt-dlp,ytdl_hook-try_ytdl_first=yes,cattube_osd-accent=7c6cf0,cattube_osd-position_dir=/d/positions".to_string()));
        assert!(v.contains(&"--ytdl-raw-options=js-runtimes=deno:/d/deno,sub-langs=\"en,en-(?-i:[A-Z]).*,en-orig\",write-subs=,write-auto-subs=".to_string()));
        assert!(v.contains(&"--sid=no".to_string()));
        assert!(v.contains(&"--demuxer-max-bytes=256MiB".to_string()));
        assert!(v.contains(&"--input-ipc-server=/run/s.sock".to_string()));
        // Zero starts normally; every positive saved position is passed.
        let o2 = PlayOptions {
            start: 0.0,
            sub_langs: String::new(),
            ..o
        };
        let v2 = mpv_argv(
            &o2,
            Path::new("/d"),
            Path::new("/ui"),
            Path::new("/s"),
            None,
        );
        assert!(!v2.iter().any(|a| a.starts_with("--start")));
        assert!(!v2.iter().any(|a| a.starts_with("--ytdl-raw-options")));
    }

    #[test]
    fn js_runtime_choice() {
        let none = |_: &str| None;
        assert_eq!(js_runtime(Path::new("/nope"), &none), None);
        let sys = |n: &str| (n == "node").then(|| PathBuf::from("/usr/bin/node"));
        assert_eq!(
            js_runtime(Path::new("/nope"), &sys).as_deref(),
            Some("node:/usr/bin/node")
        );
        let deno = |n: &str| (n == "deno").then(|| PathBuf::from("/usr/bin/deno"));
        assert_eq!(
            js_runtime(Path::new("/nope"), &deno).as_deref(),
            Some("deno:/usr/bin/deno")
        );
    }

    #[test]
    fn positions() {
        let mut l = parse_positions("WhWc3b3KhnY 120\nbad\nB4EPW7JUMTM 30\n");
        assert_eq!(l.len(), 2);
        remember(&mut l, "WhWc3b3KhnY", 200.0, Some(1000.0));
        assert_eq!(l.last().unwrap(), &("WhWc3b3KhnY".to_string(), 200.0));
        // Stopping near the end still keeps the unwatched final seconds.
        remember(&mut l, "B4EPW7JUMTM", 990.0, Some(1000.0));
        assert!(l.iter().any(|(i, p)| i == "B4EPW7JUMTM" && *p == 990.0));
        assert_eq!(
            positions_text(&l),
            "WhWc3b3KhnY 200.000\nB4EPW7JUMTM 990.000\n"
        );
    }
    #[test]
    fn exact_player_checkpoint_wins_over_delayed_watcher_save() {
        let root = std::env::temp_dir().join(format!(
            "cattube-positions-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("positions")).unwrap();
        std::fs::write(
            root.join("positions.conf"),
            "WhWc3b3KhnY 100\nB4EPW7JUMTM 900\n",
        )
        .unwrap();
        std::fs::write(
            root.join("positions/WhWc3b3KhnY.pos"),
            "123.456 130.000 0\n",
        )
        .unwrap();
        std::fs::write(
            root.join("positions/B4EPW7JUMTM.pos"),
            "1000.000 1000.000 1\n",
        )
        .unwrap();
        let loaded = load_positions(&root);
        assert_eq!(loaded, vec![("WhWc3b3KhnY".into(), 123.456)]);
        assert_eq!(parse_positions(&positions_text(&loaded)), loaded);
        std::fs::write(root.join("positions/WhWc3b3KhnY.pos"), "0.500 20.000 0\n").unwrap();
        assert_eq!(load_positions(&root), vec![("WhWc3b3KhnY".into(), 0.5)]);
        std::fs::write(root.join("positions/WhWc3b3KhnY.pos"), "NaN 20 0\n").unwrap();
        assert!(checkpoint(&root, "WhWc3b3KhnY").is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}
