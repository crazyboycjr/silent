//! silent: what went into each frame, printed with `SILENT_TRACE=frames`: the
//! lines shaped and the glyphs rasterized since the last frame, wherever gpui
//! drew it, and the tiles put in the atlas.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Instant;

pub(crate) struct Tally {
    count: AtomicU64,
    amount: AtomicU64,
}

pub(crate) static SHAPED: Tally = Tally::new();
pub(crate) static RASTERIZED: Tally = Tally::new();
/// Tiles and their bytes.
pub(crate) static UPLOADED: Tally = Tally::new();
pub(crate) static TEXTURES: Tally = Tally::new();

pub(crate) fn on() -> bool {
    log::max_level() >= log::LevelFilter::Debug
}

impl Tally {
    const fn new() -> Self {
        Self {
            count: AtomicU64::new(0),
            amount: AtomicU64::new(0),
        }
    }

    pub(crate) fn add(&self, count: u64, amount: u64) {
        self.count.fetch_add(count, Relaxed);
        self.amount.fetch_add(amount, Relaxed);
    }

    /// Runs `f`, adding `count` and its time in nanoseconds.
    pub(crate) fn time<R>(&self, count: u64, f: impl FnOnce() -> R) -> R {
        if !on() {
            return f();
        }
        let started = Instant::now();
        let out = f();
        self.add(count, started.elapsed().as_nanos() as u64);
        out
    }

    pub(crate) fn take(&self) -> (u64, u64) {
        (self.count.swap(0, Relaxed), self.amount.swap(0, Relaxed))
    }
}

/// The tallies since the last frame, for its line.
pub(crate) fn since_last() -> String {
    let (shaped, shaping) = SHAPED.take();
    let (glyphs, rasterizing) = RASTERIZED.take();
    let (tiles, bytes) = UPLOADED.take();
    let (textures, _) = TEXTURES.take();
    format!(
        "shaped {shaped} lines in {:.1} ms, rasterized {glyphs} glyphs in {:.1} ms, \
         {tiles} tiles ({} KB) to the atlas, {textures} new textures",
        shaping as f64 / 1e6,
        rasterizing as f64 / 1e6,
        bytes / 1024
    )
}
