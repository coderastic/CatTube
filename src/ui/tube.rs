//! Drawing for CatTube's screens: pure presentation of
//! `cattube_core::tube::Tube`. Pictures come from YouTube through the app's
//! image cache ([`Pics`]).
//!
//! The look follows CatosTV rather than YouTube's own TV app: the focused
//! video large over its own picture, a frosted pill tab bar whose accent
//! pill slides with focus, and rows of rounded 16:9 cards that lift under
//! focus, the title under each (as on YouTube).

use crate::canvas::Canvas;
use crate::icons;
use crate::images::Images;
use crate::prefs::Prefs;
use crate::target::Target;
use crate::theme::{self, Weight};
use crate::widgets::{self, ListRow, RowKind};
use cattube_core::account::Accounts;
use cattube_core::community;
use cattube_core::player;
use cattube_core::scan::Scan;
use cattube_core::tube::{self, MenuItem, Screen, Section, SettingRow, Tube};
use cattube_core::youtube::{Kind, Page, Tile};
use eframe::egui::{self, Align2, Color32, Rect, TextureId};
use std::cell::RefCell;
use std::collections::HashMap;

type C = Canvas<Target>;

/// Pictures: URL → cached file (`None` = failed), and the URLs the frame
/// wanted but did not have (the app downloads them).
pub struct Pics<'a> {
    pub images: &'a Images,
    pub cache: &'a HashMap<String, Option<String>>,
    pub wanted: &'a RefCell<Vec<String>>,
}

/// The part of a video's picture to show in a 16:9 card: YouTube's 4:3
/// pictures (hqdefault, sddefault) carry the video letterboxed, with black
/// bars above and below; the middle 16:9 is the video.
pub fn crop_16_9(size: Option<[usize; 2]>) -> Rect {
    let full = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    match size {
        Some([w, h]) if w > 0 && h > 0 => {
            let want = w as f32 * 9.0 / 16.0;
            let h = h as f32;
            if h > want * 1.02 {
                let bar = (h - want) / 2.0 / h;
                Rect::from_min_max(egui::pos2(0.0, bar), egui::pos2(1.0, 1.0 - bar))
            } else {
                full
            }
        }
        _ => full,
    }
}

impl Pics<'_> {
    /// How big a picture is, once loaded.
    pub fn size(&self, url: &str) -> Option<[usize; 2]> {
        match self.cache.get(url) {
            Some(Some(path)) => self.images.size(path),
            _ => None,
        }
    }

    pub fn get(&self, c: &C, url: &str) -> Option<TextureId> {
        if url.is_empty() {
            return None;
        }
        match self.cache.get(url) {
            Some(Some(path)) => self.images.get(&c.ctx, path),
            Some(None) => None,
            None => {
                let mut w = self.wanted.borrow_mut();
                if !w.iter().any(|u| u == url) {
                    w.push(url.to_string());
                }
                None
            }
        }
    }
}

/// Everything the screens draw from.
pub struct View<'a> {
    pub tube: &'a Tube,
    pub accounts: &'a Accounts,
    pub prefs: &'a Prefs,
    pub pics: &'a Pics<'a>,
    pub positions: &'a [(String, f64)],
    pub votes: &'a HashMap<String, (u64, u64)>,
    pub tools: &'a Scan<String>,
    pub now: f64,
}

const RADIUS: f32 = 18.0;
/// Card size in rows (16:9), the gap, and the room under a card for its
/// title and channel.
const CARD_W: f32 = 272.0;
const CARD_H: f32 = 153.0;
const GAP: f32 = 24.0;
const UNDER: f32 = 64.0;
const ROW_H: f32 = 40.0 + CARD_H + UNDER + 18.0;

fn focus_t(c: &C, id: impl std::hash::Hash + std::fmt::Debug, focused: bool) -> f32 {
    c.anim(id, if focused { 1.0 } else { 0.0 })
}

fn ease(c: &C, id: impl std::hash::Hash + std::fmt::Debug, secs: f32) -> f32 {
    1.0 - (1.0 - c.appear(id, secs)).powi(3)
}

pub fn draw(c: &C, v: &View) {
    match v.tube.screen() {
        Some(Screen::Browse) | None => draw_browse(c, v),
        Some(Screen::Page {
            page, row, cols, ..
        }) => draw_page(c, v, page, *row, cols),
        Some(Screen::Search {
            osk,
            suggestions,
            suggestion,
            results,
            in_results,
            row,
            cols,
            ..
        }) => draw_search(
            c,
            v,
            osk,
            suggestions,
            *suggestion,
            results,
            *in_results,
            *row,
            cols,
        ),
        Some(Screen::Menu {
            tile,
            details,
            index,
        }) => {
            // The screen underneath stays, dimmed.
            draw_under(c, v);
            draw_menu(c, v, tile, details, *index);
        }
        Some(Screen::SignIn { code }) => draw_sign_in(c, v, code),
        Some(Screen::TvCode { code }) => draw_tv_code(c, v, code),
        Some(Screen::Settings { index, groups }) => draw_settings(c, v, *index, *groups),
    }
}

/// The screen under the video menu.
fn draw_under(c: &C, v: &View) {
    let below = v.tube.stack.len().saturating_sub(2);
    match v.tube.stack.get(below) {
        Some(Screen::Page {
            page, row, cols, ..
        }) => draw_page(c, v, page, *row, cols),
        Some(Screen::Search {
            osk,
            suggestions,
            suggestion,
            results,
            in_results,
            row,
            cols,
            ..
        }) => draw_search(
            c,
            v,
            osk,
            suggestions,
            *suggestion,
            results,
            *in_results,
            *row,
            cols,
        ),
        _ => draw_browse(c, v),
    }
    c.fill(c.screen, 0.0, theme::rgba(theme::BLACK, 120));
}

// ------------------------------------------------------------- backdrop

/// The focused video's picture across the screen, cross-faded when it
/// changes and darkened so the text reads.
fn backdrop(c: &C, v: &View, tile: Option<&Tile>) {
    let dark = theme::BG_BOTTOM;
    // Blurred, so the text over it reads and the eye stays on the cards.
    let url = tile
        .map(|t| t.thumb.clone())
        .filter(|u| !u.is_empty())
        .map(|u| format!("{u}#blur"));
    let tex = url.as_deref().and_then(|u| v.pics.get(c, u));
    let now = c.ctx.input(|i| i.time);
    let shown: (Option<String>, Option<String>, f64) = c.remember("tube-bd", |old| {
        let (cur, prev, since) = old.unwrap_or((None, None, now));
        match (&url, tex) {
            (Some(u), Some(_)) if cur.as_deref() != Some(u.as_str()) => (Some(u.clone()), cur, now),
            (None, _) if cur.is_some() => (None, cur, now),
            _ => (cur, prev, since),
        }
    });
    let k = ((now - shown.2) / 0.6).clamp(0.0, 1.0) as f32;
    let k = 1.0 - (1.0 - k).powi(2);
    if k < 1.0 {
        c.ctx.request_repaint();
    }
    let full = c.r(-20.0, -12.0, 1320.0, 744.0);
    if let Some(t) = shown
        .1
        .as_deref()
        .filter(|_| k < 1.0)
        .and_then(|p| v.pics.get(c, p))
    {
        c.image(full, t, Color32::from_white_alpha(150));
    }
    if let Some(t) = shown.0.as_deref().and_then(|p| v.pics.get(c, p)) {
        let drift = ((now * 0.08).sin() * 6.0) as f32;
        c.image(
            c.r(-20.0 + drift, -12.0, 1320.0, 744.0),
            t,
            Color32::from_white_alpha((150.0 * k) as u8),
        );
    }
    // Dark on the left where the text is, clearing towards the right. It
    // runs to the screen's edge and ends fully clear (it stopped at 900
    // while still a little dark, which left a faint vertical line there).
    c.hgradient(
        c.r(0.0, 0.0, 900.0, 720.0),
        theme::rgba(dark, 235),
        theme::rgba(dark, 40),
    );
    c.hgradient(
        c.r(900.0, 0.0, 380.0, 720.0),
        theme::rgba(dark, 40),
        theme::rgba(dark, 0),
    );
    c.vgradient(
        c.r(0.0, 240.0, 1280.0, 480.0),
        theme::rgba(dark, 0),
        theme::rgba(dark, 245),
    );
    c.vgradient(
        c.r(0.0, 0.0, 1280.0, 130.0),
        theme::rgba(dark, 170),
        theme::rgba(dark, 0),
    );
}

// ---------------------------------------------------------------- header

/// The CatTube mark: a red tile with a play triangle and cat ears.
fn mark(c: &C, x: f32, cy: f32, size: f32, alpha: u8) {
    icons::draw(
        c,
        "cattube",
        x + size / 2.0,
        cy,
        size,
        theme::rgba(theme::accent_rgb(), alpha),
    );
}

/// Mark, the tab bar and the account button. `focus`: the pill with focus
/// while the header has it (tabs, then `Section::ALL.len()` = account).
fn header(c: &C, v: &View, focus: Option<usize>) {
    let k = ease(c, "tube-header", 0.4);
    let y = 26.0 - 14.0 * (1.0 - k);
    mark(c, 52.0, y + 26.0, 30.0, 255);
    c.text(
        92.0,
        y + 26.0,
        Align2::LEFT_CENTER,
        "CatTube",
        22.0,
        theme::text(),
        Weight::Semibold,
    );
    let labels: Vec<&str> = Section::ALL.iter().map(|s| s.label()).collect();
    let name = v.accounts.active().map_or("Sign in", |a| {
        if a.name.is_empty() {
            "Account"
        } else {
            a.name.as_str()
        }
    });
    let acct_w = c.text_width(name, 16.0, Weight::Semibold) + 96.0;
    let tabs_w: f32 = labels.iter().map(|l| widgets::tab_width(c, l)).sum::<f32>()
        + 6.0 * (labels.len() as f32 + 1.0);
    let right_edge = 1216.0 - acct_w;
    let cx = 640.0_f32
        .min(right_edge - 20.0 - tabs_w / 2.0)
        .max(220.0 + tabs_w / 2.0);
    let selected = Section::ALL.iter().position(|s| *s == v.tube.section);
    let rects = widgets::tab_bar(
        c,
        "tube-tabs",
        cx,
        y,
        52.0,
        &labels,
        selected,
        focus.filter(|i| *i < labels.len()),
        focus.is_some_and(|i| i >= labels.len()),
    );
    for (i, rect) in rects.iter().enumerate() {
        c.hit(*rect, Target::Tube(usize::MAX, i));
    }
    let w = acct_w;
    let rect = c.r(1216.0 - w, y + 2.0, w, 48.0);
    // Focused: the same accent pill as the tabs.
    let t = focus_t(c, "tube-acct", focus == Some(labels.len()));
    c.glass(rect, 24.0);
    if t > 0.01 {
        widgets::bar_pill(c, rect, 24.0, t);
    }
    let x = 1216.0 - w;
    let cy = y + 26.0;
    let photo = v
        .accounts
        .active()
        .map(|a| a.photo.clone())
        .unwrap_or_default();
    match v.pics.get(c, &photo) {
        Some(tex) => c.image_rounded(
            c.r(x + 11.0, cy - 15.0, 30.0, 30.0),
            15.0,
            tex,
            Color32::WHITE,
        ),
        None => {
            // The initial's disc swaps colours on the accent pill.
            c.circle(
                x + 26.0,
                cy,
                15.0,
                widgets::mix(theme::accent(), theme::on_accent(), t),
            );
            c.text(
                x + 26.0,
                cy,
                Align2::CENTER_CENTER,
                &name.chars().take(1).collect::<String>().to_uppercase(),
                15.0,
                widgets::mix(theme::on_accent(), theme::accent(), t),
                Weight::Semibold,
            );
        }
    }
    c.text(
        x + 50.0,
        cy,
        Align2::LEFT_CENTER,
        name,
        16.0,
        widgets::mix(theme::text(), theme::on_accent(), t),
        Weight::Semibold,
    );
    icons::draw(
        c,
        "gear",
        x + w - 26.0,
        cy,
        16.0,
        widgets::mix(theme::text2(), theme::on_accent(), t),
    );
    c.hit(rect, Target::Tube(usize::MAX, labels.len()));
}

// ----------------------------------------------------------------- cards

fn resumed(v: &View, t: &Tile) -> Option<f32> {
    t.progress.or_else(|| {
        let pos = v.positions.iter().find(|(id, _)| *id == t.id)?.1;
        // The length from the badge ("12:34", "1:02:03").
        let secs = t.badge.split(':').try_fold(0.0, |acc, p| {
            p.trim().parse::<f64>().ok().map(|n| acc * 60.0 + n)
        })?;
        (secs > 0.0).then(|| (pos / secs).clamp(0.0, 1.0) as f32)
    })
}

/// A 16:9 card with its picture, the badge (length, LIVE), the watched
/// line, and the title and channel under it.
#[allow(clippy::too_many_arguments)]
fn card(c: &C, v: &View, t: &Tile, x: f32, y: f32, w: f32, h: f32, focus: f32) {
    let rect = widgets::lifted(c, c.r(x, y, w, h), focus);
    widgets::focus_lift(c, rect, RADIUS, focus);
    c.fill(rect, RADIUS, theme::rgba(theme::BG_TOP, 220));
    let (rx, ry) = c.to_design(rect.min);
    let (rw, rh) = c.dsize(rect);
    match (t.kind, v.pics.get(c, &t.thumb)) {
        // Channels: their round picture on a soft card.
        (Kind::Channel, tex) => {
            c.fill(rect, RADIUS, theme::glass());
            let d = rh * 0.62;
            let (cx, cy) = (rx + rw / 2.0, ry + rh / 2.0);
            match tex {
                Some(tx) => c.image_rounded(
                    c.r(cx - d / 2.0, cy - d / 2.0, d, d),
                    d / 2.0,
                    tx,
                    Color32::WHITE,
                ),
                None => c.circle(cx, cy, d / 2.0, theme::rgba(theme::WHITE, 30)),
            }
        }
        // Shorts are upright: the picture stands in the middle, its blurred
        // copy fills the sides (squeezed into the card it looked wrong).
        (Kind::Short, Some(tex)) => {
            let k = c.appear(("tube-pic", &t.thumb), 0.3);
            let tint = Color32::from_white_alpha((255.0 * k) as u8);
            match v.pics.get(c, &format!("{}#blur", t.thumb)) {
                Some(soft) => c.image_rounded(rect, RADIUS, soft, tint),
                None => c.fill(rect, RADIUS, theme::rgba(theme::BLACK, 200)),
            }
            let w = (rh * 9.0 / 16.0).min(rw);
            c.image(c.r(rx + (rw - w) / 2.0, ry, w, rh), tex, tint);
        }
        (_, Some(tex)) => {
            let k = c.appear(("tube-pic", &t.thumb), 0.3);
            c.image_rounded_uv(
                rect,
                RADIUS,
                tex,
                crop_16_9(v.pics.size(&t.thumb)),
                Color32::from_white_alpha((255.0 * k) as u8),
            );
        }
        (_, None) => c.fill(rect, RADIUS, theme::glass()),
    }
    if t.kind == Kind::Playlist {
        // A strip on the right with the video count, as YouTube shows it.
        let strip = c.r(rx + rw * 0.62, ry, rw * 0.38, rh);
        c.fill(strip, RADIUS, theme::rgba(theme::BLACK, 170));
        icons::draw(
            c,
            "grid",
            rx + rw * 0.81,
            ry + rh * 0.42,
            22.0,
            theme::text(),
        );
        if !t.badge.is_empty() {
            c.text(
                rx + rw * 0.81,
                ry + rh * 0.66,
                Align2::CENTER_CENTER,
                &c.fit(&t.badge, rw * 0.34, 13.0, Weight::Semibold),
                13.0,
                theme::text(),
                Weight::Semibold,
            );
        }
    } else if !t.badge.is_empty() {
        let live = t.kind == Kind::Live || t.badge == "LIVE";
        let tw = c.text_width(&t.badge, 13.0, Weight::Semibold) + 14.0;
        let b = c.r(rx + rw - tw - 8.0, ry + rh - 30.0, tw, 22.0);
        c.fill(
            b,
            6.0,
            if live {
                theme::rgba((220, 38, 38), 235)
            } else {
                theme::rgba(theme::BLACK, 200)
            },
        );
        let (bx, by) = c.to_design(b.center());
        c.text(
            bx,
            by,
            Align2::CENTER_CENTER,
            &t.badge,
            13.0,
            theme::text(),
            Weight::Semibold,
        );
    }
    if t.kind == Kind::Short {
        let b = c.r(rx + 8.0, ry + 8.0, 54.0, 22.0);
        c.fill(b, 6.0, theme::rgba(theme::accent_rgb(), 220));
        let (bx, by) = c.to_design(b.center());
        c.text(
            bx,
            by,
            Align2::CENTER_CENTER,
            "Short",
            12.0,
            theme::on_accent(),
            Weight::Semibold,
        );
    }
    if let Some(p) = resumed(v, t).filter(|p| *p > 0.01) {
        let line = c.r(rx, ry + rh - 4.0, rw, 4.0);
        c.fill(line, 0.0, theme::rgba(theme::WHITE, 60));
        c.fill(
            c.r(rx, ry + rh - 4.0, (rw * p).max(4.0), 4.0),
            0.0,
            theme::accent(),
        );
    }
    c.frost_edge(rect, RADIUS, 0.4);
    widgets::focus_sheen(c, rect, RADIUS, focus);
    // Title (two lines at most) and channel under the card.
    let ink = if focus > 0.5 {
        theme::text()
    } else {
        theme::text2()
    };
    let (l1, l2) = two_lines(c, &t.title, w, 15.0);
    let ty = y + h + 12.0 + 8.0 * focus;
    c.text(x, ty, Align2::LEFT_TOP, &l1, 15.0, ink, Weight::Semibold);
    if !l2.is_empty() {
        c.text(
            x,
            ty + 19.0,
            Align2::LEFT_TOP,
            &l2,
            15.0,
            ink,
            Weight::Semibold,
        );
    }
    let sub = if t.kind == Kind::Channel {
        t.meta.clone()
    } else {
        t.byline.clone()
    };
    c.text(
        x,
        ty + if l2.is_empty() { 20.0 } else { 39.0 },
        Align2::LEFT_TOP,
        &c.fit(&sub, w, 13.0, Weight::Regular),
        13.0,
        theme::text3(),
        Weight::Regular,
    );
}

/// Break `s` into two lines that fit `w` (the second one cut short).
fn two_lines(c: &C, s: &str, w: f32, size: f32) -> (String, String) {
    if c.text_width(s, size, Weight::Semibold) <= w {
        return (s.to_string(), String::new());
    }
    let words: Vec<&str> = s.split_whitespace().collect();
    let mut first = String::new();
    let mut i = 0;
    while i < words.len() {
        let try_ = if first.is_empty() {
            words[i].to_string()
        } else {
            format!("{first} {}", words[i])
        };
        if c.text_width(&try_, size, Weight::Semibold) > w {
            break;
        }
        first = try_;
        i += 1;
    }
    if first.is_empty() {
        return (c.fit(s, w, size, Weight::Semibold), String::new());
    }
    let rest = words[i..].join(" ");
    (first, c.fit(&rest, w, size, Weight::Semibold))
}

/// The focused tile, large: title, channel, facts.
fn hero(c: &C, t: &Tile, x: f32, y: f32, w: f32) {
    let k = ease(c, ("tube-hero", &t.id), 0.3);
    let a = |v: f32| (v * k) as u8;
    let dx = 18.0 * (1.0 - k);
    let (l1, l2) = two_lines(c, &t.title, w, 34.0);
    c.text(
        x - dx,
        y,
        Align2::LEFT_TOP,
        &l1,
        34.0,
        theme::rgba(theme::INK, a(255.0)),
        Weight::Semibold,
    );
    let mut yy = y + 42.0;
    if !l2.is_empty() {
        c.text(
            x - dx,
            yy,
            Align2::LEFT_TOP,
            &l2,
            34.0,
            theme::rgba(theme::INK, a(255.0)),
            Weight::Semibold,
        );
        yy += 42.0;
    }
    let line = [t.byline.as_str(), t.meta.as_str()]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("  ·  ");
    c.text(
        x - dx,
        yy + 6.0,
        Align2::LEFT_TOP,
        &c.fit(&line, w, 17.0, Weight::Medium),
        17.0,
        theme::rgba(theme::INK, a(185.0)),
        Weight::Medium,
    );
}

/// Rows of cards (a tab, a channel, search results). The focused row
/// sits at `top`; rows above scroll away.
#[allow(clippy::too_many_arguments)]
fn rows(
    c: &C,
    v: &View,
    p: &Page,
    focus: Option<(usize, &[usize])>,
    top: f32,
    id: &str,
    hit: bool,
) {
    let row_now = focus.map_or(0, |(r, _)| r);
    let scroll = c.anim((id, "rows"), row_now as f32 * ROW_H);
    // Just above the focused row's title: the row above leaves no text
    // peeking out under the header.
    let clip_top = top - 8.0;
    let list = c.clipped(0.0, clip_top, 1280.0, 720.0 - clip_top);
    for (ri, sh) in p.shelves.iter().enumerate() {
        let y = top + ri as f32 * ROW_H - scroll;
        if y > 720.0 || y + ROW_H < clip_top {
            continue;
        }
        let row_focus = focus.is_some_and(|(r, _)| r == ri);
        let rise = ease(&list, (id, "row-in", ri), 0.3 + 0.08 * ri.min(6) as f32);
        let y = y + 20.0 * (1.0 - rise);
        let tr = list.anim((id, "row-t", ri), if row_focus { 1.0 } else { 0.0 });
        let title = if sh.title.is_empty() && p.shelves.len() == 1 {
            p.title.as_str()
        } else {
            sh.title.as_str()
        };
        list.text(
            64.0,
            y,
            Align2::LEFT_TOP,
            title,
            17.0 + 1.5 * tr,
            Color32::from_rgba_unmultiplied(
                theme::INK.0,
                theme::INK.1,
                theme::INK.2,
                ((150.0 + 105.0 * tr) * rise) as u8,
            ),
            Weight::Semibold,
        );
        let col = focus
            .and_then(|(_, cols)| cols.get(ri).copied())
            .unwrap_or(0);
        // Four cards fit between the margins (64 to 1224).
        let visible = ((1224.0 - 64.0 + GAP) / (CARD_W + GAP)) as usize;
        let first = col.saturating_sub(visible.saturating_sub(1));
        let sx = list.anim((id, "row-x", ri), first as f32 * (CARD_W + GAP));
        for (ci, t) in sh.tiles.iter().enumerate() {
            let x = 64.0 + ci as f32 * (CARD_W + GAP) - sx;
            if x > 1280.0 || x + CARD_W < 0.0 {
                continue;
            }
            let f = focus_t(&list, (id, "card", ri, ci), row_focus && col == ci);
            card(&list, v, t, x, y + 36.0, CARD_W, CARD_H, f);
            if hit && y + 36.0 > clip_top && y + 36.0 + CARD_H < 720.0 {
                c.hit(c.r(x, y + 36.0, CARD_W, CARD_H), Target::Tube(ri, ci));
            }
        }
    }
}

fn failed(c: &C, e: &str, y: f32) {
    icons::draw(c, "refresh", 640.0, y, 30.0, theme::text3());
    c.text(
        640.0,
        y + 50.0,
        Align2::CENTER_CENTER,
        e,
        20.0,
        theme::text2(),
        Weight::Medium,
    );
    c.text(
        640.0,
        y + 84.0,
        Align2::CENTER_CENTER,
        "Press OK to try again",
        16.0,
        theme::text3(),
        Weight::Regular,
    );
}

// ---------------------------------------------------------------- screens

fn draw_browse(c: &C, v: &View) {
    let t = v.tube;
    let focused = t.focused();
    backdrop(c, v, focused.as_ref());
    header(c, v, t.header.then_some(t.header_index));
    let section = t.section;
    if section.needs_sign_in() && v.accounts.active().is_none() {
        message(
            c,
            &format!("Sign in to see your {}", section.label().to_lowercase()),
            "Choose the tab to sign in with a code",
        );
        return;
    }
    match t.current() {
        Some(Scan::Done(p)) if p.first().is_some_and(|p| !p.shelves.is_empty()) => {
            let p = &p[0];
            if let Some(f) = &focused {
                hero(c, f, 64.0, 112.0, 760.0);
            }
            let focus = (!t.header).then_some((t.row, t.cols.as_slice()));
            rows(c, v, p, focus, 300.0, "browse", true);
        }
        Some(Scan::Done(_)) => {
            if section == Section::Home && v.accounts.active().is_none() {
                message(
                    c,
                    "Search to get started",
                    "Sign in (the button at the top right) for your recommendations and subscriptions",
                );
            } else {
                message(c, "Nothing here yet", "");
            }
        }
        Some(Scan::Failed(e)) => failed(c, e, 320.0),
        _ => widgets::spinner(c, 640.0, 380.0, 22.0, v.now),
    }
    tools_note(c, v);
}

/// While CatTube fetches its player (the first start).
fn tools_note(c: &C, v: &View) {
    if matches!(v.tools, Scan::Busy) && !player::yt_dlp_path(&player::data_home()).is_file() {
        widgets::busy_line(
            c,
            64.0,
            690.0,
            "Getting the player ready (first start)…",
            true,
            v.now,
        );
    }
}

fn message(c: &C, title: &str, sub: &str) {
    c.text(
        640.0,
        360.0,
        Align2::CENTER_CENTER,
        title,
        22.0,
        theme::text(),
        Weight::Semibold,
    );
    if !sub.is_empty() {
        c.text(
            640.0,
            396.0,
            Align2::CENTER_CENTER,
            sub,
            16.0,
            theme::text3(),
            Weight::Regular,
        );
    }
}

fn draw_page(c: &C, v: &View, page: &Scan<Page>, row: usize, cols: &[usize]) {
    let focused = page.items().first().and_then(|p| {
        let sh = p.shelves.get(row)?;
        sh.tiles.get(cols.get(row).copied().unwrap_or(0)).cloned()
    });
    backdrop(c, v, focused.as_ref());
    let k = ease(c, "tube-page-in", 0.35);
    mark(c, 52.0, 52.0, 26.0, (255.0 * k) as u8);
    match page {
        Scan::Done(p) if !p.is_empty() => {
            let p = &p[0];
            c.text(
                92.0,
                52.0,
                Align2::LEFT_CENTER,
                &c.fit(&p.title, 900.0, 22.0, Weight::Semibold),
                22.0,
                theme::text(),
                Weight::Semibold,
            );
            if let Some(f) = &focused {
                hero(c, f, 64.0, 112.0, 760.0);
            }
            if p.shelves.is_empty() {
                message(c, "Nothing here yet", "");
            } else {
                rows(c, v, p, Some((row, cols)), 300.0, "page", true);
            }
        }
        Scan::Failed(e) => failed(c, e, 320.0),
        _ => widgets::spinner(c, 640.0, 380.0, 22.0, v.now),
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_search(
    c: &C,
    v: &View,
    osk: &cattube_core::osk::Osk,
    suggestions: &[String],
    suggestion: Option<usize>,
    results: &Scan<Page>,
    in_results: bool,
    row: usize,
    cols: &[usize],
) {
    let focused = results.items().first().and_then(|p| {
        let sh = p.shelves.get(row)?;
        sh.tiles.get(cols.get(row).copied().unwrap_or(0)).cloned()
    });
    let on_header = v.tube.header;
    backdrop(
        c,
        v,
        if in_results && !on_header {
            focused.as_ref()
        } else {
            None
        },
    );
    header(c, v, on_header.then_some(v.tube.header_index));
    if in_results {
        // What was searched, as a quiet field centred under the top bar
        // (Up from the first row goes back to it), then the channels and a
        // grid of videos.
        let field = c.r(380.0, 92.0, 520.0, 46.0);
        c.fill(field, 23.0, theme::rgba(theme::WHITE, 16));
        c.outline(field, 23.0, 1.0, theme::rgba(theme::WHITE, 22));
        let text = c.fit(osk.text(), 440.0, 18.0, Weight::Medium);
        let tw = c.text_width(&text, 18.0, Weight::Medium);
        let x0 = 640.0 - (tw + 26.0) / 2.0;
        icons::draw(c, "search", x0 + 8.0, 115.0, 15.0, theme::text3());
        c.text(
            x0 + 26.0,
            115.0,
            Align2::LEFT_CENTER,
            &text,
            18.0,
            theme::text(),
            Weight::Medium,
        );
        if let Some(p) = results.items().first() {
            rows(
                c,
                v,
                p,
                (!on_header).then_some((row, cols)),
                168.0,
                "search",
                true,
            );
        }
        return;
    }
    // The keyboard on the left, suggestions on the right, results below.
    widgets::keyboard(
        c,
        osk,
        64.0,
        104.0,
        640.0,
        "Search YouTube",
        suggestion.is_none() && !on_header,
        "tube-osk",
        &Target::Key,
    );
    let x = 760.0;
    for (i, s) in suggestions.iter().enumerate().take(7) {
        let y = 118.0 + i as f32 * 50.0;
        let t = focus_t(c, ("tube-sugg", i), suggestion == Some(i) && !on_header);
        let r = c.r(x, y, 456.0, 42.0);
        c.hit(r, Target::Suggestion(i));
        if t > 0.01 {
            widgets::focus_pill(c, r, 21.0, t);
        }
        icons::draw(c, "search", x + 22.0, y + 21.0, 15.0, theme::text3());
        c.text(
            x + 44.0,
            y + 21.0,
            Align2::LEFT_CENTER,
            &c.fit(s, 396.0, 17.0, Weight::Medium),
            17.0,
            if t > 0.5 {
                theme::text()
            } else {
                theme::text2()
            },
            Weight::Medium,
        );
    }
    match results {
        Scan::Busy => widgets::spinner(c, 640.0, 560.0, 18.0, v.now),
        Scan::Failed(e) => {
            c.text(
                64.0,
                470.0,
                Align2::LEFT_TOP,
                e,
                17.0,
                theme::text2(),
                Weight::Medium,
            );
        }
        Scan::Done(p) if p.first().is_some_and(|p| !p.shelves.is_empty()) => {
            let p = &p[0];
            rows(c, v, p, None, 480.0, "search-peek", false);
            c.text(
                1216.0,
                470.0,
                Align2::RIGHT_TOP,
                "Down for results",
                14.0,
                theme::text3(),
                Weight::Regular,
            );
        }
        Scan::Done(_) => {
            c.text(
                64.0,
                470.0,
                Align2::LEFT_TOP,
                "Nothing found",
                17.0,
                theme::text2(),
                Weight::Medium,
            );
        }
        Scan::Idle => {}
    }
}

fn draw_menu(
    c: &C,
    v: &View,
    tile: &Tile,
    details: &Scan<cattube_core::youtube::Details>,
    index: usize,
) {
    let signed_in = v.accounts.active().is_some();
    let resumable = v.positions.iter().any(|(id, _)| *id == tile.id);
    let items = tube::menu_items(tile, signed_in, resumable);
    let subscribed = details.items().first().is_some_and(|d| d.subscribed);
    let h = 238.0 + items.len() as f32 * 56.0;
    let m = widgets::modal(c, 620.0, h.min(660.0));
    let (mx, my) = c.to_design(m.min);
    // The video: its picture, title, channel, counts.
    if let Some(tex) = v.pics.get(c, &tile.thumb) {
        c.image_rounded_uv(
            c.r(mx + 32.0, my + 32.0, 192.0, 108.0),
            12.0,
            tex,
            crop_16_9(v.pics.size(&tile.thumb)),
            Color32::WHITE,
        );
    }
    let (l1, l2) = two_lines(c, &tile.title, 340.0, 18.0);
    c.text(
        mx + 244.0,
        my + 34.0,
        Align2::LEFT_TOP,
        &l1,
        18.0,
        theme::text(),
        Weight::Semibold,
    );
    c.text(
        mx + 244.0,
        my + 57.0,
        Align2::LEFT_TOP,
        &l2,
        18.0,
        theme::text(),
        Weight::Semibold,
    );
    let d = details.items().first();
    let channel = d
        .map(|d| d.channel.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| tile.byline.clone());
    c.text(
        mx + 244.0,
        my + 88.0,
        Align2::LEFT_TOP,
        &c.fit(&channel, 340.0, 15.0, Weight::Medium),
        15.0,
        theme::text2(),
        Weight::Medium,
    );
    let mut facts: Vec<String> = Vec::new();
    if let Some(d) = d {
        if !d.views.is_empty() {
            facts.push(d.views.clone());
        }
        if !d.date.is_empty() {
            facts.push(d.date.clone());
        }
    }
    if let Some((likes, dislikes)) = v.votes.get(&tile.id) {
        facts.push(format!(
            "👍 {}  👎 {}",
            community::short_count(*likes),
            community::short_count(*dislikes)
        ));
    }
    c.text(
        mx + 244.0,
        my + 112.0,
        Align2::LEFT_TOP,
        &c.fit(&facts.join("  ·  "), 340.0, 14.0, Weight::Regular),
        14.0,
        theme::text3(),
        Weight::Regular,
    );
    let mut y = my + 168.0;
    for (i, it) in items.iter().enumerate() {
        let t = focus_t(c, ("tube-menu", i), index == i);
        let r = c.r(mx + 32.0, y, 556.0, 48.0);
        widgets::button(
            c,
            r,
            it.label(subscribed),
            if i == 0 {
                widgets::ButtonStyle::Primary
            } else {
                widgets::ButtonStyle::Secondary
            },
            t,
        );
        c.hit(r, Target::Tube(0, i));
        y += 56.0;
    }
    if matches!(details, Scan::Busy) && items.contains(&MenuItem::Subscribe) {
        widgets::spinner(c, mx + 590.0, my + 40.0, 10.0, v.now);
    }
}

/// "https://www.google.com/device" → "google.com/device".
fn short_url(url: &str) -> String {
    let u = url.trim();
    let u = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))
        .unwrap_or(u);
    let u = u.strip_prefix("www.").unwrap_or(u);
    u.trim_end_matches('/').to_string()
}

/// How big to draw a sign-in code so it fits `max_w`: (letter size,
/// space per letter, padding inside a part, gap between parts).
fn code_layout(code: &str, max_w: f32) -> (f32, f32, f32, f32) {
    let parts = code.split('-').filter(|p| !p.is_empty()).count().max(1) as f32;
    let letters = code.chars().filter(|c| *c != '-').count().max(1) as f32;
    // At full size: letters 40, 44 apart, 18 padding, 36 between parts.
    let full = letters * 44.0 + parts * 36.0 + (parts - 1.0) * 36.0;
    let k = (max_w / full).min(1.0);
    (40.0 * k, 44.0 * k, 18.0 * k, 36.0 * k)
}

fn draw_sign_in(c: &C, v: &View, code: &Scan<(String, String)>) {
    backdrop(c, v, None);
    let panel = c.r(250.0, 90.0, 780.0, 540.0);
    c.glass(panel, 32.0);
    let k = ease(c, "tube-sign-in", 0.4);
    mark(c, 610.0, 148.0, 44.0, 255);
    c.text(
        640.0,
        204.0,
        Align2::CENTER_CENTER,
        "Sign in to YouTube",
        28.0,
        theme::text(),
        Weight::Semibold,
    );
    match code {
        Scan::Done(v2) if !v2.is_empty() => {
            let (user_code, url) = &v2[0];
            c.text(
                640.0,
                250.0,
                Align2::CENTER_CENTER,
                "On your phone or computer, go to",
                18.0,
                theme::text2(),
                Weight::Regular,
            );
            // The address as people type it ("google.com/device").
            let shown = short_url(url);
            c.text(
                640.0,
                288.0,
                Align2::CENTER_CENTER,
                &c.fit(&shown, 700.0, 30.0, Weight::Semibold),
                30.0,
                theme::accent(),
                Weight::Semibold,
            );
            c.text(
                640.0,
                326.0,
                Align2::CENTER_CENTER,
                "and enter this code",
                18.0,
                theme::text2(),
                Weight::Regular,
            );
            // The code in its parts ("QZY-PCK-NJB": three pills), each
            // letter well apart; everything shrinks to fit the panel, so
            // a longer code never runs off it.
            let (size, step, pad, gap) = code_layout(user_code, 680.0);
            let parts: Vec<&str> = user_code.split('-').filter(|p| !p.is_empty()).collect();
            let widths: Vec<f32> = parts
                .iter()
                .map(|p| p.chars().count() as f32 * step + 2.0 * pad)
                .collect();
            let total = widths.iter().sum::<f32>() + gap * (parts.len().saturating_sub(1)) as f32;
            let mut x = 640.0 - total / 2.0;
            let h = size * 2.0;
            for (pi, (part, w)) in parts.iter().zip(&widths).enumerate() {
                let ki = (k * 1.6 - pi as f32 * 0.12).clamp(0.0, 1.0);
                let r = c.r(x, 366.0 - 12.0 * (1.0 - ki), *w, h);
                c.fill(r, 18.0, theme::rgba(theme::WHITE, (18.0 * ki) as u8));
                c.top_highlight(r, 18.0, theme::rgba(theme::WHITE, (140.0 * ki) as u8));
                c.outline(r, 18.0, 1.0, theme::rgba(theme::WHITE, (40.0 * ki) as u8));
                let (_, cy) = c.to_design(r.center());
                for (ci, ch) in part.chars().enumerate() {
                    c.text(
                        x + pad + step * (ci as f32 + 0.5),
                        cy,
                        Align2::CENTER_CENTER,
                        &ch.to_string(),
                        size,
                        theme::rgba(theme::INK, (255.0 * ki) as u8),
                        Weight::Semibold,
                    );
                }
                if pi + 1 < parts.len() {
                    // The dash between parts, as on the code people read.
                    c.fill(
                        c.r(x + w + gap / 2.0 - 7.0, cy - 1.5, 14.0, 3.0),
                        1.5,
                        theme::rgba(theme::WHITE, (90.0 * ki) as u8),
                    );
                }
                x += w + gap;
            }
            let msg = "Waiting for you to enter it…";
            let w = c.text_width(msg, 16.0, Weight::Medium) + 30.0;
            widgets::busy_line(c, 640.0 - w / 2.0, 490.0, msg, true, v.now);
        }
        Scan::Failed(e) => {
            icons::draw(c, "refresh", 640.0, 380.0, 30.0, theme::text3());
            c.text(
                640.0,
                430.0,
                Align2::CENTER_CENTER,
                e,
                19.0,
                theme::text2(),
                Weight::Medium,
            );
            c.text(
                640.0,
                462.0,
                Align2::CENTER_CENTER,
                "Press OK for a new code",
                16.0,
                theme::text3(),
                Weight::Regular,
            );
        }
        _ => widgets::busy_line(c, 545.0, 380.0, "Getting a code…", true, v.now),
    }
    c.text(
        640.0,
        590.0,
        Align2::CENTER_CENTER,
        "Back to cancel",
        15.0,
        theme::text3(),
        Weight::Regular,
    );
}

/// The phone remote's "Link with TV code" screen: where to type the code
/// on the phone, and the code in four groups of three.
fn draw_tv_code(c: &C, v: &View, code: &Scan<String>) {
    backdrop(c, v, None);
    let panel = c.r(220.0, 90.0, 840.0, 540.0);
    c.glass(panel, 32.0);
    let k = ease(c, "tube-tv-code", 0.4);
    icons::draw(c, "phone", 640.0, 150.0, 40.0, theme::accent());
    c.text(
        640.0,
        204.0,
        Align2::CENTER_CENTER,
        "Link your phone",
        28.0,
        theme::text(),
        Weight::Semibold,
    );
    match code {
        Scan::Done(codes) if !codes.is_empty() => {
            c.text(
                640.0,
                252.0,
                Align2::CENTER_CENTER,
                "In the YouTube app on your phone, open Settings → Watch on TV",
                18.0,
                theme::text2(),
                Weight::Regular,
            );
            c.text(
                640.0,
                280.0,
                Align2::CENTER_CENTER,
                "→ Link with TV code, and enter",
                18.0,
                theme::text2(),
                Weight::Regular,
            );
            let digits: Vec<char> = codes[0].chars().collect();
            let (bw, gap, group_gap) = (46.0, 8.0, 26.0);
            let total = 12.0 * bw + 8.0 * gap + 3.0 * group_gap;
            let mut x = 640.0 - total / 2.0;
            for (i, ch) in digits.iter().enumerate() {
                if i > 0 {
                    x += if i % 3 == 0 { group_gap } else { gap };
                }
                let ki = (k * 1.6 - i as f32 * 0.04).clamp(0.0, 1.0);
                let r = c.r(x, 330.0 - 12.0 * (1.0 - ki), bw, 70.0);
                c.fill(r, 14.0, theme::rgba(theme::WHITE, (18.0 * ki) as u8));
                c.top_highlight(r, 14.0, theme::rgba(theme::WHITE, (140.0 * ki) as u8));
                c.outline(r, 14.0, 1.0, theme::rgba(theme::WHITE, (40.0 * ki) as u8));
                let (cx, cy) = c.to_design(r.center());
                c.text(
                    cx,
                    cy,
                    Align2::CENTER_CENTER,
                    &ch.to_string(),
                    34.0,
                    theme::rgba(theme::INK, (255.0 * ki) as u8),
                    Weight::Semibold,
                );
                x += bw;
            }
            c.text(
                640.0,
                444.0,
                Align2::CENTER_CENTER,
                "Then pick a video on the phone: it plays here, and the phone stays in step.",
                16.0,
                theme::text3(),
                Weight::Regular,
            );
            c.text(
                640.0,
                472.0,
                Align2::CENTER_CENTER,
                "The code changes every few minutes. A linked phone stays linked.",
                16.0,
                theme::text3(),
                Weight::Regular,
            );
        }
        Scan::Failed(e) => {
            icons::draw(c, "refresh", 640.0, 360.0, 30.0, theme::text3());
            c.text(
                640.0,
                410.0,
                Align2::CENTER_CENTER,
                e,
                19.0,
                theme::text2(),
                Weight::Medium,
            );
            c.text(
                640.0,
                442.0,
                Align2::CENTER_CENTER,
                "Press OK to try again",
                16.0,
                theme::text3(),
                Weight::Regular,
            );
        }
        _ => widgets::busy_line(c, 545.0, 360.0, "Getting a code…", true, v.now),
    }
    c.text(
        640.0,
        590.0,
        Align2::CENTER_CENTER,
        "OK for a new code · Back to close",
        15.0,
        theme::text3(),
        Weight::Regular,
    );
}

fn draw_settings(c: &C, v: &View, index: usize, on_groups: bool) {
    backdrop(c, v, None);
    let k = ease(c, "tube-settings-in", 0.35);
    mark(c, 52.0, 52.0, 26.0, (255.0 * k) as u8);
    c.text(
        92.0,
        52.0,
        Align2::LEFT_CENTER,
        "CatTube",
        19.0,
        theme::rgba(theme::INK, 200),
        Weight::Semibold,
    );
    widgets::title(c, 64.0, 92.0, "Settings", 30.0);
    let rows = tube::setting_rows(v.accounts.list.len(), v.prefs.sponsorblock);
    let groups = tube::setting_groups(&rows);
    let shown = groups
        .iter()
        .position(|(_, r)| r.contains(&index))
        .unwrap_or(0);

    // The groups, a list on the left. The shown one is lit; with focus on
    // the list it also gets the focus pill.
    for (gi, (g, range)) in groups.iter().enumerate() {
        let y = 150.0 + gi as f32 * 54.0;
        let rect = c.r(40.0, y, 260.0, 46.0);
        let here = gi == shown;
        let t = focus_t(c, ("tube-set-group", gi), on_groups && here);
        if t > 0.01 {
            widgets::focus_pill(c, rect, 16.0, t);
        } else if here {
            c.fill(rect, 16.0, theme::rgba(theme::WHITE, 16));
        }
        c.hit(rect, Target::Tube(0, range.start));
        let ink = if here || t > 0.5 {
            theme::text()
        } else {
            theme::text2()
        };
        icons::draw(c, g.icon(), 66.0, y + 23.0, 17.0, ink);
        c.text(
            90.0,
            y + 23.0,
            Align2::LEFT_CENTER,
            g.label(),
            17.0,
            ink,
            if here {
                Weight::Semibold
            } else {
                Weight::Medium
            },
        );
    }

    // The shown group's settings, in a panel on the right.
    let Some((group, range)) = groups.get(shown).cloned() else {
        return;
    };
    let panel = c.r(330.0, 140.0, 898.0, 552.0);
    c.glass(panel, 28.0);
    c.text(
        362.0,
        176.0,
        Align2::LEFT_CENTER,
        group.label(),
        22.0,
        theme::text(),
        Weight::Semibold,
    );
    let (x, w, h, step) = (354.0, 850.0, 60.0, 68.0);
    let at = index.saturating_sub(range.start);
    // Whole rows scroll away, so no half row shows under the title.
    let target = (at as f32 - 5.0).max(0.0) * step;
    let scroll = c.anim(("tube-settings-scroll", shown), target);
    let list = c.clipped(330.0, 204.0, 898.0, 480.0);
    let p = v.prefs;
    let mut y = 212.0 - scroll;
    for (i, row) in rows.iter().enumerate() {
        if !range.contains(&i) {
            continue;
        }
        let speed = format!("{}×", p.speed);
        let quality = player::resolution_label(p.max_height);
        let tools = match v.tools {
            Scan::Done(s) => s.first().cloned().unwrap_or_default(),
            Scan::Failed(e) => e.clone(),
            _ => "Updating…".into(),
        };
        let (title, sub, value, kind, icon): (String, String, String, RowKind, &str) = match row {
            SettingRow::Account(i) => {
                let a = &v.accounts.list[*i];
                (
                    if a.name.is_empty() {
                        "YouTube account".into()
                    } else {
                        a.name.clone()
                    },
                    "OK uses it · hold OK to sign out".into(),
                    if *i == v.accounts.active {
                        "In use".into()
                    } else {
                        String::new()
                    },
                    RowKind::Action,
                    "cattube",
                )
            }
            SettingRow::AddAccount => (
                "Add an account".into(),
                "Sign in with a code".into(),
                String::new(),
                RowKind::Action,
                "plus",
            ),
            SettingRow::Quality => (
                "Highest quality".into(),
                "Lower plays smoother on slow connections".into(),
                quality,
                RowKind::Choice,
                "display",
            ),
            SettingRow::Codec => (
                "Preferred codec".into(),
                "VP9 suits most TVs; AV1 needs newer graphics".into(),
                p.codec.label().into(),
                RowKind::Choice,
                "media",
            ),
            SettingRow::Hdr => (
                "HDR".into(),
                "When the video and the TV have it".into(),
                String::new(),
                RowKind::Toggle(Some(p.hdr)),
                "spark",
            ),
            SettingRow::History => (
                "Save to YouTube history".into(),
                "What you watch here shows in your history, with how far you got".into(),
                String::new(),
                RowKind::Toggle(Some(p.history)),
                "check",
            ),
            SettingRow::Buffer => (
                "Buffer".into(),
                "Bigger rides out a shaky connection, smaller uses less memory".into(),
                match p.buffer {
                    64 => "Small".into(),
                    768 => "Large".into(),
                    _ => "Medium".into(),
                },
                RowKind::Choice,
                "download",
            ),
            SettingRow::Region => (
                "Country".into(),
                "For recommendations, trending and news".into(),
                p.region.clone(),
                RowKind::Choice,
                "home",
            ),
            SettingRow::Speed => (
                "Speed".into(),
                "Also changed in the player, and kept".into(),
                speed,
                RowKind::Choice,
                "play",
            ),
            SettingRow::Subtitles => (
                "Subtitles".into(),
                format!("Language: {}", p.sub_lang),
                String::new(),
                RowKind::Toggle(Some(p.subs_on)),
                "keyboard",
            ),
            SettingRow::Autoplay => (
                "When a video ends".into(),
                String::new(),
                p.autoplay.label().into(),
                RowKind::Choice,
                "refresh",
            ),
            SettingRow::SponsorBlock => (
                "Skip sponsors and more".into(),
                "Segments marked by SponsorBlock viewers".into(),
                String::new(),
                RowKind::Toggle(Some(p.sponsorblock)),
                "check",
            ),
            SettingRow::Category(ci) => {
                let (key, label, _) = community::CATEGORIES[*ci];
                (
                    label.into(),
                    String::new(),
                    String::new(),
                    RowKind::Toggle(Some(p.skips(key))),
                    "",
                )
            }
            SettingRow::LinkPhone => (
                "Link a phone".into(),
                "Pick and control videos from the YouTube app".into(),
                String::new(),
                RowKind::Action,
                "phone",
            ),
            SettingRow::UpdateYtDlp => (
                "Update the player".into(),
                format!("{tools} · CatTube {}", cattube_core::env::version()),
                String::new(),
                RowKind::Action,
                "download",
            ),
        };
        let t = focus_t(&list, ("tube-set", i), index == i && !on_groups);
        let rect = list.r(x, y, w, h);
        widgets::list_row(
            &list,
            rect,
            &ListRow {
                title: &title,
                subtitle: &sub,
                value: &value,
                kind: Some(kind),
                icon: (!icon.is_empty()).then_some(icon),
                disabled: false,
            },
            t,
        );
        if y > 204.0 && y + h < 684.0 {
            c.hit(rect, Target::Tube(1, i));
        }
        y += step;
    }
}

/// Pointer over the search keyboard or a suggestion: focus it.
pub fn focus_search(t: &mut Tube, target: Target) -> bool {
    let header = &mut t.header;
    let Some(Screen::Search {
        osk,
        suggestion,
        suggestions,
        in_results,
        ..
    }) = t.stack.last_mut()
    else {
        return false;
    };
    match target {
        Target::Key(r, c) => {
            *header = false;
            *in_results = false;
            *suggestion = None;
            osk.set_focus(r, c);
            true
        }
        Target::Suggestion(i) if i < suggestions.len() => {
            *header = false;
            *in_results = false;
            *suggestion = Some(i);
            true
        }
        _ => false,
    }
}

/// Pointer: focus the item at (row, col) on the current screen; false
/// when there is nothing there. `row == usize::MAX`: the header pills.
pub fn focus(t: &mut Tube, row: usize, col: usize) -> bool {
    // The top bar over Search.
    if row == usize::MAX && matches!(t.stack.last(), Some(Screen::Search { .. })) {
        t.header = true;
        t.header_index = col.min(Section::ALL.len());
        return true;
    }
    let has_rows = |p: &Scan<Page>| {
        p.items()
            .first()
            .is_some_and(|p| row < p.shelves.len() && col < p.shelves[row].tiles.len())
    };
    match t.stack.last_mut() {
        Some(Screen::Browse) => {
            if row == usize::MAX {
                t.header = true;
                t.header_index = col.min(Section::ALL.len());
                return true;
            }
            let ok = t.current().is_some_and(has_rows);
            if ok {
                t.header = false;
                t.row = row;
                if t.cols.len() <= row {
                    t.cols.resize(row + 1, 0);
                }
                t.cols[row] = col;
            }
            ok
        }
        Some(Screen::Page {
            page, row: r, cols, ..
        }) if has_rows(page) => {
            *r = row;
            if cols.len() <= row {
                cols.resize(row + 1, 0);
            }
            cols[row] = col;
            true
        }
        Some(Screen::Search {
            results,
            in_results,
            row: r,
            cols,
            ..
        }) if has_rows(results) => {
            t.header = false;
            *in_results = true;
            *r = row;
            if cols.len() <= row {
                cols.resize(row + 1, 0);
            }
            cols[row] = col;
            true
        }
        Some(Screen::Menu { index, .. }) => {
            *index = col;
            true
        }
        // Settings: row 0 is the groups (col = the group's first row),
        // row 1 the group's rows.
        Some(Screen::Settings { index, groups }) => {
            *index = col;
            *groups = row == 0;
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_in_code_and_address_fit_the_panel() {
        assert_eq!(
            short_url("https://www.google.com/device"),
            "google.com/device"
        );
        assert_eq!(
            short_url("http://youtube.com/activate/"),
            "youtube.com/activate"
        );
        // Google's usual code: full size, inside the 680 wide area.
        let (size, step, pad, gap) = code_layout("QZY-PCK-NJB", 680.0);
        assert_eq!(size, 40.0);
        assert!(9.0 * step + 3.0 * 2.0 * pad + 2.0 * gap <= 680.0);
        // A longer one shrinks to fit.
        let code = "ABCD-EFGH-IJKL-MNOP";
        let (size, step, pad, gap) = code_layout(code, 680.0);
        assert!(size < 40.0);
        assert!(16.0 * step + 4.0 * 2.0 * pad + 3.0 * gap <= 680.0 + 0.01);
    }

    #[test]
    fn letterboxed_pictures_lose_their_black_bars() {
        // hqdefault: 480×360, the video 480×270 in the middle.
        let r = crop_16_9(Some([480, 360]));
        assert!((r.min.y - 0.125).abs() < 1e-6 && (r.max.y - 0.875).abs() < 1e-6);
        assert_eq!(r.min.x, 0.0);
        assert_eq!(r.max.x, 1.0);
        // Already 16:9 (hq720, mqdefault), unknown size: the whole picture.
        for s in [Some([1280, 720]), Some([320, 180]), None, Some([0, 0])] {
            let r = crop_16_9(s);
            assert_eq!((r.min.y, r.max.y), (0.0, 1.0), "{s:?}");
        }
    }
}
