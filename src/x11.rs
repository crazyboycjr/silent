//! What the toolkits cannot do on X11: read the monitors and the desktop
//! scale before any window exists, place a window, make it transient for
//! Nautilus.

use std::error::Error;

use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::{AtomEnum, ConfigureWindowAux, ConnectionExt as _, PropMode};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use crate::geometry::{Rect, Screen, monitor_at};

pub struct X11 {
    conn: RustConnection,
    root: u32,
}

impl X11 {
    pub fn connect() -> Result<Self, Box<dyn Error>> {
        let (conn, screen) = x11rb::connect(None)?;
        let root = conn.setup().roots[screen].root;
        Ok(Self { conn, root })
    }

    /// The monitors, the primary first.
    pub fn monitors(&self) -> Vec<Rect> {
        let reply = self
            .conn
            .randr_get_monitors(self.root, true)
            .ok()
            .and_then(|cookie| cookie.reply().ok());
        let mut monitors: Vec<_> = reply
            .map(|r| r.monitors)
            .unwrap_or_default()
            .into_iter()
            .map(|m| {
                let rect = Rect {
                    x: m.x.into(),
                    y: m.y.into(),
                    width: m.width.into(),
                    height: m.height.into(),
                };
                (!m.primary, rect)
            })
            .collect();
        monitors.sort_by_key(|(secondary, _)| *secondary);
        if monitors.is_empty() {
            let root = &self.conn.setup().roots[0];
            let whole = Rect {
                x: 0,
                y: 0,
                width: root.width_in_pixels.into(),
                height: root.height_in_pixels.into(),
            };
            return vec![whole];
        }
        monitors.into_iter().map(|(_, rect)| rect).collect()
    }

    /// `Xft.dpi` / 96, or 1 when the desktop sets no DPI.
    pub fn desktop_scale(&self) -> f64 {
        self.conn
            .get_property(
                false,
                self.root,
                AtomEnum::RESOURCE_MANAGER,
                AtomEnum::STRING,
                0,
                1 << 16,
            )
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .and_then(|reply| xft_dpi(&reply.value))
            .map_or(1.0, |dpi| dpi / 96.0)
    }

    pub fn pointer(&self) -> Option<(i32, i32)> {
        let reply = self.conn.query_pointer(self.root).ok()?.reply().ok()?;
        Some((reply.root_x.into(), reply.root_y.into()))
    }

    /// Where `window` is, in root coordinates.
    pub fn frame(&self, window: u32) -> Option<Rect> {
        let geometry = self.conn.get_geometry(window).ok()?.reply().ok()?;
        let origin = self
            .conn
            .translate_coordinates(window, self.root, 0, 0)
            .ok()?
            .reply()
            .ok()?;
        Some(Rect {
            x: origin.dst_x.into(),
            y: origin.dst_y.into(),
            width: geometry.width.into(),
            height: geometry.height.into(),
        })
    }

    /// The screen a preview opens on: the parent window's monitor, else the
    /// pointer's, else the primary.
    pub fn screen_for(&self, parent: Option<u32>) -> Screen {
        let monitors = self.monitors();
        let point = parent
            .and_then(|window| self.frame(window))
            .map(|frame| frame.centre())
            .or_else(|| self.pointer());
        let monitor = point
            .and_then(|point| monitor_at(&monitors, point))
            .unwrap_or(monitors[0]);
        Screen {
            monitor,
            scale: self.desktop_scale(),
        }
    }

    pub fn set_transient_for(&self, window: u32, parent: u32) -> Result<(), Box<dyn Error>> {
        self.conn.change_property32(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_TRANSIENT_FOR,
            AtomEnum::WINDOW,
            &[parent],
        )?;
        self.conn.flush()?;
        Ok(())
    }

    /// Moves and resizes `window`. Window managers apply this to floating
    /// windows, which is how a reused preview stays centred.
    pub fn configure(&self, window: u32, rect: Rect) -> Result<(), Box<dyn Error>> {
        let aux = ConfigureWindowAux::new()
            .x(rect.x)
            .y(rect.y)
            .width(rect.width.max(1) as u32)
            .height(rect.height.max(1) as u32);
        self.conn.configure_window(window, &aux)?;
        self.conn.flush()?;
        Ok(())
    }
}

/// The window an `x11:<hex>` handle names; none for `x11:0` or other systems.
pub fn parse_handle(handle: &str) -> Option<u32> {
    let hex = handle.strip_prefix("x11:")?;
    u32::from_str_radix(hex.trim_start_matches("0x"), 16)
        .ok()
        .filter(|&xid| xid != 0)
}

fn xft_dpi(resources: &[u8]) -> Option<f64> {
    String::from_utf8_lossy(resources).lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        if key.trim() != "Xft.dpi" {
            return None;
        }
        value.trim().parse().ok().filter(|dpi: &f64| *dpi > 0.0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_name_x11_windows() {
        assert_eq!(parse_handle("x11:3a00004"), Some(0x3a00004));
        assert_eq!(parse_handle("x11:0x3a00004"), Some(0x3a00004));
        assert_eq!(parse_handle("x11:0"), None);
        assert_eq!(parse_handle("wayland:abc"), None);
        assert_eq!(parse_handle(""), None);
    }

    #[test]
    fn the_dpi_comes_from_the_resource_database() {
        let db = b"Xcursor.size:\t48\nXft.dpi:\t163\nXft.antialias:\t1\n";
        assert_eq!(xft_dpi(db), Some(163.0));
        assert_eq!(xft_dpi(b"Xft.dpi: 96.5"), Some(96.5));
        assert_eq!(xft_dpi(b"Xcursor.size: 48"), None);
        assert_eq!(xft_dpi(b"Xft.dpi: 0"), None);
    }
}
