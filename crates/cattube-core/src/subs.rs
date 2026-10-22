//! Subscriptions as one row per channel.
//!
//! YouTube's TV `FEsubscriptions` answer holds only about thirty videos, a
//! "most relevant" pick, with no way to ask for more. So the Subscriptions
//! section is built from the list of channels you follow (`FEchannels`) and
//! each channel's public feed
//! (`https://www.youtube.com/feeds/videos.xml?channel_id=UC…`, its newest
//! fifteen uploads with dates, no sign-in needed): one row per channel,
//! channels with the newest upload first, and at the end of each row the
//! channel itself, to see everything it made.
//!
//! Pure: the app fetches the feeds; this reads them and lays out the rows.

use crate::youtube::{Kind, Page, Shelf, Tile};

/// Videos shown in a channel's row (the feed has up to 15).
pub const PER_ROW: usize = 12;

/// The public feed of a channel's newest uploads.
pub fn feed_url(channel_id: &str) -> Option<String> {
    (channel_id.starts_with("UC")
        && channel_id.len() == 24
        && channel_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
    .then(|| format!("https://www.youtube.com/feeds/videos.xml?channel_id={channel_id}"))
}

/// One upload from a channel's feed.
#[derive(Debug, Clone, PartialEq)]
pub struct Upload {
    pub tile: Tile,
    /// Unix seconds.
    pub published: i64,
}

/// Read a channel's feed. `now` (Unix seconds) is for "3 days ago".
pub fn parse_feed(xml: &str, now: i64) -> Vec<Upload> {
    let mut out = Vec::new();
    let channel = between(xml, "<title>", "</title>").map(unescape);
    for entry in xml.split("<entry>").skip(1) {
        let entry = entry.split("</entry>").next().unwrap_or("");
        let Some(id) = between(entry, "<yt:videoId>", "</yt:videoId>") else {
            continue;
        };
        if id.len() != 11 {
            continue;
        }
        let channel_id = between(entry, "<yt:channelId>", "</yt:channelId>").unwrap_or("");
        let title = between(entry, "<title>", "</title>").map(unescape);
        let published = between(entry, "<published>", "</published>")
            .and_then(parse_time)
            .unwrap_or(0);
        let views = attr(entry, "<media:statistics", "views").and_then(|v| v.parse::<u64>().ok());
        let short =
            attr(entry, "<link rel=\"alternate\"", "href").is_some_and(|h| h.contains("/shorts/"));
        let ago = if published > 0 {
            ago(now - published)
        } else {
            String::new()
        };
        let meta = match views {
            Some(v) if v > 0 => format!("{} • {ago}", views_text(v)),
            _ => ago,
        };
        out.push(Upload {
            tile: Tile {
                kind: if short { Kind::Short } else { Kind::Video },
                id: id.to_string(),
                title: title.unwrap_or_default(),
                byline: channel.clone().unwrap_or_default(),
                channel_id: channel_id.to_string(),
                meta,
                badge: String::new(),
                thumb: format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg"),
                progress: None,
            },
            published,
        });
    }
    out
}

/// The Subscriptions page: a row per channel (`channels` from
/// `FEchannels`, `uploads[i]` that channel's feed), the channel with the
/// newest upload first, Shorts left out. Each row ends with the channel's own tile ("View
/// all"). Channels whose feed gave nothing come last, with that tile only.
pub fn rows(channels: &[Tile], uploads: &[Vec<Upload>]) -> Page {
    let newest = |i: usize| {
        uploads.get(i).and_then(|u| {
            u.iter()
                .filter(|x| x.tile.kind != Kind::Short)
                .map(|x| x.published)
                .max()
        })
    };
    let mut order: Vec<usize> = (0..channels.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(newest(i).unwrap_or(i64::MIN)));
    let shelves = order
        .into_iter()
        .map(|i| {
            let ch = &channels[i];
            let mut list: Vec<&Upload> = uploads
                .get(i)
                .map(|u| u.iter().filter(|x| x.tile.kind != Kind::Short).collect())
                .unwrap_or_default();
            list.sort_by_key(|u| std::cmp::Reverse(u.published));
            let mut tiles: Vec<Tile> = list
                .into_iter()
                .take(PER_ROW)
                .map(|u| u.tile.clone())
                .collect();
            tiles.push(Tile {
                kind: Kind::Channel,
                id: ch.id.clone(),
                title: "View all".into(),
                byline: String::new(),
                channel_id: ch.id.clone(),
                meta: ch.title.clone(),
                badge: String::new(),
                thumb: ch.thumb.clone(),
                progress: None,
            });
            Shelf {
                title: ch.title.clone(),
                tiles,
                continuation: None,
                more: Some(ch.id.clone()),
            }
        })
        .collect();
    Page {
        title: "Subscriptions".into(),
        shelves,
        continuation: None,
    }
}

/// The newest uploads of all channels together (for a TV home screen).
pub fn newest(uploads: &[Vec<Upload>], n: usize) -> Vec<Tile> {
    let mut all: Vec<&Upload> = uploads
        .iter()
        .flatten()
        .filter(|u| u.tile.kind != Kind::Short)
        .collect();
    all.sort_by_key(|u| std::cmp::Reverse(u.published));
    all.into_iter().take(n).map(|u| u.tile.clone()).collect()
}

fn between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let a = s.find(open)? + open.len();
    let b = s[a..].find(close)?;
    Some(&s[a..a + b])
}

/// `name="value"` in the first tag starting with `tag`.
fn attr<'a>(s: &'a str, tag: &str, name: &str) -> Option<&'a str> {
    let t = &s[s.find(tag)?..];
    let t = &t[..t.find('>')?];
    let key = format!("{name}=\"");
    let a = t.find(&key)? + key.len();
    Some(&t[a..a + t[a..].find('"')?])
}

fn unescape(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// "2026-10-22T19:14:45+00:00" → Unix seconds.
fn parse_time(s: &str) -> Option<i64> {
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (h, mi, se) = (n(11..13)?, n(14..16)?, n(17..19)?);
    // Offset "+05:30" / "-04:00" / "Z".
    let off = match s.get(19..20) {
        Some(sign @ ("+" | "-")) => {
            let m = n(20..22)? * 60 + n(23..25)?;
            if sign == "+" { m * 60 } else { -m * 60 }
        }
        _ => 0,
    };
    // Days from 1970-01-01 (Howard Hinnant's days_from_civil).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (mo + if mo > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + se - off)
}

fn ago(secs: i64) -> String {
    let secs = secs.max(0);
    let (n, unit) = match secs {
        s if s < 3600 => ((s / 60).max(1), "minute"),
        s if s < 86_400 => (s / 3600, "hour"),
        s if s < 7 * 86_400 => (s / 86_400, "day"),
        s if s < 30 * 86_400 => (s / (7 * 86_400), "week"),
        s if s < 365 * 86_400 => (s / (30 * 86_400), "month"),
        s => (s / (365 * 86_400), "year"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

fn views_text(v: u64) -> String {
    let short = |n: f64, unit: &str| {
        if n < 10.0 && (n * 10.0).round() % 10.0 != 0.0 {
            format!("{:.1}{unit}", n)
        } else {
            format!("{:.0}{unit}", n.floor())
        }
    };
    let n = match v {
        v if v < 1000 => v.to_string(),
        v if v < 1_000_000 => short(v as f64 / 1e3, "K"),
        v if v < 1_000_000_000 => short(v as f64 / 1e6, "M"),
        v => short(v as f64 / 1e9, "B"),
    };
    format!("{n} view{}", if v == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns:yt="http://www.youtube.com/xml/schemas/2015" xmlns:media="http://search.yahoo.com/mrss/" xmlns="http://www.w3.org/2005/Atom">
 <title>Blender Studio</title>
 <entry>
  <id>yt:video:aaaaaaaaaaa</id>
  <yt:videoId>aaaaaaaaaaa</yt:videoId>
  <yt:channelId>UCSMOQeBJ2RAnuFungnQOxLg</yt:channelId>
  <title>CHARGE &amp; more - Blender Open Movie</title>
  <link rel="alternate" href="https://www.youtube.com/watch?v=aaaaaaaaaaa"/>
  <published>2026-10-20T12:00:00+00:00</published>
  <media:group><media:community><media:statistics views="1234567"/></media:community></media:group>
 </entry>
 <entry>
  <yt:videoId>bbbbbbbbbbb</yt:videoId>
  <yt:channelId>UCSMOQeBJ2RAnuFungnQOxLg</yt:channelId>
  <title>A short one</title>
  <link rel="alternate" href="https://www.youtube.com/shorts/bbbbbbbbbbb"/>
  <published>2026-10-22T11:30:00+05:30</published>
  <media:group><media:community><media:statistics views="0"/></media:community></media:group>
 </entry>
</feed>"#;

    // 2026-10-23T00:00:00Z
    const NOW: i64 = 1_792_713_600;

    #[test]
    fn reads_a_channel_feed() {
        let u = parse_feed(FEED, NOW);
        assert_eq!(u.len(), 2);
        assert_eq!(u[0].tile.id, "aaaaaaaaaaa");
        assert_eq!(u[0].tile.title, "CHARGE & more - Blender Open Movie");
        assert_eq!(u[0].tile.byline, "Blender Studio");
        assert_eq!(u[0].tile.kind, Kind::Video);
        assert_eq!(u[0].tile.meta, "1.2M views • 2 days ago");
        assert_eq!(u[0].published, NOW - 2 * 86_400 - 12 * 3600);
        assert_eq!(u[1].tile.kind, Kind::Short);
        // 11:30 at +05:30 is 06:00 UTC, 18 hours before NOW; no views yet.
        assert_eq!(u[1].tile.meta, "18 hours ago");
    }

    #[test]
    fn rows_put_the_newest_channel_first_and_end_in_view_all() {
        let ch = |id: &str, name: &str| Tile {
            kind: Kind::Channel,
            id: id.into(),
            title: name.into(),
            ..Tile::default()
        };
        let channels = vec![
            ch("UCold", "Old"),
            ch("UCnew", "New"),
            ch("UCnone", "Quiet"),
        ];
        let up = |id: &str, t: i64| Upload {
            tile: Tile {
                id: id.into(),
                ..Tile::default()
            },
            published: t,
        };
        let mut short = up("s1", 99);
        short.tile.kind = Kind::Short;
        let uploads = vec![
            vec![up("o1", 10)],
            vec![up("n1", 20), up("n2", 30)],
            vec![short],
        ];
        let p = rows(&channels, &uploads);
        let titles: Vec<&str> = p.shelves.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["New", "Old", "Quiet"]);
        let first: Vec<&str> = p.shelves[0].tiles.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            first,
            ["n2", "n1", "UCnew"],
            "newest first, then the channel"
        );
        let all = p.shelves[0].tiles.last().unwrap();
        assert_eq!((all.kind, all.title.as_str()), (Kind::Channel, "View all"));
        assert_eq!(p.shelves[2].tiles.len(), 1, "a quiet channel still opens");
        assert_eq!(
            newest(&uploads, 2)
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>(),
            ["n2", "n1"]
        );
    }

    #[test]
    fn only_real_channel_ids_get_a_feed() {
        assert!(feed_url("UCSMOQeBJ2RAnuFungnQOxLg").is_some());
        assert!(feed_url("UC&evil=1xxxxxxxxxxxxxxxx").is_none());
        assert!(feed_url("PLabc").is_none());
    }

    #[test]
    fn counts_and_ages_read_like_youtube() {
        assert_eq!(views_text(999), "999 views");
        assert_eq!(views_text(5_700), "5.7K views");
        assert_eq!(views_text(113_000), "113K views");
        assert_eq!(views_text(2_000_000), "2M views");
        assert_eq!(ago(90), "1 minute ago");
        assert_eq!(ago(3 * 3600), "3 hours ago");
        assert_eq!(ago(86_400), "1 day ago");
        assert_eq!(ago(15 * 86_400), "2 weeks ago");
        assert_eq!(parse_time("1970-01-02T00:00:00+00:00"), Some(86_400));
    }
}
