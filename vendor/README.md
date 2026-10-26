# gpui's Linux and wgpu backends, vendored

`gpui-pre-linux` and `gpui-pre-wgpu` 0.3.9 as crates.io publishes them,
taken in through `[patch.crates-io]` in the top `Cargo.toml`, with silent's
changes on top. The first commit that touches this directory adds them
unchanged, without their `Cargo.lock` and benchmarks, so `git log -p --
vendor` shows every change; in the code, `silent:` comments mark them.

- **Start-up marks**, everywhere: log records with the target
  `silent::trace`, which silent prints with `SILENT_TRACE` set.
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

To take a newer gpui, copy its crates over these, drop their `Cargo.lock`
and benchmarks, and carry the changes over.
