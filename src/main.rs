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
use std::time::Instant;

use gio::glib;
use silent::dbus::{Bus, Request};
use silent::decode::{self, Event, MemoryFormatSelection};
use silent::detect::{self, Kind};
use silent::x11::X11;

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
/// this process for a call to the name, Nautilus's ping as it starts or a
/// ShowFile; what a preview needs is made from then on, in the background,
/// and the process lives as long as its callers stay on the bus.
fn serve() -> ExitCode {
    gpui_wgpu::WgpuContext::prepare();
    let (sender, requests) = async_channel::unbounded();
    watch_executable(sender.clone());
    let bus = match Bus::start(sender) {
        Ok(bus) => bus,
        Err(e) => {
            eprintln!("silent: cannot serve {}: {e}", silent::dbus::NAME);
            return ExitCode::FAILURE;
        }
    };
    trace::mark("bus");
    app::run(Some(bus), requests, Vec::new());
    ExitCode::SUCCESS
}

/// Tells the previewer when the executable it was started from is replaced
/// or removed, which an upgrade does: it leaves once no preview is open, and
/// the next call starts the new one. The directory is watched, since an
/// install renames a new file over the old or unlinks it first.
fn watch_executable(requests: async_channel::Sender<Request>) {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;

    let (Ok(path), Ok(running)) = (std::env::current_exe(), std::fs::metadata("/proc/self/exe"))
    else {
        return;
    };
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return;
    };
    let Ok(dir) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
        return;
    };
    let name = name.to_owned();
    let replaced = move || {
        std::fs::metadata(&path).map_or(true, |now| {
            (now.dev(), now.ino()) != (running.dev(), running.ino())
        })
    };
    let _ = std::thread::Builder::new()
        .name("executable".into())
        .spawn(move || {
            // SAFETY: plain system calls on a descriptor this thread owns.
            let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
            if fd < 0 {
                return;
            }
            let mask = libc::IN_CREATE | libc::IN_MOVED_TO | libc::IN_CLOSE_WRITE
                | libc::IN_DELETE | libc::IN_MOVED_FROM | libc::IN_ATTRIB;
            if unsafe { libc::inotify_add_watch(fd, dir.as_ptr(), mask) } < 0 {
                unsafe { libc::close(fd) };
                return;
            }
            let mut buffer = [0u8; 4096];
            loop {
                let read = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
                if read <= 0 {
                    break;
                }
                let mut events = &buffer[..read as usize];
                let header = std::mem::size_of::<libc::inotify_event>();
                let mut ours = false;
                while events.len() >= header {
                    let len = u32::from_ne_bytes(events[12..16].try_into().unwrap()) as usize;
                    let named = &events[header..(header + len).min(events.len())];
                    let named = &named[..named.iter().position(|&b| b == 0).unwrap_or(named.len())];
                    ours |= named == name.as_bytes();
                    events = &events[(header + len).min(events.len())..];
                }
                if ours && replaced() {
                    let _ = requests.send_blocking(Request::Replaced);
                    break;
                }
            }
            unsafe { libc::close(fd) };
        });
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
                    let pending = match (frame.orientation, &frame.icc) {
                        (decode::Orientation::NoTransforms, None) => String::new(),
                        (orientation, icc) => format!(
                            ", still to turn {orientation:?}{}",
                            if icc.is_some() { " and take to sRGB" } else { "" }
                        ),
                    };
                    println!(
                        "  frame {}x{} {:?}{pending} at {:.1} ms",
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
