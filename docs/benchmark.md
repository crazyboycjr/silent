# silent against sushi

From Nautilus's ShowFile call to the first frame that shows the file, in ms: the median, and in brackets the 95th percentile, of 5 runs a cell. Measured by `scripts/bench run` on 2026-10-25, from outside both previewers: a call as Nautilus makes it, and the window's pixels read back from the X server until they settle. Each previewer is timed to the content, not to its window's map, which comes a little before it in both. Cold: no previewer process before the call. Warm: sushi's process kept alive between previews, as it normally lives; silent has no resident process, so it is always cold. Switch: the next file in an open window, as the arrow keys show it.

![Time to the first frame of each file](benchmark.svg)

This run is partial. The user needed the screen back during sushi's warm runs: its warm zip has 2 runs and its warm folder none, and the warm runs' switches and memory were not kept, so sushi's switch column holds the switches of its cold runs. sushi aborted on the MP3 in GStreamer's decodebin3 (`mq_slot_handle_stream_start: assertion failed: (collection)`), with its sound going to a null sink of the audio server's as silent's did, so its MP3 cells hold the one run it survived. Its switch from the zip to the folder is not timed: its cards for the two differ too little in the pixels the bench samples. silent was the build installed then (sha256 52d7a3ab…6d84): the start-up work and the playbar fix of this branch, with a new window's size settling for every resize rather than only before its first frame. Its switches that resize the window, to the JPEG, the Word, spreadsheet and presentation files and once to the zip, took 600–1000 ms on the desktop, while the same switches take 15–160 ms on a display without a window manager; that settling, since confined to a window's first frame, is the likely cause, and those cells want measuring again.

| File | silent, cold | sushi, cold | sushi, warm | silent, switch | sushi, switch | RSS, silent / sushi (MB) | VRAM, silent / sushi (MiB) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| JPEG, 12 MP | 238 (280) | 372 (381) | 135 (146) | 936 (976) | 150 (155) | 230 / 333 | 51 / – |
| PNG, 3840×2160 | 252 (259) | 397 (408) | 159 (164) | 215 (236) | 151 (161) | 222 / 288 | 35 / – |
| Animated GIF | 289 (369) | 260 (270) | 53 (56) | 70 (76) | 23 (30) | 166 / 240 | 208 / – |
| PDF, 3 pages | 321 (332) | 304 (386) | 81 (82) | 95 (114) | 66 (91) | 383 / 266 | 97 / – |
| PDF, 42 pages | 237 (257) | 268 (269) | 54 (57) | 76 (82) | 29 (33) | 261 / 286 | 225 / 88 |
| Video, 1080p | 457 (552) | 706 (712) | 579 (598) | 296 (367) | 466 (480) | 446 / 551 | 55 / – |
| MP3 with cover | 479 (494) | 426 (426) | – | 72 (76) | – | 175 / 305 | 19 / – |
| Source file, 9 KB | 187 (193) | 243 (287) | 53 (88) | 26 (28) | 61 (61) | 163 / 219 | 19 / – |
| Text, 2 MB | 180 (188) | 249 (254) | 48 (50) | 8 (164) | 27 (29) | 162 / 226 | 19 / – |
| Zip archive | 185 (194) | 266 (274) | 53 (54) | 9 (954) | 21 (22) | 164 / 214 | 19 / – |
| Folder | 179 (185) | 254 (260) | – | 4 (4) | – | 155 / 218 | 19 / – |
| Word document | 199 (201) | sushi can't | sushi can't | 964 (1001) | sushi can't | 171 / – | 19 / – |
| Spreadsheet | 176 (194) | sushi can't | sushi can't | 618 (621) | sushi can't | 158 / – | 19 / – |
| Presentation | 172 (187) | sushi can't | sushi can't | 958 (989) | sushi can't | 156 / – | 43 / – |

Office documents: sushi shows them only with LibreOffice installed, which is not.

Runs left out of the medians:

- sushi, switch, Folder: 5 of 5 runs without a frame (the window kept showing the file before)
- sushi, cold, MP3 with cover: 4 of 5 runs without a frame (the previewer died)
- sushi, switch, MP3 with cover: 5 of 5 runs without a frame (the previewer died)
- sushi, warm, MP3 with cover: 5 of 5 runs without a frame (no frame)
