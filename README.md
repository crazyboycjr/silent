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
  at the size they are shown. Folders show how many items they hold and how
  much they weigh. Everything else gets a card with its type, size and date.

## Formats

| Kind | Shown as |
| --- | --- |
| Images | The picture at its own aspect, as large as the screen allows; animations play |
| Folders | A card with the item count and the total size, counted in the background |
| Everything else | A card with the type, size and modification date |

Video, audio, PDF, text and code, and archives come next; Office documents,
Markdown and HTML, and fonts later.

## Keys

| Key | Action |
| --- | --- |
| Space, Esc | Close |
| Arrow keys | The previous or next file in Nautilus |
| Enter | Open with the default application; for a folder, go into it |
| F2 | Rename in Nautilus |
| Delete | Move to the trash |
| F | Fullscreen |

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

silent needs X11, a Vulkan driver, and glycin's image loaders with
bubblewrap. Its windows have the class `silent`; window managers that tile
should float it, for example in xmonad:
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
