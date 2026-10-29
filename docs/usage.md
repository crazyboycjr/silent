# Using silent

Press Space on a file in Nautilus and silent shows it; Space or Esc closes
it, and the arrow keys move to the previous or next file. Everything else is
optional, and `?` shows the keys of the file in front of you on a card. This
page says the same in words, with what each view does.

## The info line

One line of facts about the file lies over the bottom of the content, and
`i` says when: when the mouse moves (it fades 1.5 s after the pointer rests,
a playbar 1 s after), never, or always. Its colours are worked out, not
chosen: silent looks at what is under it, the picture's own pixels, a
video's frames a few times a second, or the paper and the ink of text and
documents, and lays down a dark veil under white letters or a light one
under near-black letters, whichever needs less, as faint as it can be while
the letters keep a contrast of 4.5:1 (WCAG AA) against everything under
them. Over a dark or a pale picture that is no veil at all.

## Minimaps and thumbnails

An overview that is always shown never covers the content: it is docked
beside it, and the window is wider by the overview, at its right, so that
pressing `m` to dock or undock it leaves the content where it was on the
screen, as large as it was; near the screen's right edge the window moves
left just enough. Text shows its minimap over its right edge only while
scrolling, unless `m` docks it (always). PDFs and slides open with their
thumbnails docked, and documents with their minimap, within 85% of the
screen with the page; `m` takes them away (never), and the window narrows
by as much. Without an overview, a slim scrollbar shows while the view
scrolls.

The minimaps and the thumbnails show where the view is as a box: a click
there brings that part into view, and a drag carries the view along.
Pointing at the slim scrollbar of text or a document puts the line or the
place in the info line.

## Long lines

Long lines of text and code scroll sideways, and `w` wraps them at the
window's width instead, at the start of a word unless the word is wider
than the window, and between Chinese characters but not before their
commas; the window keeps its size and the line at the top stays there.
Wrapped, a line's number stands at its first row, the keys that move by a
line move by a row, `20G` still goes to line 20, the minimap and the
scrollbar show the rows, and copying gives the lines without the breaks.
Text wraps again when the window changes size, and the next text file
wraps too, until `w` again, in a window as tall as its rows. Hexdumps keep
their width.

## Text size and zoom

Ctrl+Shift+= makes the text of text and code, hexdumps, Markdown and HTML,
documents and spreadsheets bigger, Ctrl+- smaller and Ctrl+0 as it was, in
steps of about a tenth from 50% to 300%, and so does Ctrl with the wheel, a
step a notch. The window keeps its size, wrapped text flows again, and the
row under the pointer, or with a key the row at the top, stays where it
was. The info line says the size for a moment, and the next file opens at
it, in a window sized for it.

The same keys zoom PDFs, slides and pictures, from 50% to 400% of the size
they open at, each file at a zoom of its own: the next file opens to fit
its window, and coming back to one within the hour finds it as it was
left. The window keeps its size. With the wheel the place under the
pointer stays where it is; with a key, the top of the pages, or a
picture's middle. A step shows at once from what is drawn already, scaled,
and once the zoom rests a moment the view is drawn again at its new size:
the parts of the pages in view, so that a page zoomed fourfold costs no
more than one at its fit, and a picture decoded again from the file,
sharper as far as its own pixels go, smaller when zoomed out, a JPEG at the
fraction of its size the zoom needs; an animation plays on as it is.
Zoomed wider than the window, pages scroll sideways with Shift and the
wheel, `h` and `l`, and after a click the arrows, which go on to the next
page at the edge; a picture larger than the window moves with a drag.

## What silent remembers

A file you leave, by moving to another or closing the preview, comes back
as you left it if you return within the hour: video and sound play on from
there (unless they were nearly over), PDFs and slides open at the page,
text at the line, documents at the paragraph, spreadsheets on the sheet and
the cell, archives with the same folders open and the same entry selected,
Markdown and HTML rendered or as source, pictures, PDFs and videos turned,
PDFs, slides and pictures at their zoom.

That, the modes `m`, `i` and `w` last chose, for the minimap of text, for the
thumbnails of pages and the minimap of documents, for the info line of
each family of files (pictures, video and sound, text, pages, sheets,
archives, cards), and for wrapping text, and the text size, is all silent
remembers: two small files in `$XDG_RUNTIME_DIR/silent`, gone at the next
boot.

## Keys

`?` shows the keys on a card over the content, those of the file shown
first, then the general ones and Nautilus's; Esc, `?` or a click takes it
away (F1 shows it too, where the window manager leaves F1 alone). Letters
are written as vim writes them, as typed: `j` is the key and `J` is Shift
with it.

| Key | Action |
| --- | --- |
| Space | Toggles the preview of the selection: closes it, or, over the archive a file was taken out of, goes back to the archive; stops a file being taken out |
| Esc | Closes everything, after first dropping a half-typed vim command |
| Arrow keys | The previous or next file in Nautilus; after a click inside the content, see below |
| Enter | Open with the default application; for a folder, go into it |
| F2 | Rename in Nautilus |
| Delete | Move to the trash |
| f | Fullscreen |
| m | The minimap of text: while scrolling (over the right edge), always (docked beside the text), or never (a slim scrollbar instead); the thumbnails of a PDF or slides and the minimap of a document: always or never; mute video and audio |
| i | The info line: when the mouse moves, never, or always; while it is never shown, a key that changes something (m, i, w, the volume, a seek, a pause) still answers with a word at the bottom right for a second |
| w | Wrap the long lines of text and code at the window's width, or scroll them sideways again |
| s | Markdown and HTML rendered, or their source |
| Ctrl+Shift+=, Ctrl+-, Ctrl+0 | The text of text and code, Markdown and HTML, documents and spreadsheets bigger, smaller, or at 100%; PDFs, slides and pictures zoomed in, out, or to fit the window; Ctrl with the wheel too |
| r, R | Turn a picture, every page of a PDF, or a video a quarter clockwise, or back; the window takes the turned shape |
| k or a click | Pause or play video and audio |
| j, l | Back or forward ten seconds |
| Shift+← , Shift+→ | Half a screen sideways in text that does not wrap; back or forward thirty seconds in video and audio; the previous or next sheet of a spreadsheet |
| Shift+↑ , Shift+↓ | A screen up or down; the previous or next page of a PDF, or slide |
| Page Up, Page Down | A screen up or down |
| Home, End | The start or the end |
| Drag, double-click, triple-click | Select text, a word, a line (in an HTML page, a block); the selection is also the primary selection |
| Ctrl+a, Ctrl+c | Select all, copy |

Text, code, hexdumps, Word documents, PDFs and slides also move with vim's
keys. What has been typed of a command shows at the right of the info
line, as vim's `showcmd` does:

| Key | Action |
| --- | --- |
| j, k | A line down or up, a row in wrapped text; in a PDF or slides, the next or previous page |
| h, l, 0 | Sideways, and back to the left edge, in text that does not wrap; sideways on pages zoomed wider than the window |
| Ctrl+e, Ctrl+y | A line down or up, a row in wrapped text |
| Ctrl+d, Ctrl+u | Half a screen down or up |
| Ctrl+f, Ctrl+b | A screen down or up |
| g g, G | The start, the end |
| 2 0 G | Line 20, in the middle of the view and lit for a moment; page 20 of a PDF or slides; the twentieth paragraph or table of a document |
| A number first | Does it that many times: 5 j |

After a click inside the content, a thin warm ring shows that the arrow
keys act on the content instead of browsing files, until the window loses
the focus (a click in Nautilus) or shows another file. The first click in
a video only gives it the keys; the next one pauses.

| Content | ← → | ↑ ↓ |
| --- | --- | --- |
| Text, code, hexdumps, documents | Sideways, unless text wraps | A line, a row in wrapped text |
| PDF, slides | Sideways while zoomed wider than the window, then the previous or next page | Scroll |
| Spreadsheets | The cell cursor | The cell cursor |
| Archives | Close or open a folder | The previous or next entry; Enter or Space shows the file over the archive, or opens or closes the folder |
| Video, audio | Back or forward five seconds | Louder or quieter; the playbar shows the volume, and the next sound or video keeps it, and the mute |

## The mouse

On the playbar, a click or a drag on the track seeks, and on the volume
sets it; the speaker mutes. In text, documents and pages, a drag selects
and a double or triple click takes a word or a line, and Ctrl with the
wheel sizes the text or zooms the pages.
