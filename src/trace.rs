//! Start-up timing on stderr when `SILENT_TRACE` is set. Each mark carries
//! the wall clock, so a harness can measure from before the D-Bus call that
//! started the process. gpui's vendored backends mark their steps as log
//! records with the target `silent::trace`, which land here too, and with
//! `SILENT_TRACE=frames` every frame's, with the target `silent::frames`.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static START: OnceLock<Option<Instant>> = OnceLock::new();
const TARGET: &str = "silent::trace";
const FRAMES: &str = "silent::frames";
static FRAMES_ON: AtomicBool = AtomicBool::new(false);

pub fn start() {
    let tracing = START
        .get_or_init(|| std::env::var_os("SILENT_TRACE").map(|_| Instant::now()))
        .is_some();
    if !tracing {
        return;
    }
    let frames = std::env::var_os("SILENT_TRACE").is_some_and(|v| v == "frames");
    FRAMES_ON.store(frames, Ordering::Relaxed);
    if log::set_logger(&Marks).is_ok() {
        log::set_max_level(if frames {
            log::LevelFilter::Debug
        } else {
            log::LevelFilter::Info
        });
    }
    // The time the process has run or waited to run so far: since its fork,
    // unless it slept, as it may on a binary that is not in the page cache.
    let ran = std::fs::read_to_string("/proc/self/schedstat")
        .ok()
        .map(|stat| {
            stat.split_whitespace()
                .take(2)
                .filter_map(|ns| ns.parse::<u64>().ok())
                .sum::<u64>()
        })
        .unwrap_or_default();
    print("process start, by schedstat", wall() - u128::from(ran), -(ran as f64) / 1e6);
}

pub fn mark(what: &str) {
    let Some(Some(start)) = START.get() else {
        return;
    };
    print(what, wall(), start.elapsed().as_secs_f64() * 1e3);
}

fn wall() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos()
}

fn print(what: &str, wall: u128, after_main: f64) {
    eprintln!("silent: {what} at {wall} ns, {after_main:.1} ms after main");
}

struct Marks;

impl log::Log for Marks {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.target() == TARGET
            || (metadata.target() == FRAMES && FRAMES_ON.load(Ordering::Relaxed))
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            mark(&record.args().to_string());
        }
    }

    fn flush(&self) {}
}
