//! The `org.gnome.NautilusPreviewer2` service that Nautilus calls when the
//! user presses Space, and beside it silent's own interface for everything
//! else that previews, such as yazi, through `silent show` and its kin.
//!
//! zbus serves both from its own thread; calls reach the UI as [`Request`]s
//! over a channel, and the UI answers through [`Bus`]. Whoever calls the
//! previewer, Nautilus's ping as it starts among them, is watched until it
//! leaves the bus, and the process each client preview is for until it
//! ends, so that the previewer can live as long as its callers.

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
/// silent's own interface, on the same object: calling it through the
/// previewer's name starts silent the way Nautilus's calls do.
pub const CLIENT: &str = "host.cjr.Silent1";
/// Nemo's previewer, nemo-preview's name, which silent answers as well.
pub const NEMO: &str = "org.nemo.Preview";
pub const NEMO_PATH: &str = "/org/nemo/Preview";

/// The file manager that calls a previewer interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Manager {
    Nautilus,
    /// Nemo, which hears nothing back: no selection moves, no renames.
    Nemo,
}

#[derive(Clone, Debug)]
pub enum Request {
    /// Nautilus's or Nemo's ShowFile.
    Show {
        uri: String,
        /// `x11:<hex xid>` of the file manager's window, or `x11:0`.
        window_handle: String,
        close_if_shown: bool,
        activation_token: String,
        from: Manager,
    },
    /// A client's call, about the preview of process `owner`; `answer` is
    /// told whether that preview is shown once the call is done.
    Ask {
        ask: Ask,
        uri: String,
        /// The terminal's window, `x11:<hex xid>`, or empty.
        window_handle: String,
        owner: u32,
        /// Whether the preview takes the keyboard, as Nautilus's does.
        focus: bool,
        answer: async_channel::Sender<bool>,
    },
    /// Nautilus's or Nemo's Close.
    Close(Manager),
    /// How many of the previewer's callers are still on the bus.
    Callers(usize),
    /// How many processes client previews are for are still running.
    Owners(usize),
    /// The process a client preview was for has ended.
    Gone(u32),
    /// The executable on disk is not this process's any more: a new version
    /// was installed, or this one removed.
    Replaced,
}

/// What a client asks of the preview it owns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    /// Show the file, opening the preview if it is not open.
    Show,
    /// Close the preview if it is open, else show the file.
    Toggle,
    /// Show the file only if the preview is open, as a cursor moves.
    Follow,
    Close,
    /// Nothing shown: silent starts, if it has not, and stays for the owner.
    Warm,
}

impl Ask {
    pub const ALL: [(&'static str, Ask); 5] = [
        ("show", Ask::Show),
        ("toggle", Ask::Toggle),
        ("follow", Ask::Follow),
        ("close", Ask::Close),
        ("warm", Ask::Warm),
    ];

    pub fn parse(word: &str) -> Option<Ask> {
        Self::ALL
            .iter()
            .find(|(name, _)| *name == word)
            .map(|(_, ask)| *ask)
    }

    pub fn name(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(_, ask)| *ask == self)
            .map_or("", |(name, _)| name)
    }

    /// Whether it is about a file.
    pub fn takes_file(self) -> bool {
        matches!(self, Ask::Show | Ask::Toggle | Ask::Follow)
    }
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
            from: Manager::Nautilus,
        });
    }

    fn close(&self) {
        let _ = self.requests.try_send(Request::Close(Manager::Nautilus));
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

/// What nemo-preview serves Nemo: the first previewer interface sushi had,
/// ShowFile with the window as a number.
struct Nemo {
    requests: async_channel::Sender<Request>,
}

#[zbus::interface(name = "org.nemo.Preview")]
impl Nemo {
    fn show_file(&self, uri: String, xid: i32, close_if_already_visible: bool) {
        let _ = self.requests.try_send(Request::Show {
            uri,
            window_handle: format!("x11:{:x}", xid as u32),
            close_if_shown: close_if_already_visible,
            activation_token: String::new(),
            from: Manager::Nemo,
        });
    }

    fn close(&self) {
        let _ = self.requests.try_send(Request::Close(Manager::Nemo));
    }
}

struct Client {
    requests: async_channel::Sender<Request>,
    /// The processes client previews are for, while they run.
    owners: Arc<Mutex<HashSet<u32>>>,
}

#[zbus::interface(name = "host.cjr.Silent1")]
impl Client {
    /// `ask` is show, toggle, follow, close or warm; `uri` the file, empty
    /// for close and warm; `owner` the process the preview is for, which
    /// keeps silent running while it runs; `focus` whether the preview takes
    /// the keyboard. Returns whether `owner`'s preview is shown afterwards.
    async fn ask(
        &self,
        ask: &str,
        uri: String,
        owner: u32,
        focus: bool,
        window_handle: String,
    ) -> zbus::fdo::Result<bool> {
        let Some(ask) = Ask::parse(ask) else {
            return Err(zbus::fdo::Error::InvalidArgs(format!(
                "no such request: {ask}"
            )));
        };
        if ask.takes_file() && uri.is_empty() {
            return Err(zbus::fdo::Error::InvalidArgs("no file".into()));
        }
        let (answer, answered) = async_channel::bounded(1);
        let request = Request::Ask {
            ask,
            uri,
            window_handle,
            owner,
            focus,
            answer,
        };
        // Sent before the owner is watched: if it has ended already, its
        // preview is closed after it opens, not left open for nobody.
        if self.requests.send(request).await.is_err() {
            return Err(zbus::fdo::Error::Failed("silent is leaving".into()));
        }
        watch_owner(owner, &self.owners, &self.requests);
        Ok(answered.recv().await.unwrap_or(false))
    }
}

/// Watches `pid` until it ends, through a pidfd, unless it is watched
/// already; the count of owners goes to the UI, under the lock, so that the
/// last count it reads is the true one.
fn watch_owner(
    pid: u32,
    owners: &Arc<Mutex<HashSet<u32>>>,
    requests: &async_channel::Sender<Request>,
) {
    let mut known = owners.lock().unwrap();
    if pid == 0 || known.contains(&pid) {
        return;
    }
    // SAFETY: a plain system call; the descriptor is this thread's to close.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) } as libc::c_int;
    if fd < 0 {
        // Ended already.
        let _ = requests.send_blocking(Request::Gone(pid));
        return;
    }
    known.insert(pid);
    let _ = requests.send_blocking(Request::Owners(known.len()));
    let (watched_owners, gone) = (owners.clone(), requests.clone());
    let watched = std::thread::Builder::new()
        .name("owner".into())
        .spawn(move || {
            let mut ended = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one pollfd, owned here; the descriptor is closed once.
            unsafe {
                while libc::poll(&mut ended, 1, -1) < 0
                    && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
                {
                }
                libc::close(fd);
            }
            let mut known = watched_owners.lock().unwrap();
            known.remove(&pid);
            let _ = gone.send_blocking(Request::Gone(pid));
            let _ = gone.send_blocking(Request::Owners(known.len()));
        });
    if watched.is_err() {
        known.remove(&pid);
        let _ = requests.send_blocking(Request::Owners(known.len()));
    }
}

/// Asks the previewer on the session bus, which the bus starts for the
/// call if it is not running, for `ask` about `owner`'s preview; whether
/// that preview is shown afterwards.
pub fn ask(
    ask: Ask,
    uri: &str,
    owner: u32,
    focus: bool,
    window_handle: &str,
) -> zbus::Result<bool> {
    let conn = Connection::session()?;
    let reply = conn.call_method(
        Some(NAME),
        PATH,
        Some(CLIENT),
        "Ask",
        &(ask.name(), uri, owner, focus, window_handle),
    )?;
    reply.body().deserialize()
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
                let Some(sender) = header.sender() else {
                    continue;
                };
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
                        let here = bus
                            .name_has_owner(BusName::from(sender.clone()))
                            .unwrap_or(false);
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
        let client = Client {
            requests: requests.clone(),
            owners: Arc::default(),
        };
        let nemo = Nemo {
            requests: requests.clone(),
        };
        let conn = zbus::blocking::connection::Builder::session()?
            .serve_at(PATH, previewer)?
            .serve_at(PATH, client)?
            .serve_at(NEMO_PATH, nemo)?
            .build()?;
        watch_callers(&conn, requests)?;
        conn.request_name(NAME)?;
        // nemo-preview may hold it; Nemo is not silent's to insist on.
        let _ = conn.request_name(NEMO);
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
        let _ = self.conn.release_name(NEMO);
        self.conn.release_name(NAME)?;
        Ok(())
    }

    /// Takes the name back when a call slipped in while it was being released.
    pub fn reclaim(&self) -> zbus::Result<()> {
        let _ = self.conn.request_name(NEMO);
        self.conn.request_name(NAME)
    }
}
