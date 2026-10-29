# What silent shows

silent decides how to show a file by what is in it, not only by its name: a
text file without an extension reads as text, a gzipped JSON file opens as
laid-out JSON, and Chinese saved in GBK reads right.

| Kind | Shown as |
| --- | --- |
| Images | The picture at its own aspect, as large as the screen allows; animations play; zoomed with Ctrl and + or - or the wheel, and dragged once larger than the window |
| Video | Plays at its own aspect, with sound; hardware decoding when GStreamer has it; a playbar over the bottom of the picture, with the seek track, the times and the volume, while the pointer moves and after each seek |
| Audio | A card with the cover art, title, artist and album, playing, with the same playbar |
| PDF | The pages in a scroll, shaped by the first page, rendered as they come into view; page thumbnails docked at the right; text can be selected and copied; zoomed with Ctrl and + or - or the wheel |
| Markdown | Rendered: headings, lists, tables, code highlighted, pictures beside the file; `s` shows the source and back. A picture from the web shows its words: silent fetches nothing. Files over 256 KB open as their source |
| HTML | Laid out by [Blitz](https://github.com/DioxusLabs/blitz) as a browser would, with the page's own CSS (silent's paper, ink and fonts only where it sets none), its pictures and stylesheets beside the file, and nothing fetched: a picture from the web shows its words. Links open with the desktop or scroll to their place in the page, folded sections open and close with a click, text selects and copies; `s` shows the source and back. A page over 1 MB, or one whose layout takes over 0.4 s, opens as its source |
| Text and code | Highlighted, with line numbers; a minimap over the right edge while it scrolls, or docked beside the text; long lines scroll sideways, or wrap at the window's width with `w`; the text bigger or smaller with Ctrl and the wheel; select with the mouse and copy; UTF-8, UTF-16 and GB18030 (GBK) text; minified JSON laid out; the first 2 MB of large files |
| Binary files | As `hexdump -C` prints them, read from a memory map, so a dump of gigabytes opens at once, minimap and all; select and copy as text |
| Compressed files | gzip, xz, zstd, bzip2, lz4 and lzma files show what is inside: text and code as above, a tar as its listing, anything else as a hexdump; the info line gives both sizes when the format records them |
| Word documents | docx, odt and the older doc of Word 97 to 2003 (and WPS Writer's wps) as a column on a page: headings, paragraphs with their bold, italic, underline and colour, bulleted and numbered lists, tables, and the pictures of docx and odt; a minimap of the whole column docked at the right |
| Spreadsheets | xlsx, xls and ods as a grid with column letters and row numbers, numbers and dates formatted, a tab for each worksheet, opening on the one that was in front; a click puts a cell cursor down, and the info line shows its cell in full; chart and hidden sheets get no tab; old .xls written as a bare BIFF stream too; the first 5,000 rows; the grid's text bigger or smaller with Ctrl, as a document's |
| Presentations | pptx as its slides in a scroll, with text, pictures, tables and shapes where the deck puts them and what they inherit from the layout and master, thumbnails of the slides docked at the right, zoomed as PDFs are; odp as its thumbnail and text; the older ppt as an outline of each slide's title and text, sized as documents are |
| Keynote, Pages, Numbers | The preview picture the file carries, as Quick Look shows it, zoomed as pictures are |
| Archives | The entries as a tree with file-type icons and their sizes, and the total unpacked; zip, tar (gz, bz2, xz, zstd), 7z, rar, ISO and more; any file in them opens in the same window, archives in archives too |
| Folders | A card with the item count and the total size, counted in the background |
| Everything else | A card with the type, size and modification date |

## Office documents

silent opens an Office package and parses its XML itself, or the binary
streams of the formats of Office 97 to 2003, with no office suite, and draws
the result natively.
The faces a document names are drawn in installed families with the same
metrics, Carlito for Calibri and Caladea for Cambria among them.

## Pictures

Pictures are decoded by glycin's loaders, each in a sandbox of its own,
except JPEG and still PNG: silent decodes those itself, in pure Rust, JPEG
by libjpeg-turbo-rs, a port of libjpeg-turbo, and PNG by the decoder
glycin's loader uses. That saves the loader's process, some 35 ms a picture,
at the price of the sandbox for these two formats. A JPEG twice the size of
the screen or more is decoded at a half, a quarter or an eighth of its size,
and decoded again, sharper, when you zoom in.

## Files inside archives

A file inside an archive opens in the same window: Enter, Space or a
double-click on it in the tree, once a click has given the tree the keys.
silent takes it out on the fly, into `$XDG_RUNTIME_DIR/silent/extract`
while it is small and `$XDG_CACHE_HOME/silent/extract` past 64 MB, and
shows it as it shows any file, the window taking its size about the same
centre; the info line names it by its place, `project.zip › src/main.rs`.

Previews lie on each other this way, and Space toggles the preview of the
selection, as in Nautilus: on the archive's selected entry it puts that
entry's preview on top, and anywhere else it takes the top preview off,
back to the archive with its folders, its selection and its scroll as they
were, one archive at a time when one was inside another, until the last
closes. Esc closes everything. The file taken out is gone at once.

A slow file shows how far it has come; Space stops it and stays in the
archive, Esc stops it and closes. Encrypted entries and anything over 4 GB
are not taken out, and the info line says why. What a crashed preview left
behind is cleared at the next start.
