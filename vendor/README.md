# gpui's Linux and wgpu backends, vendored

`gpui-pre-linux` and `gpui-pre-wgpu` 0.3.9 as crates.io publishes them,
taken in through `[patch.crates-io]` in the top `Cargo.toml`, with silent's
changes on top. The first commit that touches this directory adds them
unchanged, without their `Cargo.lock` and benchmarks, so `git log -p --
vendor` shows every change; in the code, `silent:` comments mark them.

- **Start-up marks**, everywhere: log records with the target
  `silent::trace`, which silent prints with `SILENT_TRACE` set, and with
  the target `silent::frames` each frame's configure, swapchain and
  present, printed with `SILENT_TRACE=frames`.
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

To take a newer gpui, copy its crates over these, drop their `Cargo.lock`
and benchmarks, and carry the changes over.
