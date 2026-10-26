//! The previewer's life: requests from Nautilus, the window, and leaving.
//!
//! Started by D-Bus, the previewer lives as long as its callers stay on the
//! bus: Nautilus pings it as it starts, and it stays until Nautilus quits.
//! Its one window is made ahead of the first file, with its GPU surface,
//! swapchain and pipelines, and is withdrawn rather than destroyed when a
//! preview closes, to be shown again for the next.

use std::time::Duration;

use futures_lite::FutureExt;
use gpui_kit::*;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use silent::dbus::{Bus, Direction, Request};
use silent::detect::Kind;
use silent::extract::{self, Places};
use silent::geometry::{self, Rect, Screen};
use silent::x11::{self, X11};

use crate::content::{self, Loaded};
use crate::icons::Icons;
use crate::trace;
use crate::view::Preview;

pub enum Input {
    Request(Request),
    Intent(Intent),
    Loaded(u64, Box<Loaded>),
    Closed(WindowId),
    /// The GPU device is made: the window for the next file can be.
    Warm,
}

/// What a key in the window asks for.
pub enum Intent {
    Close,
    Select(Direction),
    Open,
    Rename,
    Trash,
    /// The overview's dock grows by this many logical pixels at the
    /// window's right, or shrinks when it is negative.
    Redock(f32),
    /// The window takes this size about its centre, for a file taken out of
    /// an archive and back.
    Fit((i32, i32)),
}

/// How long a released name waits for calls that were already on their way.
const LINGER: Duration = Duration::from_millis(50);
/// How long the previewer waits, with nothing shown and no caller on the
/// bus, before it leaves: a script that called and left may call again.
const IDLE: Duration = Duration::from_secs(3);

struct Shown {
    uri: String,
    window_handle: String,
    activation_token: String,
    folder: bool,
}

struct Open {
    handle: WindowHandle<Preview>,
    xid: Option<u32>,
    size: (i32, i32),
    /// Mapped, showing a file; otherwise withdrawn, waiting for the next.
    shown: bool,
}

struct Controller {
    bus: Option<Bus>,
    x11: Option<X11>,
    input: async_channel::Sender<Input>,
    window: Option<Open>,
    shown: Option<Shown>,
    /// Bumped by every new file and every close, so stale loads are dropped.
    generation: u64,
    loading: bool,
    screen: Screen,
    rounded: bool,
    /// The previewer's callers still on the bus.
    callers: usize,
}

/// Runs the previewer: with a bus, as long as its callers stay on it or a
/// file is shown; without, until its window closes. `first` holds the
/// requests that came before the toolkit started; their loading begins
/// before the toolkit does.
pub fn run(bus: Option<Bus>, requests: async_channel::Receiver<Request>, first: Vec<Request>) {
    // What a crashed preview took out of archives; nothing waits for it.
    let _ = std::thread::Builder::new()
        .name("sweep".into())
        .spawn(|| extract::sweep(&Places::standard()));
    let x11 = X11::connect().ok();
    let rounded = x11.as_ref().is_some_and(X11::compositing);
    let (input, inputs) = async_channel::unbounded();
    let mut controller = Controller {
        bus,
        x11,
        input: input.clone(),
        window: None,
        shown: None,
        generation: 0,
        loading: false,
        screen: Screen {
            monitor: Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            scale: 1.0,
        },
        rounded,
        callers: 0,
    };
    for request in first {
        if let Request::Show { .. } = request {
            controller.request(request, None);
        }
    }
    trace::mark("toolkit starts");
    // The controller leaves on its own terms: after a delayed Rename, and
    // after the calls that were on their way when the name was given up.
    let app = gpui_kit::application()
        .with_quit_mode(QuitMode::Explicit)
        .with_assets(Icons);
    trace::mark("toolkit");
    app.run(move |cx| {
        trace::mark("app runs");
        // Finding the fonts takes the first frame milliseconds; here it is
        // done while the GPU is still starting.
        let text = cx.text_system().clone();
        cx.background_executor()
            .spawn(async move { text.prewarm_fonts(&first_fonts()) })
            .detach();
        let forward = input.clone();
        cx.spawn(async move |_| {
            while let Ok(request) = requests.recv().await {
                if forward.send(Input::Request(request)).await.is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.on_window_closed(move |_, id| {
            let _ = input.try_send(Input::Closed(id));
        })
        .detach();
        cx.spawn(async move |cx| controller.serve(inputs, cx).await)
            .detach();
    });
}

/// The faces a first frame draws in: the strip's and the cards' sans, and
/// code's mono with the styles highlighting gives it.
fn first_fonts() -> Vec<Font> {
    let sans = font("Noto Sans");
    let code = crate::lines::LinesPane::font();
    vec![
        Font {
            features: crate::strip::figures(),
            ..sans.clone()
        },
        Font {
            weight: FontWeight::SEMIBOLD,
            ..sans.clone()
        },
        sans,
        code.clone().bold(),
        code.clone().italic(),
        code,
    ]
}

impl Controller {
    async fn serve(mut self, inputs: async_channel::Receiver<Input>, cx: &mut AsyncApp) {
        if self.bus.is_some() {
            self.warm_when_ready(cx);
        }
        loop {
            let input = if self.idle() {
                match self.leave(&inputs, cx).await {
                    Some(input) => input,
                    None => break,
                }
            } else {
                match inputs.recv().await {
                    Ok(input) => input,
                    Err(_) => break,
                }
            };
            match input {
                Input::Request(request) => self.request(request, Some(cx)),
                Input::Intent(intent) => self.intent(intent, cx).await,
                Input::Loaded(generation, loaded) => {
                    if generation == self.generation {
                        self.loaded(*loaded, cx);
                    }
                }
                // Closed by the window manager: the next is made at once.
                Input::Closed(id) => {
                    if self
                        .window
                        .as_ref()
                        .is_some_and(|w| w.handle.window_id() == id)
                    {
                        self.window = None;
                        self.close(cx);
                        if self.bus.is_some() {
                            self.warm_when_ready(cx);
                        }
                    }
                }
                // The window for the next file, and what previews would wait
                // for, unless a file that came first has opened its own: then
                // the rest is prepared as its preview closes.
                Input::Warm => {
                    if self.window.is_none() && !self.loading {
                        content::prepare();
                        self.warm(cx);
                    }
                }
            }
        }
        extract::clear(&Places::standard());
        cx.update(|cx| cx.quit());
    }

    /// Nothing shown or on its way, and no caller left on the bus.
    fn idle(&self) -> bool {
        !self.window.as_ref().is_some_and(|w| w.shown) && !self.loading && self.callers == 0
    }

    /// Waits a moment for a call, then gives the name up and serves whatever
    /// was already on its way. Without a bus, it leaves at once.
    async fn leave(
        &mut self,
        inputs: &async_channel::Receiver<Input>,
        cx: &mut AsyncApp,
    ) -> Option<Input> {
        let bus = self.bus.as_ref()?;
        let executor = cx.background_executor().clone();
        let waited = inputs
            .recv()
            .or(async {
                executor.timer(IDLE).await;
                Err(async_channel::RecvError)
            })
            .await;
        if let Ok(input) = waited {
            return Some(input);
        }
        let _ = bus.release();
        loop {
            let late = inputs
                .recv()
                .or(async {
                    executor.timer(LINGER).await;
                    Err(async_channel::RecvError)
                })
                .await
                .ok()?;
            if let Input::Request(_) | Input::Intent(_) = late {
                let _ = bus.reclaim();
                return Some(late);
            }
        }
    }

    fn request(&mut self, request: Request, cx: Option<&mut AsyncApp>) {
        match request {
            Request::Show {
                uri,
                window_handle,
                close_if_shown,
                activation_token,
            } => {
                trace::mark("request");
                if self.shown.as_ref().is_some_and(|shown| shown.uri == uri) {
                    if close_if_shown && let Some(cx) = cx {
                        self.close(cx);
                    }
                    return;
                }
                if let Some(bus) = &self.bus {
                    let _ = bus.set_parent_handle(&window_handle);
                    let _ = bus.set_visible(true);
                }
                if !self.window.as_ref().is_some_and(|w| w.shown)
                    && let Some(x11) = &self.x11
                {
                    self.screen = x11.screen_for(x11::parse_handle(&window_handle));
                }
                self.shown = Some(Shown {
                    uri: uri.clone(),
                    window_handle,
                    activation_token,
                    folder: false,
                });
                self.load(uri);
            }
            Request::Close => {
                if let Some(cx) = cx {
                    self.close(cx);
                }
            }
            Request::Callers(count) => self.callers = count,
        }
    }

    fn load(&mut self, uri: String) {
        self.generation += 1;
        self.loading = true;
        let (generation, screen, input) = (self.generation, self.screen, self.input.clone());
        // A window ready, shown or waiting, shows a picture once it has its
        // pixels; only a window still to be made opens beside the decoding.
        let wait_for_pixels = self.window.is_some();
        std::thread::Builder::new()
            .name("load".into())
            .spawn(move || {
                let loaded = content::load(&uri, screen, wait_for_pixels);
                trace::mark("content ready");
                let _ = input.send_blocking(Input::Loaded(generation, Box::new(loaded)));
            })
            .expect("spawning the loader");
    }

    fn loaded(&mut self, loaded: Loaded, cx: &mut AsyncApp) {
        self.loading = false;
        let Some(shown) = &mut self.shown else {
            return;
        };
        shown.folder = loaded.facts.kind == Kind::Folder;
        trace::mark("loaded");
        let size = loaded.size;
        let Some(open) = &mut self.window else {
            return self.open(loaded, true, cx);
        };
        if (!open.shown || open.size != size)
            && let (Some(x11), Some(xid)) = (&self.x11, open.xid)
        {
            let _ = x11.configure(xid, self.screen.centre(size));
        }
        open.size = size;
        if open.shown {
            let _ = open
                .handle
                .update(cx, |view, window, cx| view.show(loaded, window, cx));
            return;
        }
        // The window waiting for this file: its content first, then on the
        // screen, where the window manager centres it and gives it the focus.
        let screen = self.screen;
        let _ = open.handle.update(cx, |view, window, cx| {
            view.reopen(loaded, screen, window, cx)
        });
        open.shown = true;
        if let (Some(x11), Some(xid)) = (&self.x11, open.xid) {
            if let Some(parent) = x11::parse_handle(&shown.window_handle) {
                let _ = x11.set_transient_for(xid, parent);
            }
            let _ = x11.complete_startup(xid, &shown.activation_token);
            trace::mark("map");
            let _ = x11.map(xid);
        }
    }

    /// Asks for the window ahead of the first file once the GPU device is
    /// made, which opening it would otherwise wait for, holding up a file
    /// that came meanwhile.
    fn warm_when_ready(&self, cx: &mut AsyncApp) {
        let input = self.input.clone();
        let executor = cx.background_executor().clone();
        cx.background_executor()
            .spawn(async move {
                while !gpui_wgpu::WgpuContext::prepared() {
                    executor.timer(Duration::from_millis(5)).await;
                }
                let _ = input.send(Input::Warm).await;
            })
            .detach();
    }

    /// Makes the window ahead of the first file, withdrawn, on the monitor
    /// under the pointer.
    fn warm(&mut self, cx: &mut AsyncApp) {
        if let Some(x11) = &self.x11 {
            self.screen = x11.screen_for(None);
        }
        self.open(Loaded::blank(self.screen), false, cx);
    }

    /// Opens the window with `loaded`, on the screen when `visible`.
    fn open(&mut self, loaded: Loaded, visible: bool, cx: &mut AsyncApp) {
        let size = loaded.size;
        let rect = self.screen.centre(size);
        let scale = self.screen.scale as f32;
        let logical = |v: i32| px(v as f32 / scale);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                // GPUI creates X11 windows 2 px right of the origin it is given.
                point(logical(rect.x - 2), logical(rect.y)),
                gpui_kit::size(logical(rect.width), logical(rect.height)),
            ))),
            titlebar: Some(TitlebarOptions {
                title: Some(loaded.facts.name.clone().into()),
                ..Default::default()
            }),
            window_decorations: Some(WindowDecorations::Client),
            window_background: if self.rounded {
                WindowBackgroundAppearance::Transparent
            } else {
                WindowBackgroundAppearance::Opaque
            },
            app_id: Some("silent".into()),
            show: visible,
            ..Default::default()
        };
        let (screen, rounded, input) = (self.screen, self.rounded, self.input.clone());
        trace::mark("window opens");
        let opened = cx.update(|cx| {
            cx.open_window(options, |window, cx| {
                cx.new(|cx| Preview::new(loaded, screen, rounded, input, window, cx))
            })
        });
        let handle = match opened {
            Ok(handle) => handle,
            Err(error) => {
                eprintln!("silent: cannot open a window: {error:#}");
                self.close(cx);
                return;
            }
        };
        trace::mark("window open");
        let xid = handle.update(cx, |_, window, _| xid(window)).ok().flatten();
        if let (Some(x11), Some(xid)) = (&self.x11, xid) {
            let _ = x11.undecorate(xid);
            if let Some(shown) = self.shown.as_ref().filter(|_| visible) {
                if let Some(parent) = x11::parse_handle(&shown.window_handle) {
                    let _ = x11.set_transient_for(xid, parent);
                }
                let _ = x11.complete_startup(xid, &shown.activation_token);
            }
        }
        self.window = Some(Open {
            handle,
            xid,
            size,
            shown: visible,
        });
    }

    /// Ends the preview: the window lets its file go and is withdrawn, to be
    /// shown again for the next.
    fn close(&mut self, cx: &mut AsyncApp) {
        if let Some(open) = self.window.as_mut().filter(|w| w.shown) {
            let _ = open.handle.update(cx, |view, window, cx| {
                if window.is_fullscreen() {
                    window.toggle_fullscreen();
                }
                view.clear(window, cx);
            });
            if let (Some(x11), Some(xid)) = (&self.x11, open.xid) {
                let _ = x11.unmap(xid);
            }
            open.shown = false;
            if self.bus.is_some() {
                content::prepare();
                // The heap the file took goes back to the system while
                // silent waits for the next.
                #[cfg(target_env = "gnu")]
                unsafe {
                    libc::malloc_trim(0);
                }
            }
        }
        self.shown = None;
        self.generation += 1;
        self.loading = false;
        if let Some(bus) = &self.bus {
            let _ = bus.set_visible(false);
        }
    }

    async fn intent(&mut self, intent: Intent, cx: &mut AsyncApp) {
        match intent {
            Intent::Redock(by) => return self.redock(by),
            Intent::Fit(size) => return self.fit(size),
            _ => {}
        }
        let Some(shown) = &self.shown else {
            return;
        };
        let (uri, folder) = (shown.uri.clone(), shown.folder);
        match intent {
            Intent::Close => self.close(cx),
            Intent::Select(direction) => {
                if let Some(bus) = &self.bus {
                    let _ = bus.selection_event(direction);
                }
            }
            Intent::Open if folder => {
                self.close(cx);
                if let Some(bus) = &self.bus {
                    let _ = bus.navigate_to(&uri);
                }
            }
            Intent::Open => {
                match gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE) {
                    Ok(()) => self.close(cx),
                    Err(error) => eprintln!("silent: cannot open {uri}: {error}"),
                }
            }
            Intent::Rename => {
                self.close(cx);
                // Nautilus's rename popover is ignored if it opens while the preview is still going away.
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                if let Some(bus) = &self.bus {
                    let _ = bus.rename(&uri);
                }
            }
            Intent::Trash => {
                self.close(cx);
                if let Some(bus) = &self.bus {
                    let _ = bus.trash(&uri);
                }
            }
            Intent::Redock(_) | Intent::Fit(_) => {}
        }
    }

    /// Resizes the window about its centre.
    fn fit(&mut self, size: (i32, i32)) {
        let (Some(x11), Some(open)) = (&self.x11, &mut self.window) else {
            return;
        };
        let Some(frame) = open.xid.and_then(|xid| x11.frame(xid)) else {
            return;
        };
        let rect = self.screen.around(frame.centre(), size);
        if let Some(xid) = open.xid {
            let _ = x11.configure(xid, rect);
        }
        open.size = (rect.width, rect.height);
    }

    /// Widens or narrows the window at its right edge, so that the content
    /// stays where it is on the screen.
    fn redock(&mut self, by: f32) {
        let (Some(x11), Some(open)) = (&self.x11, &mut self.window) else {
            return;
        };
        let Some(frame) = open.xid.and_then(|xid| x11.frame(xid)) else {
            return;
        };
        let by = self.screen.pixels(f64::from(by));
        let rect = geometry::redock(frame, self.screen.monitor, by);
        if let Some(xid) = open.xid {
            let _ = x11.configure(xid, rect);
        }
        open.size = (rect.width, rect.height);
    }
}

fn xid(window: &Window) -> Option<u32> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Xcb(handle) => Some(handle.window.get()),
        RawWindowHandle::Xlib(handle) => u32::try_from(handle.window).ok(),
        _ => None,
    }
}
