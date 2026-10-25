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
| Video | Plays at its own aspect, with sound; hardware decoding when GStreamer has it; Shift shows a seek bar |
| Audio | A card with the cover art, title, artist and album, playing |
| PDF | The pages in a scroll, shaped by the first page, rendered as they come into view; text can be selected and copied; Shift shows a page scrubber |
| Text and code | Highlighted, with line numbers; long lines scroll sideways instead of wrapping; select with the mouse and copy; UTF-8, UTF-16 and GB18030 (GBK) text; minified JSON laid out; the first 2 MB of large files |
| Binary files | As `hexdump -C` prints them, read from a memory map, so a dump of gigabytes opens at once; select and copy as text |
| Compressed files | gzip, xz, zstd, bzip2, lz4 and lzma files show what is inside: text and code as above, a tar as its listing, anything else as a hexdump; the strip gives both sizes when the format records them |
| Word documents | docx and odt as a column on a page: headings, paragraphs with their bold, italic, underline and colour, bulleted and numbered lists, tables and pictures |
| Spreadsheets | xlsx, xls and ods as a grid with column letters and row numbers, numbers and dates formatted, a tab for each worksheet, opening on the one that was in front; chart and hidden sheets get no tab; old .xls written as a bare BIFF stream too; the first 5,000 rows |
| Presentations | pptx as its slides in a scroll, with text, pictures, tables and shapes where the deck puts them and what they inherit from the layout and master; odp as its thumbnail and text; Shift shows a slide scrubber |
| Keynote, Pages, Numbers | The preview picture the file carries, as Quick Look shows it |
| Archives | The entries as a tree with file-type icons and their sizes, and the total unpacked; zip, tar (gz, bz2, xz, zstd), 7z, rar, ISO and more |
| Folders | A card with the item count and the total size, counted in the background |
| Everything else | A card with the type, size and modification date; old .doc and .ppt files too |

The Office files are read the way macOS Quick Look reads them: silent opens
the package and parses its XML itself, with no office suite, and draws the
result natively. Rendered Markdown and HTML, and fonts, come later.

A video or a sound that you leave, by moving to another file or closing the
preview, starts again where you left it if you come back within the hour,
unless it was nearly over. That is the only thing silent remembers: one
small file in `$XDG_RUNTIME_DIR/silent`, gone at the next boot.

## Keys

| Key | Action |
| --- | --- |
| Space, Esc | Close |
| Arrow keys | The previous or next file in Nautilus |
| Enter | Open with the default application; for a folder, go into it |
| F2 | Rename in Nautilus |
| Delete | Move to the trash |
| F | Fullscreen |
| K or a click | Pause or play video and audio |
| J, L | Back or forward ten seconds |
| M | Mute |
| Shift, held | A seek bar on a video, a scrubber on a PDF, slides or text; click or drag on them |
| Shift+← , Shift+→ | Back or forward five seconds; the previous or next sheet of a spreadsheet |
| Shift+↑ , Shift+↓ | The previous or next page of a PDF, or slide |
| Shift+Home, Shift+End | The first or last page or slide; the start or end of text, a document or a sheet |
| Page Up, Page Down | Scroll a document, a sheet or text |
| Shift with the wheel | Scroll sideways |
| Drag, double-click, triple-click | Select text, a word, a line; the selection is also the primary selection |
| Ctrl+A, Ctrl+C | Select all, copy |

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
