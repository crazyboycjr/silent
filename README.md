# silent

Press Space on a file in Nautilus and silent shows it, in the middle of the
screen, at a size you can see. It takes the place of GNOME's sushi.

- **No border.** No title bar, no frame, no buttons: the window is the
  picture, or a card, and nothing else.
- **Minimal.** One line of facts about the file fades in over the bottom
  of the picture when the pointer moves and out when it rests, on a veil
  only as dark or as light as its letters need to stay readable over what
  is under them. The window follows the desktop's light or dark
  preference, in warm tones.
- **Blazing fast.** The window opens at its final size, centred, with the
  content already in it: text some 15 ms after Nautilus asks, a PDF in 50,
  a large photo as fast as it decodes. silent starts when Nautilus does,
  which pings it, makes its window and its GPU device then, and keeps them
  until Nautilus quits; a closed preview's window waits withdrawn for the
  next, holding no file.
- **Smarter, broader format support.** Pictures in every format the system's
  glycin loaders read (JPEG, PNG, WebP, AVIF, HEIC, JPEG XL, SVG, TIFF, GIF
  and animated GIF, WebP and APNG among them), upright as the camera meant,
  sharp at any size: tiny icons grow pixel for pixel, vector images are drawn
  at the size they are shown. Video and music play and carry on where you
  left them, PDFs scroll, code is highlighted, Chinese text in GBK reads
  right, binaries read as `hexdump -C`, a `.json.gz` opens as laid-out JSON,
  Word, Excel and PowerPoint files open without an office suite, archives
  open as a tree, folders show how much they weigh.

## Formats

| Kind | Shown as |
| --- | --- |
| Images | The picture at its own aspect, as large as the screen allows; animations play |
| Video | Plays at its own aspect, with sound; hardware decoding when GStreamer has it; a playbar over the bottom of the picture, with the seek track, the times and the volume, while the pointer moves and after each seek |
| Audio | A card with the cover art, title, artist and album, playing, with the same playbar |
| PDF | The pages in a scroll, shaped by the first page, rendered as they come into view; a rail of page thumbnails docked at the right; text can be selected and copied |
| Markdown and HTML | Rendered: headings, lists, tables, code highlighted, pictures beside the file; s shows the source and back. A picture from the web shows its words: silent fetches nothing. Files over 256 KB open as their source |
| Text and code | Highlighted, with line numbers; a minimap over the right edge while it scrolls, or docked beside the text; long lines scroll sideways, or wrap at the window's width with w; select with the mouse and copy; UTF-8, UTF-16 and GB18030 (GBK) text; minified JSON laid out; the first 2 MB of large files |
| Binary files | As `hexdump -C` prints them, read from a memory map, so a dump of gigabytes opens at once, minimap and all; select and copy as text |
| Compressed files | gzip, xz, zstd, bzip2, lz4 and lzma files show what is inside: text and code as above, a tar as its listing, anything else as a hexdump; the strip gives both sizes when the format records them |
| Word documents | docx, odt and the older doc of Word 97 to 2003 (and WPS Writer's wps) as a column on a page: headings, paragraphs with their bold, italic, underline and colour, bulleted and numbered lists, tables, and the pictures of docx and odt; docked at the right, the whole column drawn small |
| Spreadsheets | xlsx, xls and ods as a grid with column letters and row numbers, numbers and dates formatted, a tab for each worksheet, opening on the one that was in front; a click puts a cell cursor down, and the strip shows its cell in full; chart and hidden sheets get no tab; old .xls written as a bare BIFF stream too; the first 5,000 rows |
| Presentations | pptx as its slides in a scroll, with text, pictures, tables and shapes where the deck puts them and what they inherit from the layout and master, and a rail of the slides drawn small docked at the right; odp as its thumbnail and text; the older ppt as an outline of each slide's title and text |
| Keynote, Pages, Numbers | The preview picture the file carries, as Quick Look shows it |
| Archives | The entries as a tree with file-type icons and their sizes, and the total unpacked; zip, tar (gz, bz2, xz, zstd), 7z, rar, ISO and more; any file in them opens in the same window, archives in archives too |
| Folders | A card with the item count and the total size, counted in the background |
| Everything else | A card with the type, size and modification date |

The Office files are read the way macOS Quick Look reads them: silent opens
the package and parses its XML itself, or the binary streams of the formats
of Office 97 to 2003, with no office suite, and draws the result natively. Fonts come later.

Pictures are decoded by glycin's loaders, each in a sandbox of its own,
except JPEG and still PNG: silent decodes those itself, in pure Rust, JPEG
by libjpeg-turbo-rs, a port of libjpeg-turbo, and PNG by the decoder
glycin's loader uses, which saves the loader's process, some 35 ms a
picture, at the price of the sandbox for these two formats. A JPEG twice
the size of the screen or more is decoded at a half, a quarter or an
eighth of its size.

The strip of facts lies over the bottom of the content, and i says when:
while the pointer moves (it fades 1.5 s after it rests, a playbar 1 s
after), never, or always. Its colours are worked out, not chosen: silent
looks at what is under the strip, the picture's own pixels, a video's
frames a few times a second, or the paper and the ink of text and
documents, and lays down a dark veil under white letters or a light one
under near-black letters, whichever needs less, as faint as it can be
while the letters keep a contrast of 4.5:1 (WCAG AA) against everything
under them. Over a dark or a pale picture that is no veil at all.

An overview that is always shown never covers the content: it is docked
beside it, and the window is wider by the overview, at its right, so that
pressing M to dock or undock it leaves the content where it was on the
screen, as large as it was; near the screen's right edge the window moves
left just enough. Text shows its minimap over its right edge only while
it scrolls, unless M docks it. PDFs, slides and documents open with their
rail docked, within 85% of the screen with the page; M takes it away, and
the window narrows by as much. Without an overview, a slim scrollbar shows
while the view scrolls.

Long lines of text and code scroll sideways, and w wraps them at the
window's width instead, at the start of a word unless the word is wider
than the window, and between Chinese characters but not before their
commas; the window keeps its size and the line at the top stays there.
Wrapped, a line's number stands at its first row, the keys that move by a
line move by a row, 20G still goes to line 20, the minimap and the
scrollbar show the rows, and copying gives the lines without the breaks.
Text wraps again when the window changes size, and the next text file
wraps too, until w again, in a window as tall as its rows. Hexdumps keep
their width.

A file you leave, by moving to another or closing the preview, comes back
as you left it if you return within the hour: video and sound play on from
there (unless they were nearly over), PDFs and slides open at the page,
text at the line, documents at the paragraph, spreadsheets on the sheet and
the cell, archives with the same folders open and the same entry selected,
Markdown and HTML rendered or as source, pictures, PDFs and videos turned.
That, and the modes M, i and w last chose, for the minimap, for the rail
of pages and documents, for the strip of each family of files (pictures,
video and sound, text, pages, sheets, archives, cards), and for wrapping
text, is all silent remembers: two small files in `$XDG_RUNTIME_DIR/silent`,
gone at the next boot.

A file inside an archive opens in the same window: Enter, Space or a
double-click on it in the tree, once a click has given the tree the keys.
silent takes it out on the fly, into `$XDG_RUNTIME_DIR/silent/extract`
while it is small and `$XDG_CACHE_HOME/silent/extract` past 64 MB, and
shows it as it shows any file, the window taking its size about the same
centre; the strip names it by its place, `project.zip › src/main.rs`.
Previews lie on each other this way, and Space toggles the preview of the
selection, as in Nautilus: on the archive's selected entry it puts that
entry's preview on top, and anywhere else it takes the top preview off,
back to the archive with its folders, its selection and its scroll as they
were, one archive at a time when one was inside another, until the last
closes. Esc closes everything. The file taken out is gone at once. A slow
one shows how far it has come; Space stops it and stays in the archive,
Esc stops it and closes. Encrypted entries and anything over 4 GB are not
taken out, and the strip says why. What a crashed preview left there is
cleared at the next start.

## Keys

| Key | Action |
| --- | --- |
| Space | Toggles the preview of the selection: closes it, or, over the archive a file was taken out of, goes back to the archive; stops a file being taken out |
| Esc | Closes everything, after first dropping a half-typed vim command |
| Arrow keys | The previous or next file in Nautilus; after a click inside the content, see below |
| Enter | Open with the default application; for a folder, go into it |
| F2 | Rename in Nautilus |
| Delete | Move to the trash |
| F | Fullscreen |
| M | The minimap of text: shown while scrolling, docked beside the text, or none (a slim scrollbar instead); the rail of a PDF, slides or a document: docked or none; mute video and audio |
| i | The strip: shown while the pointer moves, never, or always; while it is never shown, a key that changes something (M, i, w, the volume, a seek, a pause) still answers with a word at the bottom right for a second |
| w | Wrap the long lines of text and code at the window's width, or scroll them sideways again |
| s | Markdown and HTML rendered, or their source |
| r, Shift+R | Turn a picture, every page of a PDF, or a video a quarter clockwise, or back; the window takes the turned shape |
| ? | The keys, on a card over the content; Esc takes it away first (F1 too, where the window manager leaves it) |
| K or a click | Pause or play video and audio |
| J, L | Back or forward ten seconds |
| Shift+← , Shift+→ | Half a screen sideways in text that does not wrap; back or forward thirty seconds in video and audio; the previous or next sheet of a spreadsheet |
| Shift+↑ , Shift+↓ | A screen up or down; the previous or next page of a PDF, or slide |
| Page Up, Page Down | A screen up or down |
| Home, End | The start or the end |
| Drag, double-click, triple-click | Select text, a word, a line; the selection is also the primary selection |
| Ctrl+A, Ctrl+C | Select all, copy |

Text, code, hexdumps, Word documents, PDFs and slides also move with vim's
keys. What has been typed of a command shows at the right of the strip,
as vim's `showcmd` does:

| Key | Action |
| --- | --- |
| j, k | A line down or up, a row in wrapped text; in a PDF or slides, the next or previous page |
| h, l, 0 | Sideways, and back to the left edge, in text that does not wrap |
| Ctrl+e, Ctrl+y | A line down or up, a row in wrapped text |
| Ctrl+d, Ctrl+u | Half a screen down or up |
| Ctrl+f, Ctrl+b | A screen down or up |
| gg, G | The start, the end |
| 20G | Line 20, in the middle of the view and lit for a moment; page 20 of a PDF or slides; the twentieth paragraph or table of a document |
| A number first | Does it that many times: 5j |

A click inside the content engages it, and a thin warm ring shows it: the
arrow keys act on the content instead of browsing files, until the window
loses the focus (a click in Nautilus) or shows another file. The click that
engages a video only engages it; the next one pauses.

| Content | ← → | ↑ ↓ |
| --- | --- | --- |
| Text, code, hexdumps, documents | Sideways, unless text wraps | A line, a row in wrapped text |
| PDF, slides | The previous or next page | Scroll |
| Spreadsheets | The cell cursor | The cell cursor |
| Archives | Close or open a folder | The previous or next entry; Enter or Space shows the file over the archive, or opens or closes the folder |
| Video, audio | Back or forward five seconds | Louder or quieter; the playbar shows the volume, and the next sound or video keeps it, and the mute |

The minimap, the rail and a document's miniature show where the view is
as a box: a click there brings that part into view, and a drag carries
the view along. Pointing at the slim scrollbar of text or a document puts
the line or the place in the strip. On the playbar, a click or a drag on
the track seeks, and on the volume sets it; the speaker mutes.

## Install

On Arch Linux, from this repository:

```sh
cd packaging/arch
makepkg -si
```

This builds the release binary and installs `/usr/bin/silent` and the D-Bus
service `org.gnome.NautilusPreviewer`, which is what Nautilus calls. The
package replaces sushi; pacman offers to remove it. Nautilus starts silent on
the next Space, with no restart. An upgrade takes over the same way: silent
notices its file replaced and leaves as soon as no preview is open, and the
next Space starts the new one.

silent needs X11, a Vulkan driver, glycin's image loaders with bubblewrap,
and GStreamer for video and audio. Its windows have the class `silent`;
window managers that tile should float it, for example in xmonad:
`className =? "silent" --> doCenterFloat`.

## Going back to sushi

```sh
sudo pacman -S sushi
```

pacman removes silent in the same step, since the two conflict.

## Other file managers

yazi, Nemo, ranger, lf, nnn, Thunar, PCManFM and Dolphin preview with
silent too: `silent show FILE` and its kin hand a file to the resident
previewer, as warm as Nautilus's, and a terminal file manager keeps the
keyboard while the preview follows its cursor. How to set each one up is
in [docs/integration.md](docs/integration.md).

## Development

`scripts/harness` makes test files, starts a private X display (3840×2160 at
163 DPI) and session bus, makes Nautilus's calls, and times cold starts:

```sh
scripts/harness fixtures
export DISPLAY=$(scripts/harness display)
scripts/harness cold "$PWD/target/release/silent" ~/.cache/claude-scratch/silent/fixtures/images/photo-4032x3024.jpg 10
```

`silent FILE` previews a file without D-Bus; `silent --probe FILE...` prints
what a preview would be and how long decoding takes. `SILENT_TRACE=1` prints a
mark at each step of the start-up on stderr, with the wall clock: the bus, the
fonts, the X11 window, the GPU's instance, device, swapchain and pipelines,
the first frame and its present. `SILENT_TRACE=frames` adds every frame's
ConfigureNotify, swapchain and present, as resizing the window shows them.

`scripts/bench` times silent against sushi on the desktop, from the ShowFile
call to the first frame that shows the file, read back from the X server:
cold, warm, with each previewer's process kept alive between previews as it
lives, and switching files in an open window.
It switches previewers with a service file of the user's and always puts the
user's back. The results are in [docs/benchmark.md](docs/benchmark.md):

```sh
scripts/bench fixtures
scripts/bench run --n 5
```

## License

silent is licensed under the Apache License, Version 2.0; see
[LICENSE](LICENSE).
