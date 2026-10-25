//! Start-up timing on stderr when `SILENT_TRACE` is set. Each mark carries
//! the wall clock, so a harness can measure from before the D-Bus call that
//! started the process.

use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

static START: OnceLock<Option<Instant>> = OnceLock::new();

pub fn start() {
    START.get_or_init(|| std::env::var_os("SILENT_TRACE").map(|_| Instant::now()));
}

pub fn mark(what: &str) {
    let Some(Some(start)) = START.get() else {
        return;
    };
    let wall = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    eprintln!(
        "silent: {what} at {wall} ns, {:.1} ms after main",
        start.elapsed().as_secs_f64() * 1e3
    );
}
