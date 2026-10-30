# Changelog

## 1.0.0 (2026-10-29)

The first stable release. silent takes over Nautilus's Space-to-preview from
GNOME's sushi on X11, and serves Nemo, yazi and terminal file managers too.
It runs on X11 only.

**The window.** A preview opens centred, at a size worked out from the
content before the window maps, with no title bar, frame or buttons. It has
rounded corners under a compositor, follows the desktop's light or dark
preference in warm tones, and keeps up with the pointer when it is resized.
Its fades and other motion are drawn at the monitor's full rate, also while
a terminal's file manager keeps the keyboard.

**Speed.** silent starts when Nautilus does and stays while it runs, with
its window and GPU device made ahead of the first Space, so a preview opens
at its final size, with the file drawn within a frame or two. A new build
installed over it takes over at the next preview.

**What it shows.**

- Pictures in every format glycin reads, upright and sharp at any size, with
  JPEG and PNG decoded in silent's own process; zoom with Ctrl and + or −,
  and drag.
- Video and audio through GStreamer, with a playbar for seeking and the volume.
- PDFs as pages in a scroll with thumbnails docked beside them, text you can
  select and copy, and zoom that redraws only what is in view.
- Text and code highlighted, with a minimap, soft wrap, Chinese in GBK, and
  minified JSON laid out; binaries as `hexdump -C` prints them.
- Markdown and HTML rendered, HTML by Blitz with the page's own CSS, and
  nothing fetched from the network.
- Word, Excel and PowerPoint files, old and new, OpenDocument files, and the
  preview pictures of Keynote, Pages and Numbers, with no office suite.
- gzip, xz, zstd, bzip2, lz4 and lzma files as what is inside them, and
  archives as a tree whose files open in the same window, archives within
  archives too.
- Folders with their size, and a card for everything else.

**Keys.** `?` shows the keys of the file in front of you on a card. The arrows
browse Nautilus's files, vim's keys move through text, documents and pages,
`m` sets the minimap or thumbnails, `i` the info line and `w` wrapping, `r`
turns pictures and pages, and Ctrl with + and − sizes text and zooms pages.

**Memory.** A file you come back to within the hour opens as you left it, and
the modes you chose are kept until the next boot. Nothing else is kept.

**Elsewhere.** `silent show`, `toggle`, `follow`, `close` and `warm` talk to
the resident previewer from any program, starting it if need be; there is a
plugin for yazi and an `org.nemo.Preview` service for Nemo. `silent
--version` prints the version.
