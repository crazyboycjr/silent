//! What the toolkits cannot do on X11: read the monitors and the desktop
//! scale before any window exists, place a window, make it transient for
//! Nautilus, say whether it takes the keyboard.

use std::error::Error;

use x11rb::connection::Connection;
use x11rb::properties::WmHints;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConfigureWindowAux, ConnectionExt as _, EventMask, InputFocus,
    PropMode, UNMAP_NOTIFY_EVENT, UnmapNotifyEvent,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use crate::geometry::{Rect, Screen, monitor_at};

pub struct X11 {
    conn: RustConnection,
    root: u32,
    screen: usize,
}

impl X11 {
    pub fn connect() -> Result<Self, Box<dyn Error>> {
        let (conn, screen) = x11rb::connect(None)?;
        let root = conn.setup().roots[screen].root;
        Ok(Self { conn, root, screen })
    }

    fn atom(&self, name: &str) -> Option<u32> {
        Some(
            self.conn
                .intern_atom(false, name.as_bytes())
                .ok()?
                .reply()
                .ok()?
                .atom,
        )
    }

    /// Whether a compositor runs, so that transparent pixels show what is
    /// behind the window rather than black.
    pub fn compositing(&self) -> bool {
        self.atom(&format!("_NET_WM_CM_S{}", self.screen))
            .and_then(|atom| self.conn.get_selection_owner(atom).ok()?.reply().ok())
            .is_some_and(|reply| reply.owner != x11rb::NONE)
    }

    /// Hands on the activation token Nautilus passed: the window carries it
    /// for focus-stealing prevention, and the launch it began is completed.
    pub fn complete_startup(&self, window: u32, token: &str) -> Result<(), Box<dyn Error>> {
        if token.is_empty() {
            return Ok(());
        }
        let utf8 = self.atom("UTF8_STRING").ok_or("no UTF8_STRING")?;
        let startup_id = self.atom("_NET_STARTUP_ID").ok_or("no _NET_STARTUP_ID")?;
        self.conn.change_property8(
            PropMode::REPLACE,
            window,
            startup_id,
            utf8,
            token.as_bytes(),
        )?;
        let begin = self.atom("_NET_STARTUP_INFO_BEGIN").ok_or("no atom")?;
        let more = self.atom("_NET_STARTUP_INFO").ok_or("no atom")?;
        let escaped = token.replace('\\', "\\\\").replace('"', "\\\"");
        let mut message = format!("remove: ID=\"{escaped}\"").into_bytes();
        message.push(0);
        // The startup-notification protocol sends text in 20-byte client messages.
        for (i, chunk) in message.chunks(20).enumerate() {
            let mut data = [0u8; 20];
            data[..chunk.len()].copy_from_slice(chunk);
            let event = ClientMessageEvent::new(8, window, if i == 0 { begin } else { more }, data);
            self.conn
                .send_event(false, self.root, EventMask::PROPERTY_CHANGE, event)?;
        }
        self.conn.flush()?;
        Ok(())
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

    /// Asks the window manager for no title bar and no frame.
    pub fn undecorate(&self, window: u32) -> Result<(), Box<dyn Error>> {
        let hints = self.atom("_MOTIF_WM_HINTS").ok_or("no _MOTIF_WM_HINTS")?;
        // flags = decorations, functions = 0, decorations = none, input mode, status
        self.conn
            .change_property32(PropMode::REPLACE, window, hints, hints, &[2, 0, 0, 0, 0])?;
        self.conn.flush()?;
        Ok(())
    }

    /// ICCCM's input hint: whether the window manager gives `window` the
    /// keyboard. A preview opened from a terminal says no, and the keys stay
    /// in the terminal; xmonad, among others, then focuses it only in name.
    pub fn set_input_hint(&self, window: u32, input: bool) -> Result<(), Box<dyn Error>> {
        let mut hints = WmHints::get(&self.conn, window)?
            .reply()?
            .unwrap_or_default();
        hints.input = Some(input);
        hints.set(&self.conn, window)?;
        self.conn.flush()?;
        Ok(())
    }

    /// Gives the shown `window` the keyboard: asked of the window manager,
    /// and set directly for one that would not answer.
    pub fn focus(&self, window: u32) -> Result<(), Box<dyn Error>> {
        let active = self
            .atom("_NET_ACTIVE_WINDOW")
            .ok_or("no _NET_ACTIVE_WINDOW")?;
        // Source 1, an application; the current time.
        let event = ClientMessageEvent::new(32, window, active, [1u32, 0, 0, 0, 0]);
        self.conn.send_event(
            false,
            self.root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            event,
        )?;
        self.conn
            .set_input_focus(InputFocus::PARENT, window, x11rb::CURRENT_TIME)?;
        self.conn.flush()?;
        Ok(())
    }

    /// Whether `window` exists.
    pub fn exists(&self, window: u32) -> bool {
        self.conn
            .get_window_attributes(window)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some()
    }

    /// Makes `window` transient for `parent`, or for nothing.
    pub fn set_transient_for(
        &self,
        window: u32,
        parent: Option<u32>,
    ) -> Result<(), Box<dyn Error>> {
        match parent {
            Some(parent) => self.conn.change_property32(
                PropMode::REPLACE,
                window,
                AtomEnum::WM_TRANSIENT_FOR,
                AtomEnum::WINDOW,
                &[parent],
            )?,
            None => self
                .conn
                .delete_property(window, AtomEnum::WM_TRANSIENT_FOR.into())?,
        };
        self.conn.flush()?;
        Ok(())
    }

    /// Shows `window`; the window manager manages it afresh, and centres it.
    pub fn map(&self, window: u32) -> Result<(), Box<dyn Error>> {
        self.conn.map_window(window)?;
        self.conn.flush()?;
        Ok(())
    }

    /// Withdraws `window`, as ICCCM asks: unmapped, and a synthetic
    /// UnmapNotify to the root for window managers that reparent.
    pub fn unmap(&self, window: u32) -> Result<(), Box<dyn Error>> {
        self.conn.unmap_window(window)?;
        let event = UnmapNotifyEvent {
            response_type: UNMAP_NOTIFY_EVENT,
            sequence: 0,
            event: self.root,
            window,
            from_configure: false,
        };
        self.conn.send_event(
            false,
            self.root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            event,
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
