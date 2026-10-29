# Developing silent

silent is a Rust program on [GPUI](https://www.gpui.rs) through
[gpui-kit](https://crates.io/crates/gpui-kit), drawing with Vulkan through
wgpu on X11.

## Building

You need Rust 1.95 or newer and the libraries silent links: on Arch Linux,
`glycin`, `libarchive`, `gstreamer` with `gst-plugins-base` and
`gst-plugins-good`, `vulkan-icd-loader`, `libxcb`, `libxkbcommon`,
`libxkbcommon-x11`, `fontconfig`, `glib2` and `libseccomp`, and at run time
`bubblewrap` for glycin's sandboxed loaders.

```sh
cargo build --release
target/release/silent FILE    # previews FILE on its own, without D-Bus
```

To try a build as Nautilus's previewer without installing it, point a
service file of your own at it, in
`~/.local/share/dbus-1/services/org.gnome.NautilusPreviewer.service`:

```ini
[D-BUS Service]
Name=org.gnome.NautilusPreviewer
Exec=/path/to/silent/target/release/silent
```

The next Space starts it. A previewer already running leaves by itself
when its executable is replaced, once no preview is open.

## Quick builds while you iterate

The release profile links with thin LTO, so a one-line change takes about
two minutes to build. A profile without LTO, built incrementally and linked
by [mold](https://github.com/rui314/mold), rebuilds the same change in some
seven seconds:

```sh
RUSTFLAGS="-C link-arg=-fuse-ld=mold" cargo build --profile quick \
    --config 'profile.quick.inherits="release"' \
    --config 'profile.quick.lto=false' \
    --config 'profile.quick.codegen-units=64' \
    --config 'profile.quick.incremental=true' \
    --config 'profile.quick.strip=false'
```

The binary is `target/quick/silent`. Timings and anything you install
should come from a release build.

## Tests

`cargo test --release` runs the unit tests of the library and the binary,
about 190 of them: geometry, wrapping, vim's counts, the info line's
colours, the Office readers, the hexdump, what is remembered, and the keys
on the `?` card among them. The Arch package runs the library's at build
time.

## The test harness

`scripts/harness` makes test files, starts a private X display (3840×2160
at 163 DPI) and session bus, makes Nautilus's calls, and times cold starts.
Nothing it does touches your display or your session bus, and inside it
silent draws on the CPU (Mesa's lavapipe) and plays sound into nothing.

```sh
scripts/harness fixtures
export DISPLAY=$(scripts/harness display)
F=~/.cache/silent-harness/fixtures
scripts/harness cold "$PWD/target/release/silent" $F/images/photo-4032x3024.jpg 10
```

The harness keeps its files, the fixtures among them, in `$SILENT_SCRATCH`,
by default `~/.cache/silent-harness`. To look at a preview, run a command on
the private bus, where `harness show` calls ShowFile as Nautilus does and
`harness shot` saves the display:

```sh
scripts/harness bus "$PWD/target/release/silent" -- bash -c '
    scripts/harness show "$1"; sleep 2; scripts/harness shot pdf' _ $F/docs/dotguide.pdf
```

`silent --probe FILE...` prints what a preview of each file would be and how
long decoding takes. `SILENT_TRACE=1` prints a mark at each step of the
start-up on stderr, with the wall clock: the bus, the fonts, the X11 window,
the GPU's instance, device, swapchain and pipelines, the first frame and its
present. `SILENT_TRACE=frames` adds every frame's ConfigureNotify, swapchain
and present, as resizing the window shows them, with the lines shaped, the
glyphs rasterized and the time the renderer took.

## The benchmark

`scripts/bench` times silent against sushi on the desktop, from the
ShowFile call to the first frame that shows the file, read back from the X
server: cold, warm, with each previewer's process kept alive between
previews as it lives, and switching files in an open window. It switches
previewers with a service file of yours and always puts yours back. The
results are in [benchmark.md](benchmark.md).

```sh
scripts/bench fixtures
scripts/bench run --n 5
```

## Vendored crates

gpui's Linux and wgpu backends, gpui-base, Blitz's `blitz-dom` and
`blitz-paint`, and hayro live in `vendor/`, taken in through
`[patch.crates-io]` in `Cargo.toml`, with silent's changes on top: start-up
marks, Vulkan alone, a GPU device made before the first window, resizes
that keep up with the pointer, HTML laid out block by block, and a part of
a PDF page drawn alone, among others. [vendor/README.md](../vendor/README.md)
says what each change is and why; the first commit that touches a crate
adds it unchanged, so `git log -p -- vendor` shows every change, and
`silent:` comments mark them in the code. To take a newer release, copy its
crates over these, drop their `Cargo.lock` and benchmarks, and carry the
changes over.

## Where things are

| File | What it does |
| --- | --- |
| `src/main.rs` | The command line: serving the bus, `silent FILE`, the client verbs, `--probe` |
| `src/app.rs`, `src/dbus.rs` | The previewer's life, and the D-Bus interfaces of Nautilus, Nemo and the clients |
| `src/view.rs` | The preview window: the content, the info line, the playbar, the keys |
| `src/geometry.rs`, `src/x11.rs` | How big a window is and where it goes, and what the toolkit cannot do on X11 |
| `src/detect.rs`, `src/content.rs` | What a file is, and loading what a preview shows off the UI thread |
| `src/decode.rs`, `src/native.rs`, `src/picture.rs`, `src/framing.rs` | Pictures: decoding, JPEG and PNG in the process, zoom |
| `src/media.rs` | Video and audio through GStreamer |
| `src/pdf.rs`, `src/pdftext.rs`, `src/paged.rs`, `src/slides.rs` | PDF pages and their text, and slides, in a scroll |
| `src/lines.rs`, `src/wrap.rs`, `src/hexdump.rs`, `src/pretty.rs` | Text, code, wrapping, hexdumps and minified JSON |
| `src/web.rs`, `src/page.rs`, `src/rendered.rs`, `src/markup.rs` | HTML through Blitz, and Markdown |
| `src/office/`, `src/docview.rs`, `src/grid.rs` | The Office and OpenDocument readers, documents and spreadsheets |
| `src/archive.rs`, `src/extract.rs`, `src/unpack.rs` | Archives as a tree, files taken out of them, compressed files |
| `src/overview.rs`, `src/strip.rs`, `src/scrim.rs` | Minimaps and thumbnails, the info line and its colours |
| `src/help.rs`, `src/keys.rs`, `src/vim.rs` | The `?` card, what each key does, vim's keys |
| `src/memory.rs`, `src/zoom.rs` | Where each file was left, and the text size |
| `src/theme.rs`, `src/icons.rs` | The warm colours, and the file-type icons |

## Packaging

`packaging/arch/PKGBUILD` builds the working tree it sits in, runs the
tests, and installs `/usr/bin/silent` with Nautilus's D-Bus service; it
provides and conflicts with `sushi`. From that directory, `makepkg -si`.
