//! The `org.gnome.NautilusPreviewer2` service that Nautilus calls when the
//! user presses Space.
//!
//! zbus serves it from its own thread; calls reach the UI as [`Request`]s
//! over a channel, and the UI answers through [`Bus`]. Whoever calls the
//! previewer, Nautilus's ping as it starts among them, is watched until it
//! leaves the bus, so that the previewer can live as long as its callers.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use zbus::blocking::Connection;
use zbus::blocking::fdo::DBusProxy;
use zbus::blocking::object_server::InterfaceRef;
use zbus::message::Type;
use zbus::names::BusName;
use zbus::object_server::SignalEmitter;

pub const NAME: &str = "org.gnome.NautilusPreviewer";
pub const PATH: &str = "/org/gnome/NautilusPreviewer";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Show {
        uri: String,
        /// `x11:<hex xid>` of the Nautilus window, or `x11:0`.
        window_handle: String,
        close_if_shown: bool,
        activation_token: String,
    },
    Close,
    /// How many of the previewer's callers are still on the bus.
    Callers(usize),
}

/// `GtkDirectionType` values, which Nautilus expects in `SelectionEvent`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Up = 2,
    Down = 3,
    Left = 4,
    Right = 5,
}

struct Previewer {
    requests: async_channel::Sender<Request>,
    visible: bool,
    parent_handle: String,
}

#[zbus::interface(name = "org.gnome.NautilusPreviewer2")]
impl Previewer {
    fn show_file(
        &self,
        uri: String,
        window_handle: String,
        close_if_already_shown: bool,
        activation_token: String,
    ) {
        log::info!(target: "silent::trace", "show file");
        let _ = self.requests.try_send(Request::Show {
            uri,
            window_handle,
            close_if_shown: close_if_already_shown,
            activation_token,
        });
    }

    fn close(&self) {
        let _ = self.requests.try_send(Request::Close);
    }

    #[zbus(property)]
    fn visible(&self) -> bool {
        self.visible
    }

    #[zbus(property)]
    fn parent_handle(&self) -> String {
        self.parent_handle.clone()
    }

    #[zbus(signal)]
    async fn selection_event(emitter: &SignalEmitter<'_>, direction: u32) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn navigate_to(emitter: &SignalEmitter<'_>, uri: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn rename(emitter: &SignalEmitter<'_>, uri: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn trash(emitter: &SignalEmitter<'_>, uri: &str) -> zbus::Result<()>;
}

/// Counts the callers still on the bus: the senders of method calls to this
/// connection, Peer's Ping included, which zbus answers itself, until the
/// bus says they have left.
fn watch_callers(conn: &Connection, requests: async_channel::Sender<Request>) -> zbus::Result<()> {
    let callers = Arc::new(Mutex::new(HashSet::<String>::new()));
    let bus = DBusProxy::new(conn)?;
    // Departures are watched first, so that none goes unseen.
    let departures = bus.receive_name_owner_changed()?;
    let (gone, left) = (callers.clone(), requests.clone());
    std::thread::Builder::new()
        .name("callers left".into())
        .spawn(move || {
            for signal in departures {
                let Ok(args) = signal.args() else { continue };
                if args.new_owner().is_none() && gone.lock().unwrap().remove(args.name().as_str()) {
                    let count = gone.lock().unwrap().len();
                    let _ = left.send_blocking(Request::Callers(count));
                }
            }
        })
        .map_err(|e| zbus::Error::Failure(e.to_string()))?;
    let calls = zbus::blocking::MessageIterator::from(conn);
    std::thread::Builder::new()
        .name("callers".into())
        .spawn(move || {
            for message in calls.flatten() {
                let header = message.header();
                if header.message_type() != Type::MethodCall {
                    continue;
                }
                let Some(sender) = header.sender() else { continue };
                let sender = sender.to_owned();
                let count = {
                    let mut known = callers.lock().unwrap();
                    if !known.insert(sender.to_string()) {
                        continue;
                    }
                    known.len()
                };
                let _ = requests.send_blocking(Request::Callers(count));
                // A caller may have left before it was counted, as a script
                // does. The bus is asked on a thread of its own: this one must
                // keep reading, or the answer could not come in.
                let (bus, callers, requests) = (bus.clone(), callers.clone(), requests.clone());
                let _ = std::thread::Builder::new()
                    .name("caller here".into())
                    .spawn(move || {
                        let here = bus.name_has_owner(BusName::from(sender.clone())).unwrap_or(false);
                        let mut known = callers.lock().unwrap();
                        if !here && known.remove(sender.as_str()) {
                            let _ = requests.send_blocking(Request::Callers(known.len()));
                        }
                    });
            }
            // The bus has gone, and nobody can call again.
            let _ = requests.send_blocking(Request::Callers(0));
        })
        .map_err(|e| zbus::Error::Failure(e.to_string()))?;
    Ok(())
}

/// The session-bus connection that owns the previewer's name.
#[derive(Clone)]
pub struct Bus {
    conn: Connection,
}

impl Bus {
    /// Exports the previewer and watches for callers, then claims its name,
    /// so that the call which activated this process finds the object
    /// waiting, and its caller is seen.
    pub fn start(requests: async_channel::Sender<Request>) -> zbus::Result<Self> {
        let previewer = Previewer {
            requests: requests.clone(),
            visible: false,
            parent_handle: String::new(),
        };
        let conn = zbus::blocking::connection::Builder::session()?
            .serve_at(PATH, previewer)?
            .build()?;
        watch_callers(&conn, requests)?;
        conn.request_name(NAME)?;
        Ok(Self { conn })
    }

    fn previewer(&self) -> zbus::Result<InterfaceRef<Previewer>> {
        self.conn.object_server().interface(PATH)
    }

    pub fn set_visible(&self, visible: bool) -> zbus::Result<()> {
        let previewer = self.previewer()?;
        let changed = std::mem::replace(&mut previewer.get_mut().visible, visible) != visible;
        if changed {
            zbus::block_on(previewer.get().visible_changed(previewer.signal_emitter()))?;
        }
        Ok(())
    }

    pub fn set_parent_handle(&self, handle: &str) -> zbus::Result<()> {
        let previewer = self.previewer()?;
        let changed = {
            let mut state = previewer.get_mut();
            let changed = state.parent_handle != handle;
            state.parent_handle = handle.to_owned();
            changed
        };
        if changed {
            zbus::block_on(
                previewer
                    .get()
                    .parent_handle_changed(previewer.signal_emitter()),
            )?;
        }
        Ok(())
    }

    /// Asks Nautilus to move its selection; it answers with a new `ShowFile`.
    pub fn selection_event(&self, direction: Direction) -> zbus::Result<()> {
        let previewer = self.previewer()?;
        zbus::block_on(Previewer::selection_event(
            previewer.signal_emitter(),
            direction as u32,
        ))
    }

    pub fn navigate_to(&self, uri: &str) -> zbus::Result<()> {
        let previewer = self.previewer()?;
        zbus::block_on(Previewer::navigate_to(previewer.signal_emitter(), uri))
    }

    pub fn rename(&self, uri: &str) -> zbus::Result<()> {
        let previewer = self.previewer()?;
        zbus::block_on(Previewer::rename(previewer.signal_emitter(), uri))
    }

    pub fn trash(&self, uri: &str) -> zbus::Result<()> {
        let previewer = self.previewer()?;
        zbus::block_on(Previewer::trash(previewer.signal_emitter(), uri))
    }

    /// Gives up the name before the process exits, so that the next call
    /// activates a fresh process instead of reaching this one.
    pub fn release(&self) -> zbus::Result<()> {
        self.conn.release_name(NAME)?;
        Ok(())
    }

    /// Takes the name back when a call slipped in while it was being released.
    pub fn reclaim(&self) -> zbus::Result<()> {
        self.conn.request_name(NAME)
    }
}
