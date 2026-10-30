//! silent: how often a window is given a frame. A window gets the current
//! mode's rate of the monitor it lies over most, read again whenever it is
//! shown, moved or resized and whenever RandR tells of a change, so that a
//! monitor that slept, woke or changed its mode sets the pace once it is
//! back. With no lit monitor under it, as while its monitor sleeps, a
//! window keeps the rate it had, or takes the fastest there is.

use std::time::Duration;

use x11rb::{
    connection::RequestConnection,
    protocol::{
        randr::{self, ConnectionExt as _},
        xproto::{self, ConnectionExt as _},
    },
};

const FRAMES: &str = "silent::frames";

/// How often a window gets a frame, and what that comes from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Refresh {
    pub period: Duration,
    pub source: Source,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    /// The current mode of the monitor, its CRTC, that the window lies
    /// over most.
    Monitor(randr::Crtc),
    /// The rate the window had from its monitor, while it lies over no lit
    /// monitor.
    Kept,
    /// The fastest lit monitor's, while the window lies over none.
    FastestMonitor,
    /// The fastest mode RandR lists, while no monitor is lit.
    FastestMode,
    /// Nothing to go by.
    Default,
}

const DEFAULT_HZ: f64 = 60.0;

impl Refresh {
    fn at(hz: f64, source: Source) -> Self {
        Self {
            period: Duration::from_secs_f64(1.0 / hz),
            source,
        }
    }

    fn hz(&self) -> f64 {
        1.0 / self.period.as_secs_f64()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl Rect {
    fn overlap(&self, other: &Rect) -> i64 {
        let width = (self.x + self.width).min(other.x + other.width) - self.x.max(other.x);
        let height = (self.y + self.height).min(other.y + other.height) - self.y.max(other.y);
        i64::from(width.max(0)) * i64::from(height.max(0))
    }
}

impl std::fmt::Display for Rect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}x{}{:+}{:+}", self.width, self.height, self.x, self.y)
    }
}

/// A CRTC that shows something: where, and at what rate.
#[derive(Clone, Copy, Debug)]
struct Lit {
    crtc: randr::Crtc,
    rect: Rect,
    hz: f64,
}

/// What RandR says of the monitors: the CRTCs lit, in its order, and the
/// fastest mode it lists.
#[derive(Debug, Default)]
struct Monitors {
    lit: Vec<Lit>,
    fastest_mode: Option<f64>,
}

impl Monitors {
    fn new(
        resources: &randr::GetScreenResourcesCurrentReply,
        crtcs: &[(randr::Crtc, randr::GetCrtcInfoReply)],
    ) -> Self {
        let rate = |mode: randr::Mode| {
            resources
                .modes
                .iter()
                .find(|info| info.id == mode)
                .and_then(mode_hz)
        };
        let lit = crtcs
            .iter()
            .filter_map(|(crtc, info)| {
                let hz = (info.mode != 0).then(|| rate(info.mode)).flatten()?;
                Some(Lit {
                    crtc: *crtc,
                    rect: Rect {
                        x: info.x.into(),
                        y: info.y.into(),
                        width: info.width.into(),
                        height: info.height.into(),
                    },
                    hz,
                })
            })
            .collect();
        let fastest_mode = resources
            .modes
            .iter()
            .filter_map(mode_hz)
            .max_by(f64::total_cmp);
        Self { lit, fastest_mode }
    }
}

/// A mode's refresh rate in Hz, as xrandr reckons it, if it is believable.
fn mode_hz(mode: &randr::ModeInfo) -> Option<f64> {
    let mut lines = f64::from(mode.vtotal);
    if mode.mode_flags.contains(randr::ModeFlag::DOUBLE_SCAN) {
        lines *= 2.0;
    }
    if mode.mode_flags.contains(randr::ModeFlag::INTERLACE) {
        lines /= 2.0;
    }
    let hz = f64::from(mode.dot_clock) / (f64::from(mode.htotal) * lines);
    (hz.is_finite() && (10.0..=1000.0).contains(&hz)).then_some(hz)
}

fn choose(window: Option<Rect>, monitors: &Monitors, before: Option<Refresh>) -> Refresh {
    let under = window.and_then(|window| {
        monitors
            .lit
            .iter()
            .map(|lit| (lit.rect.overlap(&window), lit))
            .filter(|&(overlap, _)| overlap > 0)
            .max_by(|(a, lit_a), (b, lit_b)| a.cmp(b).then(lit_a.hz.total_cmp(&lit_b.hz)))
            .map(|(_, lit)| lit)
    });
    if let Some(lit) = under {
        return Refresh::at(lit.hz, Source::Monitor(lit.crtc));
    }
    if let Some(before) =
        before.filter(|before| matches!(before.source, Source::Monitor(_) | Source::Kept))
    {
        return Refresh {
            period: before.period,
            source: Source::Kept,
        };
    }
    if let Some(hz) = monitors.lit.iter().map(|lit| lit.hz).max_by(f64::total_cmp) {
        return Refresh::at(hz, Source::FastestMonitor);
    }
    match monitors.fastest_mode {
        Some(hz) => Refresh::at(hz, Source::FastestMode),
        None => Refresh::at(DEFAULT_HZ, Source::Default),
    }
}

/// The refresh for `window` by what RandR says now, given the one it had,
/// `before`. A change is logged with the target `silent::frames`.
pub(crate) fn window_refresh<C: RequestConnection>(
    xcb: &C,
    root: xproto::Window,
    window: xproto::Window,
    before: Option<Refresh>,
) -> Refresh {
    let geometry = xcb.get_geometry(window).ok();
    let origin = xcb.translate_coordinates(window, root, 0, 0).ok();
    let resources = xcb.randr_get_screen_resources_current(root).ok();
    let rect = match (geometry, origin) {
        (Some(geometry), Some(origin)) => {
            geometry
                .reply()
                .ok()
                .zip(origin.reply().ok())
                .map(|(geometry, origin)| Rect {
                    x: origin.dst_x.into(),
                    y: origin.dst_y.into(),
                    width: geometry.width.into(),
                    height: geometry.height.into(),
                })
        }
        _ => None,
    };
    let monitors = match resources.and_then(|cookie| cookie.reply().ok()) {
        Some(resources) => {
            let cookies: Vec<_> = resources
                .crtcs
                .iter()
                .map(|&crtc| (crtc, xcb.randr_get_crtc_info(crtc, x11rb::CURRENT_TIME)))
                .collect();
            let crtcs: Vec<_> = cookies
                .into_iter()
                .filter_map(|(crtc, cookie)| Some((crtc, cookie.ok()?.reply().ok()?)))
                .collect();
            Monitors::new(&resources, &crtcs)
        }
        None => Monitors::default(),
    };
    let refresh = choose(rect, &monitors, before);
    if Some(refresh) != before && log::log_enabled!(target: FRAMES, log::Level::Debug) {
        let place = rect.map_or("somewhere unknown".to_string(), |rect| rect.to_string());
        let source = match refresh.source {
            Source::Monitor(crtc) => format!(
                "the current mode of its monitor{}",
                monitor_name(xcb, crtc).map_or(String::new(), |name| format!(", {name}"))
            ),
            Source::Kept => "the rate it had, while it lies over no lit monitor".to_string(),
            Source::FastestMonitor => {
                "the fastest lit monitor's, while it lies over none".to_string()
            }
            Source::FastestMode => {
                "the fastest mode RandR lists, while no monitor is lit".to_string()
            }
            Source::Default => "a default, as RandR lists no mode".to_string(),
        };
        log::debug!(
            target: FRAMES,
            "window {window:#x} at {place}: {:.2} Hz, a frame every {:.3} ms, from {source}",
            refresh.hz(),
            refresh.period.as_secs_f64() * 1000.0,
        );
    }
    refresh
}

/// The name of the output `crtc` drives, as RandR gives it.
fn monitor_name<C: RequestConnection>(xcb: &C, crtc: randr::Crtc) -> Option<String> {
    let info = xcb
        .randr_get_crtc_info(crtc, x11rb::CURRENT_TIME)
        .ok()?
        .reply()
        .ok()?;
    let output = xcb
        .randr_get_output_info(*info.outputs.first()?, x11rb::CURRENT_TIME)
        .ok()?
        .reply()
        .ok()?;
    Some(String::from_utf8_lossy(&output.name).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, width: i32) -> Rect {
        Rect {
            x,
            y: 0,
            width,
            height: 1080,
        }
    }

    fn lit(crtc: randr::Crtc, x: i32, hz: f64) -> Lit {
        Lit {
            crtc,
            rect: rect(x, 1920),
            hz,
        }
    }

    fn monitors(lit: &[Lit], fastest_mode: Option<f64>) -> Monitors {
        Monitors {
            lit: lit.to_vec(),
            fastest_mode,
        }
    }

    fn mode(dot_clock: u32, htotal: u16, vtotal: u16) -> randr::ModeInfo {
        randr::ModeInfo {
            dot_clock,
            htotal,
            vtotal,
            ..Default::default()
        }
    }

    /// A mode RandR lists, with its id.
    fn listed(id: randr::Mode, dot_clock: u32, htotal: u16, vtotal: u16) -> randr::ModeInfo {
        randr::ModeInfo {
            id,
            ..mode(dot_clock, htotal, vtotal)
        }
    }

    /// A CRTC showing `mode` at `x`, 1920 wide, or none with mode 0.
    fn crtc(x: i16, mode: randr::Mode) -> randr::GetCrtcInfoReply {
        randr::GetCrtcInfoReply {
            x,
            y: 0,
            width: if mode == 0 { 0 } else { 1920 },
            height: if mode == 0 { 0 } else { 1080 },
            mode,
            ..Default::default()
        }
    }

    fn resources(modes: Vec<randr::ModeInfo>) -> randr::GetScreenResourcesCurrentReply {
        randr::GetScreenResourcesCurrentReply {
            modes,
            ..Default::default()
        }
    }

    // A 240 Hz monitor's mode, and a 60 Hz one's.
    const FAST: randr::Mode = 0x41;
    const SLOW: randr::Mode = 0x42;

    fn both_modes() -> Vec<randr::ModeInfo> {
        vec![
            listed(SLOW, 148_350_000, 2200, 1125),
            listed(FAST, 2_331_750_000, 4000, 2429),
        ]
    }

    #[test]
    fn a_mode_s_rate_is_its_clock_over_its_pixels() {
        let hz = mode_hz(&mode(2_331_750_000, 4000, 2429)).unwrap();
        assert!((hz - 239.99).abs() < 0.01, "{hz}");
        assert_eq!(mode_hz(&mode(0, 0, 0)), None);
        let interlaced = randr::ModeInfo {
            mode_flags: randr::ModeFlag::INTERLACE,
            ..mode(74_250_000, 2200, 1125)
        };
        assert!((mode_hz(&interlaced).unwrap() - 60.0).abs() < 0.01);
        let doubled = randr::ModeInfo {
            mode_flags: randr::ModeFlag::DOUBLE_SCAN,
            ..mode(25_175_000, 800, 525)
        };
        assert!((mode_hz(&doubled).unwrap() - 29.97).abs() < 0.01);
    }

    #[test]
    fn a_crtc_is_lit_by_a_mode_randr_lists() {
        let resources = resources(both_modes());
        let crtcs = [
            (1, crtc(0, 0)),
            (2, crtc(0, 0x99)),
            (3, crtc(1920, SLOW)),
            (4, crtc(0, FAST)),
        ];
        let monitors = Monitors::new(&resources, &crtcs);
        let lit: Vec<_> = monitors.lit.iter().map(|lit| lit.crtc).collect();
        assert_eq!(lit, [3, 4]);
        assert_eq!(monitors.lit[0].rect, rect(1920, 1920));
        assert!((monitors.lit[1].hz - 239.99).abs() < 0.01);
        assert!((monitors.fastest_mode.unwrap() - 239.99).abs() < 0.01);
    }

    #[test]
    fn each_window_takes_the_monitor_it_lies_over_most() {
        let monitors = monitors(&[lit(1, 0, 239.99), lit(2, 1920, 59.94)], None);
        let left = choose(Some(rect(0, 1920)), &monitors, None);
        assert_eq!(left.source, Source::Monitor(1));
        assert!((left.hz() - 239.99).abs() < 1e-3);
        let right = choose(Some(rect(1920, 1920)), &monitors, None);
        assert_eq!(right.source, Source::Monitor(2));
        let mostly_right = choose(Some(rect(1000, 1920)), &monitors, None);
        assert_eq!(mostly_right.source, Source::Monitor(2));
    }

    #[test]
    fn a_window_takes_its_own_monitor_not_the_first_listed() {
        // The slow monitor comes first, as it may in RandR's list, and the
        // window lies on the fast one.
        let resources = resources(both_modes());
        let crtcs = [(7, crtc(1920, SLOW)), (8, crtc(0, FAST))];
        let monitors = Monitors::new(&resources, &crtcs);
        let refresh = choose(Some(rect(400, 1100)), &monitors, None);
        assert_eq!(refresh.source, Source::Monitor(8));
        assert!((refresh.hz() - 239.99).abs() < 0.01);
    }

    #[test]
    fn of_mirrored_monitors_the_faster_counts() {
        let monitors = monitors(&[lit(1, 0, 59.94), lit(2, 0, 143.99)], None);
        let refresh = choose(Some(rect(0, 1920)), &monitors, None);
        assert_eq!(refresh.source, Source::Monitor(2));
        let resources = resources(both_modes());
        let mirrored = Monitors::new(&resources, &[(1, crtc(0, SLOW)), (2, crtc(0, FAST))]);
        let refresh = choose(Some(rect(500, 900)), &mirrored, None);
        assert_eq!(refresh.source, Source::Monitor(2));
    }

    #[test]
    fn a_window_whose_monitor_sleeps_keeps_its_rate_until_it_wakes() {
        let awake = monitors(&[lit(1, 0, 239.99)], None);
        let before = choose(Some(rect(0, 1920)), &awake, None);
        // Asleep, the monitor is listed disconnected with no modes, its
        // CRTC off.
        let asleep = Monitors::new(&resources(Vec::new()), &[(1, crtc(0, 0))]);
        let kept = choose(Some(rect(0, 1920)), &asleep, Some(before));
        assert_eq!(kept.source, Source::Kept);
        assert_eq!(kept.period, before.period);
        let still = choose(Some(rect(0, 1920)), &monitors(&[], Some(60.0)), Some(kept));
        assert_eq!(still, kept);
        let woke = choose(Some(rect(0, 1920)), &awake, Some(still));
        assert_eq!(woke, before);
    }

    #[test]
    fn a_window_first_shown_while_its_monitor_slept_takes_its_rate_once_it_wakes() {
        let asleep = Monitors::new(&resources(Vec::new()), &[(1, crtc(0, 0))]);
        let first = choose(Some(rect(0, 1920)), &asleep, None);
        assert_eq!(first.source, Source::Default);
        assert!((first.hz() - 60.0).abs() < 1e-3);
        let resources = resources(both_modes());
        let awake = Monitors::new(&resources, &[(1, crtc(0, FAST))]);
        let woke = choose(Some(rect(0, 1920)), &awake, Some(first));
        assert_eq!(woke.source, Source::Monitor(1));
        assert!((woke.hz() - 239.99).abs() < 0.01);
    }

    #[test]
    fn a_monitor_s_new_mode_sets_the_pace() {
        let resources = resources(both_modes());
        let fast = Monitors::new(&resources, &[(1, crtc(0, FAST))]);
        let before = choose(Some(rect(0, 1920)), &fast, None);
        let slow = Monitors::new(&resources, &[(1, crtc(0, SLOW))]);
        let after = choose(Some(rect(0, 1920)), &slow, Some(before));
        assert_eq!(after.source, Source::Monitor(1));
        assert!((after.hz() - 59.94).abs() < 0.01);
    }

    #[test]
    fn with_nothing_known_the_fastest_there_is_counts() {
        let two = monitors(&[lit(1, 0, 59.94), lit(2, 1920, 143.99)], None);
        let off = choose(Some(rect(5000, 100)), &two, None);
        assert_eq!(off.source, Source::FastestMonitor);
        assert!((off.hz() - 143.99).abs() < 1e-3);
        let unlit = choose(None, &monitors(&[], Some(239.99)), None);
        assert_eq!(unlit.source, Source::FastestMode);
        let unknown = choose(None, &Monitors::default(), None);
        assert_eq!(unknown.source, Source::Default);
        assert!((unknown.hz() - 60.0).abs() < 1e-3);
        // A fallback is no rate to keep: the next look takes what is there.
        let woke = choose(
            Some(rect(0, 1920)),
            &monitors(&[lit(1, 0, 239.99)], None),
            Some(unknown),
        );
        assert_eq!(woke.source, Source::Monitor(1));
    }
}
