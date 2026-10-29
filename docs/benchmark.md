# How fast silent is

silent 1.0 against GNOME's sushi 51.2, measured on 2026-10-29. Every number is
the time from Nautilus's call to the first frame that shows the file, in
milliseconds: the median of five runs, with the 95th percentile in brackets.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="benchmark-dark.svg">
  <img src="benchmark.svg" width="100%" alt="Two bar charts of the time to the first frame of fourteen files, silent against sushi, with each previewer already running and with each started for the file">
</picture>

## Already running, and started for the file

**Warm** is how both previewers usually answer: silent runs as long as
Nautilus does, and sushi stays for a while after each preview. **Cold** is a
new process for the file, as on the first Space after logging in.

| File | silent, warm | sushi, warm | silent, cold | sushi, cold |
| --- | ---: | ---: | ---: | ---: |
| JPEG, 12 MP | 94 (99) | 157 (180) | 187 (194) | 361 (380) |
| PNG, 3840×2160 | 133 (143) | 187 (199) | 193 (198) | 398 (421) |
| Animated GIF | 20 (29) | 22 (27) | 183 (191) | 262 (269) |
| PDF, 3 pages | 45 (55) | 87 (100) | 186 (194) | 284 (290) |
| PDF, 42 pages | 29 (32) | 20 (22) | 180 (190) | 257 (299) |
| Video, 1080p | 138 (147) | 517 (539) | 363 (375) | 657 (1069) |
| MP3 with cover | 18 (22) | not measured¹ | 175 (179) | not measured¹ |
| Source file, 9 KB | 5 (5) | 21 (81) | 188 (198) | 240 (247) |
| Text, 2 MB | 21 (23) | 22 (25) | 177 (183) | 248 (252) |
| Zip archive | 6 (9) | 26 (32) | 188 (188) | 243 (261) |
| Folder | 13 (36) | 20 (109) | 172 (184) | 240 (258) |
| Word document | 35 (56) | needs LibreOffice | 208 (222) | needs LibreOffice |
| Spreadsheet | 23 (24) | needs LibreOffice | 175 (180) | needs LibreOffice |
| Presentation | 19 (19) | needs LibreOffice | 182 (188) | needs LibreOffice |

Started cold, silent is faster on every file, by roughly a quarter to a
half. Warm, it is faster on most files: the animated GIF (20 against 22 ms)
and the 2 MB text (21 against 22) are ties, and sushi is faster on the
42-page PDF (20 against 29). sushi shows Office files only when LibreOffice
is installed, and it was not.

¹ In this setup sushi's process aborted on the MP3 in every run, inside
GStreamer's playback code. GStreamer's playbin and playbin3 play the file to
the end on their own, so the cause isn't settled and these cells are left
unmeasured.

## Switching files, and memory

A switch is the next file in a window that is already open, as the arrow
keys show it; each row is the switch to that file, made by a previewer that
was already running, as with Nautilus. The memory is the previewer's resident
set in the cold runs: a new process showing just that file.

| File | silent, switch | sushi, switch | silent, memory (MB) | sushi, memory (MB) |
| --- | ---: | ---: | ---: | ---: |
| JPEG, 12 MP | 87 (90) | 119 (142) | 192 | 328 |
| PNG, 3840×2160 | 129 (138) | 151 (159) | 199 | 287 |
| Animated GIF | 16 (32) | 23 (30) | 173 | 239 |
| PDF, 3 pages | 43 (46) | 34 (93) | 264 | 271 |
| PDF, 42 pages | 29 (30) | 28 (34) | 211 | 280 |
| Video, 1080p | 135 (210) | 230 (399) | 448 | 553 |
| MP3 with cover | 15 (16) | not measured¹ | 184 | – |
| Source file, 9 KB | 4 (4) | not measured² | 172 | 218 |
| Text, 2 MB | 12 (12) | 30 (33) | 170 | 225 |
| Zip archive | 4 (4) | 11 (21) | 174 | 214 |
| Folder | 4 (4) | not measured³ | 166 | 219 |
| Word document | 4 (4) | needs LibreOffice | 176 | – |
| Spreadsheet | 12 (15) | needs LibreOffice | 169 | – |
| Presentation | 4 (4) | needs LibreOffice | 168 | – |

Switching, sushi is ahead on the 3-page PDF and level on the 42-page one,
and silent is ahead on the rest.

² In the run's order this switch follows the MP3, where sushi had aborted.

³ sushi does switch from the zip to the folder, as a check on a test display
showed, but its two cards differ too little in the pixels the bench samples
for it to see when.

## How it is measured

`scripts/bench run` calls ShowFile as Nautilus does and watches the
previewer's window from the X server, reading its pixels back until they
settle. A previewer is timed from the call to the first frame that shows the
file, not to the moment its window maps, which comes a little earlier. Sound
goes to a null sink made for the run, and the previewer the run is not timing
is kept off the bus.

The machine: an AMD Ryzen 9 9950X (16 cores) with an NVIDIA RTX 4070 SUPER,
X11 at 3840×2160 with an X11 compositing manager and NVIDIA's
ForceFullCompositionPipeline on, otherwise idle; Nautilus 51.1 and
GStreamer 1.28.8. The run took two sittings the same afternoon; sushi's cold
video holds two runs from the first and three from the second. Every run is
in [benchmark.json](benchmark.json).

A first, partial comparison on 2026-10-25 timed silent before it stayed
running beside Nautilus, so every silent number in it was a cold start, under
a different compositor; its runs are in
[benchmark-2026-10-25.json](benchmark-2026-10-25.json).

## Run it yourself

The bench is [scripts/bench](../scripts/bench), and it times the previewers on
the X11 session you are logged into. It needs sushi installed beside silent,
`pactl` for the null sink that takes the sound, and Python 3 with numpy,
PyGObject and python-xlib. Its test files are made by `scripts/harness
fixtures`, which uses ffmpeg, ImageMagick, exiftool, zip, gzip, xz, zstd,
bzip2 and lz4, takes two PDFs from the graphviz and ghostscript
documentation, installs python-docx and a few others into a virtual
environment of its own, and keeps everything in `~/.cache/silent-harness`.

```sh
scripts/bench fixtures                                # the test files, once
scripts/bench run --n 5 --silent /usr/bin/silent      # about a quarter of an hour
```

For the run, the bench moves your D-Bus service file for
`org.gnome.NautilusPreviewer` aside, points the name at each previewer in
turn, and puts your file back however the run ends, then checks that silent
answers again. Previews open in the middle of your screen the whole time, so
leave the desk to it. `--previewers`, `--modes` and `--only` measure a part
of it, and `scripts/bench report RESULTS --markdown OUT.md` writes a table
and the charts from a run's results.
