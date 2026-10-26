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
  thread of their own, which silent calls as soon as it has a file to show,
  before its toolkit, its X11 connection and its loading; the first window
  takes them, and its surface target carries the display handle,
  since the prepared instance has none.
- **A new window's size settles before its swapchain** (`wgpu_renderer.rs`):
  until the first frame is presented, a new drawable size waits 32 ms
  before the swapchain follows it, so that a window manager's border
  adjustment that comes and goes costs no swapchain, which costs NVIDIA
  15–75 ms. Later resizes make theirs at once, as gpui does.

To take a newer gpui, copy its crates over these, drop their `Cargo.lock`
and benchmarks, and carry the changes over.
