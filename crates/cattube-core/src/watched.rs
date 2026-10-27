//! What was watched on this TV, newest first, for the History tab
//! (`watched.conf`, one video a line, tab-separated).

use crate::youtube::{Kind, Page, Shelf, Tile};

/// How many videos History keeps.
pub const KEEP: usize = 300;
/// Videos to a row on the History tab.
pub const COLS: usize = 4;

fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

pub fn parse(text: &str) -> Vec<Tile> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            let id = *f.first()?;
            let ok = id.len() == 11
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
            if !ok {
                return None;
            }
            let get = |i: usize| f.get(i).copied().unwrap_or("").to_string();
            Some(Tile {
                kind: match get(6).as_str() {
                    "short" => Kind::Short,
                    "live" => Kind::Live,
                    _ => Kind::Video,
                },
                id: id.to_string(),
                title: get(1),
                byline: get(2),
                channel_id: get(3),
                badge: get(4),
                thumb: get(5),
                ..Tile::default()
            })
        })
        .take(KEEP)
        .collect()
}

pub fn to_text(list: &[Tile]) -> String {
    list.iter()
        .map(|t| {
            let kind = match t.kind {
                Kind::Short => "short",
                Kind::Live => "live",
                _ => "video",
            };
            format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                t.id,
                clean(&t.title),
                clean(&t.byline),
                clean(&t.channel_id),
                clean(&t.badge),
                clean(&t.thumb),
                kind
            )
        })
        .collect()
}

/// A video was played: it goes to the top.
pub fn remember(list: &mut Vec<Tile>, t: &Tile) {
    if !t.playable() || t.id.is_empty() {
        return;
    }
    let mut t = t.clone();
    // A live stream watched is a video by the time you come back.
    if t.kind == Kind::Live {
        t.badge.clear();
    }
    t.progress = None;
    t.meta.clear();
    // Opened from the TV's home screen: only the id is known.
    if t.thumb.is_empty() {
        t.thumb = format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", t.id);
    }
    list.retain(|x| x.id != t.id);
    list.insert(0, t);
    list.truncate(KEEP);
}

/// Record a video once it actually starts, including related and autoplay picks.
pub fn record(data: &std::path::Path, tile: &Tile) {
    static WRITER: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = WRITER.lock().unwrap_or_else(|e| e.into_inner());
    let path = data.join("watched.conf");
    let mut list = std::fs::read_to_string(&path)
        .map(|s| parse(&s))
        .unwrap_or_default();
    remember(&mut list, tile);
    let tmp = data.join(format!("watched-{}.tmp", std::process::id()));
    if std::fs::create_dir_all(data).is_ok() && std::fs::write(&tmp, to_text(&list)).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// The History tab: the videos in rows of [`COLS`].
pub fn page(list: &[Tile]) -> Page {
    Page {
        title: "History".into(),
        shelves: list
            .chunks(COLS)
            .enumerate()
            .map(|(i, c)| Shelf {
                title: if i == 0 {
                    "Watched".into()
                } else {
                    String::new()
                },
                tiles: c.to_vec(),
                ..Shelf::default()
            })
            .collect(),
        continuation: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(id: &str) -> Tile {
        Tile {
            kind: Kind::Video,
            id: id.into(),
            title: format!("Title\t{id}"),
            byline: "Chan".into(),
            badge: "7:45".into(),
            thumb: "https://i.ytimg.com/vi/x/hqdefault.jpg".into(),
            ..Tile::default()
        }
    }

    #[test]
    fn newest_first_without_doubles_and_round_trip() {
        let mut l = Vec::new();
        remember(&mut l, &tile("aaaaaaaaaaa"));
        remember(&mut l, &tile("bbbbbbbbbbb"));
        remember(&mut l, &tile("aaaaaaaaaaa"));
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].id, "aaaaaaaaaaa");
        let back = parse(&to_text(&l));
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].title, "Title aaaaaaaaaaa", "no tabs inside a field");
        assert_eq!(back[1].badge, "7:45");
        assert!(parse("bad\tx\n\n").is_empty());
        for i in 0..400 {
            remember(&mut l, &tile(&format!("{i:011}")));
        }
        assert_eq!(l.len(), KEEP);
        let p = page(&l[..6]);
        assert_eq!(p.shelves.len(), 2);
        assert_eq!(p.shelves[0].title, "Watched");
        // Channels are not videos.
        let mut c = tile("UCxxxxxxxxx");
        c.kind = Kind::Channel;
        remember(&mut l, &c);
        assert_ne!(l[0].id, "UCxxxxxxxxx");
    }
}
