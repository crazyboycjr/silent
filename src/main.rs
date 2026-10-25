use std::process::ExitCode;
use std::time::Instant;

use gio::glib;
use silent::dbus::{Bus, Request};
use silent::decode::{self, Event, MemoryFormatSelection};
use silent::detect::{self, Kind};
use silent::geometry::{Rect, Screen};
use silent::x11::{self, X11};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--probe") => probe(&args[1..]),
        _ => serve(),
    }
}

fn screen(x11: Option<&X11>, parent: Option<u32>) -> Screen {
    x11.map(|x| x.screen_for(parent)).unwrap_or(Screen {
        monitor: Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        },
        scale: 1.0,
    })
}

/// Prints what a preview of each file would be, and how long it takes.
fn probe(paths: &[String]) -> ExitCode {
    let x11 = X11::connect().ok();
    let screen = screen(x11.as_ref(), None);
    println!("screen {screen:?}");
    for path in paths {
        let started = Instant::now();
        let path = std::path::absolute(path).unwrap_or_else(|_| path.into());
        let uri = match glib::filename_to_uri(&path, None) {
            Ok(uri) => uri.to_string(),
            Err(e) => {
                eprintln!("{}: {e}", path.display());
                continue;
            }
        };
        let facts = match detect::probe(&uri) {
            Ok(facts) => facts,
            Err(e) => {
                eprintln!("{}: {e}", path.display());
                continue;
            }
        };
        println!(
            "{}: {:?} {} {} bytes, probed in {:.1} ms",
            facts.name,
            facts.kind,
            facts.content_type,
            facts.size,
            ms(started)
        );
        if facts.kind == Kind::Image {
            let decoder = decode::start(uri, Some(screen), MemoryFormatSelection::all(), |frame| {
                frame
            });
            while let Ok(event) = decoder.events.recv_blocking() {
                match event {
                    Event::Size { width, height } => {
                        let fit = screen.fit_media((width, height));
                        println!(
                            "  size {width}x{height} at {:.1} ms, shown as {fit:?}",
                            ms(started)
                        );
                    }
                    Event::Frame(frame) => {
                        println!(
                            "  frame {}x{} {:?} at {:.1} ms",
                            frame.width,
                            frame.height,
                            frame.format,
                            ms(started)
                        );
                        break;
                    }
                    Event::Failed(error) => {
                        println!("  failed at {:.1} ms: {error}", ms(started));
                        break;
                    }
                }
            }
        }
    }
    ExitCode::SUCCESS
}

/// Answers Nautilus without showing anything yet: each `ShowFile` is
/// probed and decoded, and the process leaves once the preview is closed.
fn serve() -> ExitCode {
    let started = Instant::now();
    let (sender, requests) = async_channel::unbounded();
    let bus = match Bus::start(sender) {
        Ok(bus) => bus,
        Err(e) => {
            eprintln!("silent: cannot serve {}: {e}", silent::dbus::NAME);
            return ExitCode::FAILURE;
        }
    };
    eprintln!("silent: serving after {:.1} ms", ms(started));
    let x11 = X11::connect().ok();
    let mut shown: Option<String> = None;
    while let Ok(request) = requests.recv_blocking() {
        match request {
            Request::Show {
                uri,
                window_handle,
                close_if_shown,
                ..
            } => {
                if shown.as_deref() == Some(uri.as_str()) {
                    if close_if_shown {
                        let _ = bus.set_visible(false);
                        break;
                    }
                    continue;
                }
                let parent = x11::parse_handle(&window_handle);
                let _ = bus.set_parent_handle(&window_handle);
                let _ = bus.set_visible(true);
                let screen = screen(x11.as_ref(), parent);
                match detect::probe(&uri) {
                    Ok(facts) => {
                        eprintln!("silent: {uri}: {:?} {}", facts.kind, facts.content_type)
                    }
                    Err(e) => eprintln!("silent: {uri}: {e}"),
                }
                eprintln!("silent: would show on {screen:?}");
                shown = Some(uri);
            }
            Request::Close => {
                let _ = bus.set_visible(false);
                break;
            }
        }
    }
    let _ = bus.release();
    ExitCode::SUCCESS
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}
