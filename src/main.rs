mod app;
mod archive;
mod content;
mod docview;
mod grid;
mod icons;
mod keys;
mod lines;
mod media;
mod overview;
mod paged;
mod pdf;
mod pdftext;
mod picture;
mod slides;
mod strip;
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

/// How long a process D-Bus started waits for something to show.
const IDLE: Duration = Duration::from_secs(3);

fn main() -> ExitCode {
    // Pictures and pages are mapped and given back when freed. Left to itself,
    // glibc raises this threshold after the first large free, and the heap of
    // a previewer that stays grows with every picture it has shown.
    #[cfg(target_env = "gnu")]
    unsafe {
        libc::mallopt(libc::M_MMAP_THRESHOLD, 1 << 20);
    }
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
/// this process for a call, so that call is waited for before anything else;
/// a call that shows nothing, such as Nautilus's ping, leaves it idle, and it
/// goes after a moment.
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
        async_io::Timer::after(IDLE).await;
        Err(async_channel::RecvError)
    }));
    let Ok(first) = first else {
        let _ = bus.release();
        return ExitCode::SUCCESS;
    };
    trace::mark("request");
    if matches!(first, Request::Show { .. }) {
        // The GPU's instance and device take longer than anything else before
        // the window: they are made beside the toolkit and the loading.
        gpui_wgpu::WgpuContext::prepare();
    }
    app::run(Some(bus), requests, vec![first]);
    ExitCode::SUCCESS
}

fn standalone(path: &str) -> ExitCode {
    gpui_wgpu::WgpuContext::prepare();
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
        if matches!(
            facts.kind,
            Kind::Document | Kind::Spreadsheet | Kind::Presentation
        ) {
            describe_office(&path, &facts.content_type, facts.kind);
            println!("  read in {:.1} ms", ms(started));
            continue;
        }
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

/// What the Office reader makes of a file: blocks, sheets or slides.
fn describe_office(path: &std::path::Path, content_type: &str, kind: Kind) {
    use silent::office::{self, Block, doc, docx, odf, ppt, pptx, sheet};
    let open = content_type.contains("opendocument");
    let ole = office::is_ole_file(path);
    if content_type.contains("apple") || content_type.contains("iwork") {
        let preview = office::iwork_preview(path);
        println!("  preview picture: {:?} bytes", preview.map(|p| p.len()));
        return;
    }
    match kind {
        Kind::Spreadsheet => match sheet::open(path) {
            Some((names, current, grid)) => println!(
                "  sheets {names:?}; opens on {:?}: {} × {} used, {} rows read from {:?}",
                names[current],
                grid.size.0,
                grid.size.1,
                grid.cells.len(),
                grid.origin
            ),
            None => println!("  unreadable"),
        },
        Kind::Presentation if !open && !ole => {
            let Some(deck) = pptx::open(path) else {
                println!("  unreadable");
                return;
            };
            println!("  {} slides of {:?} pt", deck.slides.len(), deck.size);
            for i in 0..deck.slides.len().min(3) {
                let Some(slide) = pptx::slide(path, &deck, i) else {
                    continue;
                };
                println!("  slide {}: {} items", i + 1, slide.items.len());
                for item in &slide.items {
                    let what = match &item.kind {
                        pptx::Kind::Shape { text: Some(t), .. } => t
                            .paragraphs
                            .iter()
                            .map(|p| p.spans.iter().map(|s| s.text.as_str()).collect::<String>())
                            .collect::<Vec<_>>()
                            .join(" / "),
                        pptx::Kind::Shape { .. } => "shape".into(),
                        pptx::Kind::Picture(data) => format!("picture of {} bytes", data.len()),
                        pptx::Kind::Table { rows, .. } => format!("table of {} rows", rows.len()),
                    };
                    println!("    {:?}: {what}", item.rect);
                }
            }
        }
        _ => {
            let document = match kind {
                Kind::Presentation if ole => ppt::read(path),
                Kind::Presentation => odf::read_slides(path),
                _ if ole => doc::read(path),
                _ if open => odf::read_text(path),
                _ => docx::read(path),
            };
            let Some(document) = document else {
                println!("  unreadable");
                return;
            };
            println!(
                "  page {:?} pt, {} blocks",
                document.page,
                document.blocks.len()
            );
            for block in document.blocks.iter().take(12) {
                match block {
                    Block::Paragraph(p) => {
                        println!("    {:?} {:?} {}", p.role, p.marker, p.text())
                    }
                    Block::Table(t) => println!("    table of {} rows", t.rows.len()),
                    Block::Image(i) => println!("    picture {:?} pt", i.size),
                }
            }
        }
    }
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}
