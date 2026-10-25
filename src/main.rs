mod app;
mod archive;
mod code;
mod content;
mod media;
mod pdf;
mod picture;
mod theme;
mod trace;
mod view;

use std::process::ExitCode;
use std::time::{Duration, Instant};

use futures_lite::FutureExt;
use gio::glib;
use silent::dbus::{Bus, Request};
use silent::decode::{self, Event, MemoryFormatSelection};
use silent::detect::{self, Kind};
use silent::x11::X11;

fn main() -> ExitCode {
    trace::start();
    trace::mark("main");
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--probe") => probe(&args[1..]),
        Some("--help" | "-h") => {
            println!("usage: silent            serve Nautilus's previewer on the session bus");
            println!("       silent FILE       preview FILE");
            println!(
                "       silent --probe FILE...  print what a preview would be, and its timings"
            );
            ExitCode::SUCCESS
        }
        Some(path) => standalone(path),
        None => serve(),
    }
}

/// Owns Nautilus's previewer name and shows what it asks for. D-Bus starts
/// this process for a call, so that call is waited for before anything else.
fn serve() -> ExitCode {
    let (sender, requests) = async_channel::unbounded();
    let bus = match Bus::start(sender) {
        Ok(bus) => bus,
        Err(e) => {
            eprintln!("silent: cannot serve {}: {e}", silent::dbus::NAME);
            return ExitCode::FAILURE;
        }
    };
    trace::mark("bus");
    let first = async_io::block_on(requests.recv().or(async {
        async_io::Timer::after(Duration::from_secs(10)).await;
        Err(async_channel::RecvError)
    }));
    let Ok(first) = first else {
        let _ = bus.release();
        return ExitCode::SUCCESS;
    };
    trace::mark("request");
    app::run(Some(bus), requests, vec![first]);
    ExitCode::SUCCESS
}

fn standalone(path: &str) -> ExitCode {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.into());
    let Ok(uri) = glib::filename_to_uri(&path, None) else {
        eprintln!("silent: {} is not a local file", path.display());
        return ExitCode::FAILURE;
    };
    let (_sender, requests) = async_channel::unbounded();
    let show = Request::Show {
        uri: uri.into(),
        window_handle: "x11:0".into(),
        close_if_shown: false,
        activation_token: String::new(),
    };
    app::run(None, requests, vec![show]);
    ExitCode::SUCCESS
}

/// Prints what a preview of each file would be, and how long it takes.
fn probe(paths: &[String]) -> ExitCode {
    let x11 = X11::connect().ok();
    let screen = x11.as_ref().map(|x| x.screen_for(None));
    println!("screen {screen:?}");
    for path in paths {
        let started = Instant::now();
        let path = std::path::absolute(path).unwrap_or_else(|_| path.into());
        let Ok(uri) = glib::filename_to_uri(&path, None) else {
            eprintln!("{}: not a local file", path.display());
            continue;
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
        if facts.kind != Kind::Image {
            continue;
        }
        let decoder = decode::start(uri.into(), screen, MemoryFormatSelection::all(), |f| f);
        while let Ok(event) = decoder.events.recv_blocking() {
            match event {
                Event::Size { width, height } => {
                    let fit = screen.map(|s| s.fit_media((width, height)));
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
    ExitCode::SUCCESS
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}
