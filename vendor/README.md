# gpui's Linux and wgpu backends, gpui-base and Blitz, vendored

`gpui-pre-linux` and `gpui-pre-wgpu` 0.3.9, `gpui-base` 0.8.0, and Blitz's
`blitz-dom` and `blitz-paint` 0.3.0-beta.4, as crates.io publishes them,
taken in through `[patch.crates-io]` in the top `Cargo.toml`, with
silent's changes on top. The first commit that touches each adds it
unchanged, without its `Cargo.lock` (and for the backends their
benchmarks), so `git log -p -- vendor` shows every change; in the code,
`silent:` comments mark them.

- **Start-up marks**, everywhere: log records with the target
  `silent::trace`, which silent prints with `SILENT_TRACE` set, and with
  the target `silent::frames` each frame's configure, swapchain and
  present, printed with `SILENT_TRACE=frames`. Each frame's line also tells
  the lines shaped, the glyphs rasterized and the tiles put in the atlas
  since the last frame, and how long the renderer took to acquire, encode
  and present it (`frame_tally.rs`).
- **Vulkan alone** (`wgpu_context.rs`): with gpui's XCB display, wgpu's GL
  backend asks EGL for its XCB platform, which NVIDIA's EGL lacks; Mesa's
  EGL then takes the NVIDIA card and fails, costing 90 ms a start and once
  1.8 s.
- **A device made from the start** (`wgpu_context.rs`, `wgpu_renderer.rs`):
  `WgpuContext::prepare` makes the Vulkan instance, adapter and device on a
  thread of their own, which silent calls as D-Bus starts it, before its
  toolkit and its X11 connection; the first window takes them, and its
  surface target carries the display handle, since the prepared instance
  has none.
- **The swapchain follows a new size at the next frame**
  (`wgpu_renderer.rs`), not at each ConfigureNotify, so that sizes which
  come and go in between cost no swapchain, which costs NVIDIA 3–75 ms.
- **A window opened hidden, and shown again** (`x11/window.rs`,
  `x11/client.rs`): a window opened with `show: false` is not mapped, for
  silent to map it when it has something to show; when it is unmapped, the
  next showing's first frame is marked as a first one again.
- **A window resized under the pointer follows it** (`x11/client.rs`,
  `x11/window.rs`, `wgpu_renderer.rs`): of the ConfigureNotify events read
  in one go, each window's last is handled, and the layout and the
  swapchain take its size without a round trip to the server; a frame
  whose swapchain the window outgrew while it was being made is drawn into
  one made again for the size the window has then, when the size changes
  slower than a swapchain is made, and a swapchain is never made again for
  a size the window has left. While the size keeps changing, frames are
  presented at once rather than at the vertical blank, and the window keeps
  its last frame at its top left (north-west bit gravity) until the next is
  drawn.

- **Mailbox under a compositor** (`x11/window.rs`): a window's frames
  present with mailbox when a compositor owns the screen's
  `_NET_WM_CM_S<n>` selection as the window is made, else FIFO. A FIFO
  present pending into a composited window makes NVIDIA's X driver wait
  for a vertical blank inside the compositor's GLX calls for every window
  resized meanwhile (6.6 ms a resize at 240 Hz, 14 ms at 60, against 0.9
  with mailbox), and blocks while the monitor's blanks stop. gpui's X11
  frames are paced by a timer at the monitor's refresh rate, not by the
  present, so mailbox draws no more frames than FIFO.

- **HTML paragraphs with code each have an id of their own**
  (`gpui-base`, `src/text/node.rs`): the text view's HTML parser records
  no source spans, so every paragraph laid out as an inline flow, which is
  any with `<code>` in it, had the element id `p`. They shared one element
  state, and each but the last was drawn with another's layout: its code
  backgrounds as empty bars and none of its text, as in gi-docgen's pages
  (`<pre><code>`, and inline code in the docblocks). A paragraph without a
  span is now named by its state's address, which lives as long as the
  parsed document. Drop this once gpui-base gives such paragraphs ids of
  their own.

- **Text resized without laying out the whole page** (`gpui-base`,
  `src/text/state.rs`): the text view's list measures every block, and a
  font or rem change measured them all again in its frame: 90 ms for a
  226 KB Markdown file at each step of the text size. A text view made
  with `remeasure_gradually()`, as silent makes its own, lays out the
  blocks in view at once, as in every frame, and measures the others again
  sixteen a frame from the second frame after the change, their old
  heights standing in meanwhile.

- **HTML pages as lists of their blocks** (`gpui-base`,
  `src/text/format/html.rs`): the parser gave a page as one block, which
  the list laid out whole in every frame, 150 ms for a 250 KB page, even
  to scroll. The containers around the blocks are opened, so the list
  holds the blocks as it does Markdown's. A container draws nothing of its
  own; what changes is the gap under a heading followed by a container's
  first block, which now follows the heading as in Markdown.

- **SVG text in the generic families fontconfig gives** (`blitz-dom`,
  `src/util.rs`): Blitz hands usvg a fontdb of the system's fonts, and
  fontdb takes each generic family from the last fontconfig `<alias>`
  that prefers something for it; urw-base35's
  `69-urw-standard-symbols-ps.conf` prefers Standard Symbols PS for
  `serif`, so the graphs in gi-docgen's pages (`Times,serif`) read in Greek
  letters. The generic families now come from fontique, which asks
  fontconfig as the page's own text does. `prepare_svg_fonts` finds the
  fonts as silent starts rather than on the first page with SVG in it.

- **A selection across anonymous blocks** (`blitz-dom`,
  `src/traversal.rs`): the blocks a selection runs through were found by
  a walk of the DOM, which saw an anonymous block (text and inline boxes
  straight in a block with blocks beside them) only at either end, so
  groff's contents, bare links between `<br>`s, were neither drawn
  selected nor copied. They are now taken in the order they are laid
  out, the DOM walk kept for an end that is not laid out.

- **List markers in their `::marker` colour** (`blitz-dom`,
  `src/layout/list.rs`, `src/node/element.rs`; `blitz-paint`,
  `src/render.rs`, `src/text.rs`): Blitz cascades no `::marker` style,
  so a bullet took its item's colour. A list item's `::marker` style is
  now computed when its marker is laid out, and an outside marker drawn
  in its colour where that differs, as silent's Markdown sheet asks for
  muted bullets as the text view draws them.

- **The selection in its `::selection` colour** (`blitz-paint`,
  `src/render.rs`, `src/text.rs`): Blitz drew selected text on a fixed
  light blue. It now takes the `::selection` background of the text's
  element or the nearest one above, as the page or silent's sheet sets
  it, silent's amber by default, and the blue only failing that.

To take a newer gpui or Blitz, copy its crates over these, drop their
`Cargo.lock` and benchmarks, and carry the changes over.
