//! CatTube's settings (`settings.conf` in the config folder): the video
//! preset (highest resolution, codec, HDR), speed, subtitles, what plays
//! after a video, SponsorBlock, and the accent colour (one of the names in
//! `theme::ACCENTS`).

use cattube_core::community;
use cattube_core::player::{Codec, RESOLUTIONS, SPEEDS};
use std::path::{Path, PathBuf};

/// What happens when a video ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Autoplay {
    /// The next one YouTube suggests (or the next in a playlist).
    #[default]
    Next,
    /// Play it again.
    Loop,
    /// Stop.
    Off,
}

impl Autoplay {
    pub const ALL: [Autoplay; 3] = [Autoplay::Next, Autoplay::Loop, Autoplay::Off];

    pub fn key(self) -> &'static str {
        match self {
            Self::Next => "next",
            Self::Loop => "loop",
            Self::Off => "off",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Next => "Play the next video",
            Self::Loop => "Play it again",
            Self::Off => "Stop",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Prefs {
    pub accent: String,
    /// Highest resolution (0 = best there is).
    pub max_height: u32,
    pub codec: Codec,
    pub hdr: bool,
    pub speed: f64,
    pub subs_on: bool,
    pub sub_lang: String,
    pub autoplay: Autoplay,
    pub sponsorblock: bool,
    pub sb_categories: Vec<String>,
    /// Load-ahead in MiB (`player::BUFFERS`).
    pub buffer: u32,
    /// Signed in: what plays goes into the account's YouTube history.
    pub history: bool,
    /// Region for YouTube's lists ("IN", "US").
    pub region: String,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            accent: "red".into(),
            max_height: 2160,
            codec: Codec::Vp9,
            hdr: true,
            speed: 1.0,
            subs_on: false,
            sub_lang: "en".into(),
            autoplay: Autoplay::Next,
            sponsorblock: true,
            sb_categories: community::default_categories(),
            history: true,
            buffer: 256,
            region: "US".into(),
        }
    }
}

impl Prefs {
    pub fn parse(text: &str) -> Self {
        let mut p = Prefs::default();
        let mut cats: Option<Vec<String>> = None;
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let v = v.trim();
            match k.trim() {
                "accent" if crate::theme::ACCENTS.iter().any(|(n, _)| *n == v) => {
                    p.accent = v.into()
                }
                "max_height" => {
                    if let Ok(n) = v.parse::<u32>()
                        && RESOLUTIONS.contains(&n)
                    {
                        p.max_height = n;
                    }
                }
                "codec" => p.codec = Codec::from_key(v),
                "hdr" => p.hdr = v != "false",
                "speed" => {
                    if let Ok(s) = v.parse::<f64>()
                        && SPEEDS.iter().any(|x| (x - s).abs() < 1e-6)
                    {
                        p.speed = s;
                    }
                }
                "subtitles" => p.subs_on = v == "true",
                "subtitle_language"
                    if !v.is_empty()
                        && v.len() <= 8
                        && v.bytes().all(|b| b.is_ascii_alphabetic() || b == b'-') =>
                {
                    p.sub_lang = v.to_string()
                }
                "autoplay" => {
                    if let Some(a) = Autoplay::ALL.into_iter().find(|a| a.key() == v) {
                        p.autoplay = a;
                    }
                }
                "sponsorblock" => p.sponsorblock = v != "false",
                "history" => p.history = v != "false",
                "buffer" => {
                    if let Ok(n) = v.parse::<u32>()
                        && cattube_core::player::BUFFERS.contains(&n)
                    {
                        p.buffer = n;
                    }
                }
                "sponsorblock_categories" => {
                    cats = Some(
                        v.split(',')
                            .map(str::trim)
                            .filter(|c| community::CATEGORIES.iter().any(|(k, _, _)| k == c))
                            .map(str::to_string)
                            .collect(),
                    )
                }
                "region" if v.len() == 2 && v.bytes().all(|b| b.is_ascii_uppercase()) => {
                    p.region = v.to_string()
                }
                _ => {}
            }
        }
        if let Some(c) = cats {
            p.sb_categories = c;
        }
        p
    }

    pub fn to_conf(&self) -> String {
        format!(
            "# CatTube settings\naccent = {}\nmax_height = {}\ncodec = {}\nhdr = {}\nspeed = {}\nsubtitles = {}\nsubtitle_language = {}\nautoplay = {}\nsponsorblock = {}\nsponsorblock_categories = {}\nhistory = {}\nbuffer = {}\nregion = {}\n",
            self.accent,
            self.max_height,
            self.codec.key(),
            self.hdr,
            self.speed,
            self.subs_on,
            self.sub_lang,
            self.autoplay.key(),
            self.sponsorblock,
            self.sb_categories.join(","),
            self.history,
            self.buffer,
            self.region
        )
    }

    pub fn skips(&self, category: &str) -> bool {
        self.sb_categories.iter().any(|c| c == category)
    }
}

pub fn path(config: &Path) -> PathBuf {
    config.join("settings.conf")
}

pub fn load(config: &Path) -> Prefs {
    std::fs::read_to_string(path(config))
        .map(|t| Prefs::parse(&t))
        .unwrap_or_default()
}

pub fn save(config: &Path, p: &Prefs) -> std::io::Result<()> {
    std::fs::create_dir_all(config)?;
    std::fs::write(path(config), p.to_conf())
}

/// The next value in `list` in `delta`'s direction, wrapping.
pub fn cycle<T: PartialEq + Copy>(list: &[T], cur: T, delta: i32) -> T {
    let n = list.len() as i32;
    let i = list.iter().position(|x| *x == cur).unwrap_or(0) as i32;
    list[(i + delta).rem_euclid(n) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let p = Prefs {
            accent: "teal".into(),
            max_height: 1080,
            codec: Codec::Av1,
            hdr: false,
            speed: 1.5,
            subs_on: true,
            sub_lang: "de".into(),
            autoplay: Autoplay::Loop,
            sponsorblock: false,
            sb_categories: vec!["sponsor".into(), "intro".into()],
            history: false,
            buffer: 768,
            region: "DE".into(),
        };
        assert_eq!(Prefs::parse(&p.to_conf()), p);
        let junk = Prefs::parse(
            "max_height = 999\nspeed = 7\ncodec = xyz\nsponsorblock_categories = sponsor,evil\n",
        );
        assert_eq!(
            (junk.max_height, junk.speed, junk.codec),
            (2160, 1.0, Codec::Vp9)
        );
        assert_eq!(junk.sb_categories, vec!["sponsor".to_string()]);
        assert_eq!(cycle(&SPEEDS, 1.0, 1), 1.25);
        assert_eq!(cycle(&RESOLUTIONS, 480, 1), 0);
    }
}
