//! Pictures (posters, backdrops, episode stills) as GPU textures.
//!
//! Decoded lazily on first use and cached for the life of the process. A
//! path that fails to load is remembered as missing so a bad file is not
//! re-read every frame; the caller then draws a placeholder.

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

/// Largest side kept in memory; bigger pictures (backdrops) are scaled down by box averaging.
const MAX_SIDE: u32 = 1600;
/// Pictures bigger than this many bytes are decoded on a worker thread, so
/// scrolling through posters never stalls a frame.
const ASYNC_BYTES: u64 = 192 * 1024;
/// Never decode anything larger than this (a broken or hostile file).
const DECODE_LIMIT: u32 = 8192;

type Decoded = Arc<Mutex<HashMap<String, Option<ColorImage>>>>;

#[derive(Default)]
pub struct Images {
    cache: RefCell<HashMap<String, Option<TextureHandle>>>,
    /// Big PNGs being decoded right now (shared with the worker threads).
    pending: Arc<Mutex<HashSet<String>>>,
    /// Big PNGs decoded by worker threads, waiting to become textures.
    decoded: Decoded,
}

impl Images {
    pub fn new() -> Self {
        Self::default()
    }

    /// Texture for a PNG (or SVG) file, loading it on first use. An SVG
    /// returns `None` until its PNG has been rendered in the background.
    pub fn get(&self, ctx: &egui::Context, path: &str) -> Option<egui::TextureId> {
        if let Some(entry) = self.cache.borrow().get(path) {
            return entry.as_ref().map(|t| t.id());
        }
        let file = path.to_string();
        // A big file: decode on a worker thread, upload when it is ready.
        let ready = self
            .decoded
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(path);
        let img = match ready {
            Some(img) => img,
            None if std::fs::metadata(&file).is_ok_and(|m| m.len() > ASYNC_BYTES) => {
                self.decode_later(ctx, path, file);
                return None;
            }
            None => load_png(&file),
        };
        let tex = img.map(|img| ctx.load_texture(path, img, TextureOptions::LINEAR));
        let id = tex.as_ref().map(|t| t.id());
        self.cache.borrow_mut().insert(path.to_string(), tex);
        id
    }

    /// Width and height of a picture already loaded.
    pub fn size(&self, path: &str) -> Option<[usize; 2]> {
        self.cache
            .borrow()
            .get(path)
            .and_then(|t| t.as_ref().map(|t| t.size()))
    }

    fn decode_later(&self, ctx: &egui::Context, path: &str, file: String) {
        {
            let mut p = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            if !p.insert(path.to_string()) {
                return;
            }
        }
        let (pending, decoded) = (Arc::clone(&self.pending), Arc::clone(&self.decoded));
        let ctx = ctx.clone();
        let path = path.to_string();
        std::thread::spawn(move || {
            let img = load_png(&file);
            decoded
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(path.clone(), img);
            pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&path);
            ctx.request_repaint();
        });
    }
}

/// A cached picture: PNG, JPEG (YouTube's thumbnails) or WebP.
fn load_png(path: &str) -> Option<ColorImage> {
    if ![".png", ".jpg", ".jpeg", ".webp"]
        .iter()
        .any(|e| path.ends_with(e))
    {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 || w > DECODE_LIMIT || h > DECODE_LIMIT {
        return None;
    }
    let (w, h, px) = shrink(w, h, img.as_raw().to_vec(), MAX_SIDE);
    Some(ColorImage::from_rgba_unmultiplied(
        [w as usize, h as usize],
        &px,
    ))
}

/// Scale RGBA pixels down by a whole factor (box average) until both
/// sides fit in `max`. Pictures that already fit are returned as they are.
pub fn shrink(w: u32, h: u32, px: Vec<u8>, max: u32) -> (u32, u32, Vec<u8>) {
    let k = w.max(h).div_ceil(max.max(1));
    if k <= 1 {
        return (w, h, px);
    }
    let (nw, nh) = ((w / k).max(1), (h / k).max(1));
    let mut out = vec![0u8; (nw * nh * 4) as usize];
    for y in 0..nh {
        for x in 0..nw {
            let mut acc = [0u32; 4];
            for dy in 0..k {
                for dx in 0..k {
                    let i = (((y * k + dy) * w + (x * k + dx)) * 4) as usize;
                    for c in 0..4 {
                        acc[c] += px[i + c] as u32;
                    }
                }
            }
            let o = ((y * nw + x) * 4) as usize;
            for c in 0..4 {
                out[o + c] = (acc[c] / (k * k)) as u8;
            }
        }
    }
    (nw, nh, out)
}

#[cfg(test)]
mod tests {
    use super::shrink;

    #[test]
    fn shrink_box_averages_and_keeps_small_pictures() {
        let px = vec![10u8; 4 * 4 * 4];
        assert_eq!(shrink(4, 4, px.clone(), 8), (4, 4, px));
        let mut px = vec![0u8; 4 * 2 * 4];
        // Left 2×2 block white, right block black.
        for y in 0..2 {
            for x in 0..2 {
                let i = (y * 4 + x) * 4;
                px[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        let (w, h, out) = shrink(4, 2, px, 2);
        assert_eq!((w, h), (2, 1));
        assert_eq!(&out[0..4], &[255, 255, 255, 255]);
        assert_eq!(&out[4..8], &[0, 0, 0, 0]);
    }
}
