# silent

Press Space on a file in Nautilus and silent shows it, in the middle of the
screen, at a size you can see. It takes the place of GNOME's sushi.

- **No border.** No title bar, no frame, no buttons: the window is the
  picture, or a card, and nothing else.
- **Minimal.** One line of facts about the file fades in when the pointer
  moves and fades out when it rests. The window follows the desktop's light
  or dark preference, in warm tones.
- **Blazing fast.** The window opens at its final size, centred, with the
  content already in it, well under a quarter of a second after the key
  press. Decoding runs beside the start of the toolkit and the GPU, and
  nothing stays in memory between previews.
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
| Video | Plays at its own aspect, with sound; hardware decoding when GStreamer has it; a seek bar after each seek and while the pointer is near the bottom |
| Audio | A card with the cover art, title, artist and album, playing |
| PDF | The pages in a scroll, shaped by the first page, rendered as they come into view; a rail of page thumbnails at the right while it scrolls; text can be selected and copied |
| Text and code | Highlighted, with line numbers; a minimap at the right while it scrolls; long lines scroll sideways instead of wrapping; select with the mouse and copy; UTF-8, UTF-16 and GB18030 (GBK) text; minified JSON laid out; the first 2 MB of large files |
| Binary files | As `hexdump -C` prints them, read from a memory map, so a dump of gigabytes opens at once, minimap and all; select and copy as text |
| Compressed files | gzip, xz, zstd, bzip2, lz4 and lzma files show what is inside: text and code as above, a tar as its listing, anything else as a hexdump; the strip gives both sizes when the format records them |
| Word documents | docx, odt and the older doc of Word 97 to 2003 (and WPS Writer's wps) as a column on a page: headings, paragraphs with their bold, italic, underline and colour, bulleted and numbered lists, tables, and the pictures of docx and odt |
| Spreadsheets | xlsx, xls and ods as a grid with column letters and row numbers, numbers and dates formatted, a tab for each worksheet, opening on the one that was in front; a click puts a cell cursor down, and the strip shows its cell in full; chart and hidden sheets get no tab; old .xls written as a bare BIFF stream too; the first 5,000 rows |
| Presentations | pptx as its slides in a scroll, with text, pictures, tables and shapes where the deck puts them and what they inherit from the layout and master, and a rail of the slides drawn small while it scrolls; odp as its thumbnail and text; the older ppt as an outline of each slide's title and text |
| Keynote, Pages, Numbers | The preview picture the file carries, as Quick Look shows it |
| Archives | The entries as a tree with file-type icons and their sizes, and the total unpacked; zip, tar (gz, bz2, xz, zstd), 7z, rar, ISO and more |
| Folders | A card with the item count and the total size, counted in the background |
| Everything else | A card with the type, size and modification date |

The Office files are read the way macOS Quick Look reads them: silent opens
the package and parses its XML itself, or the binary streams of the formats
of Office 97 to 2003, with no office suite, and draws the result natively. Rendered Markdown and HTML, and fonts, come later.

A file you leave, by moving to another or closing the preview, comes back
as you left it if you return within the hour: video and sound play on from
there (unless they were nearly over), PDFs and slides open at the page,
text at the line, documents at the paragraph, spreadsheets on the sheet and
the cell, archives with the same folders open and the same entry selected.
That, and the mode M last chose for the minimap and the thumbnail rail, is
all silent remembers: two small files in `$XDG_RUNTIME_DIR/silent`, gone at
the next boot.

## Keys

| Key | Action |
| --- | --- |
| Space, Esc | Close (Esc first drops a half-typed vim command) |
| Arrow keys | The previous or next file in Nautilus; after a click inside the content, see below |
| Enter | Open with the default application; for a folder, go into it |
| F2 | Rename in Nautilus |
| Delete | Move to the trash |
| F | Fullscreen |
| M | The minimap of text, or the thumbnail rail of a PDF or slides: shown while scrolling, always, or never (a slim scrollbar instead); mute video and audio |
| K or a click | Pause or play video and audio |
| J, L | Back or forward ten seconds |
| Shift+← , Shift+→ | Half a screen sideways in text; back or forward thirty seconds in video and audio; the previous or next sheet of a spreadsheet |
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
| j, k | A line down or up; in a PDF or slides, the next or previous page |
| h, l, 0 | Sideways, and back to the left edge |
| Ctrl+e, Ctrl+y | A line down or up |
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
| Text, code, hexdumps, documents | Sideways | A line |
| PDF, slides | The previous or next page | Scroll |
| Spreadsheets | The cell cursor | The cell cursor |
| Archives | Close or open a folder | The previous or next entry |
| Video, audio | Back or forward five seconds | Louder or quieter; the strip shows the volume, and the next sound or video keeps it, and the mute |

The minimap and the rail show where the view is as a box: a click there
brings that part into view, and a drag carries the view along.

## Install

On Arch Linux, from this repository:

```sh
cd packaging/arch
makepkg -si
```

This builds the release binary and installs `/usr/bin/silent` and the D-Bus
service `org.gnome.NautilusPreviewer`, which is what Nautilus calls. The
package replaces sushi; pacman offers to remove it. Nautilus starts silent on
the next Space, with no restart.

silent needs X11, a Vulkan driver, glycin's image loaders with bubblewrap,
and GStreamer for video and audio. Its windows have the class `silent`;
window managers that tile should float it, for example in xmonad:
`className =? "silent" --> doCenterFloat`.

## Going back to sushi

```sh
sudo pacman -S sushi
```

pacman removes silent in the same step, since the two conflict.

## Development

`scripts/harness` makes test files, starts a private X display (3840×2160 at
163 DPI) and session bus, makes Nautilus's calls, and times cold starts:

```sh
scripts/harness fixtures
export DISPLAY=$(scripts/harness display)
scripts/harness cold "$PWD/target/release/silent" ~/.cache/claude-scratch/silent/fixtures/images/photo-4032x3024.jpg 10
```

`silent FILE` previews a file without D-Bus; `silent --probe FILE...` prints
what a preview would be and how long decoding takes. `SILENT_TRACE=1` prints
start-up timings.
