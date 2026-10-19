# LightCraft in the browser

`apps/lightcraft-web` runs the same egui UI as the desktop app (`crates/ui-egui`) in the browser,
compiled to WebAssembly and drawn with WebGL2 (eframe's `glow` backend).

## Build and run locally

One-time setup:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version <the wasm-bindgen version in Cargo.lock> --locked
```

`cargo xtask web` checks the CLI against `Cargo.lock` and prints the exact install command if
it's missing or a different version.

Build, then serve:

```sh
cargo xtask web            # → <target>/web/{index.html, worker.js, lightcraft_web.js, lightcraft_web_bg.wasm}
cargo xtask web --serve    # build, then serve on http://127.0.0.1:8080/ (or `--serve 9000`)
cargo xtask web --dev      # unoptimized build with debug info (faster to compile, slow to run)
```

`<target>` is `target/` unless `CARGO_TARGET_DIR` is set. Any static HTTP server works, as long as
it serves `.wasm` as `application/wasm`. The bundle can't be opened from `file://` (module
workers and OPFS need an HTTP(S) origin; `localhost`/`127.0.0.1` count as secure). If `wasm-opt`
(binaryen) is on `PATH`, the release build also runs it over the module.

The release build uses the `web` Cargo profile (see *Bundle size* below) and writes gzip (`-9`)
and brotli (`-11`) precompressed copies next to each file (`*.gz`, `*.br`, pure Rust, printed
as a size table). `--serve` sends them with `Content-Encoding` when the browser accepts them;
configure a production server the same way (e.g. nginx `gzip_static`/`brotli_static`).

## Bundle size

Measured on the full bundle (every codec and raw decoder is in the module), without `wasm-opt`:

| `web` profile                                   | `.wasm` bytes | gzip -9 | brotli -11 | slider job* |
|-------------------------------------------------|--------------:|--------:|-----------:|------------:|
| before: release, thin LTO                       |    15 742 091 | 5 251 664 | 3 473 726 | 3.8 ms |
| fat LTO, 1 CGU, strip, panic=abort, opt 3       |    12 924 849 | 4 779 686 | 3 210 246 | 3.9 ms |
| same, opt-level "s"                             |    13 056 715 | 4 362 536 | 2 959 126 | 6.1 ms |
| same, opt-level "z"                             |    12 648 958 | 4 152 661 | 2 860 173 | 9.0 ms |
| **current:** "s", per-pixel crates at opt 3     |    13 533 126 | 4 583 465 | 3 093 780 | 4.0 ms |

\* median Exposure draft render of the `?bench` loupe in a worker (stage-cached), headless
Chrome. Size-optimizing the pipeline crates costs 50–130 % render time, so `Cargo.toml` keeps
them (pipeline, raster, color, develop, geom, raw, codecs, scenes, preview and the JPEG/PNG
codecs) at opt-level 3 and the rest (egui, eframe, serde, glue) at "s". What goes over the wire
is the brotli column: 3.1 MB, 11 % less than the old build's brotli size and 41 % less than its
gzip size (5.25 MB). `wasm-opt -Oz`, when installed, shrinks it further.

**Fonts.** The browser has no system fonts to fall back on, so Japanese text comes from
[craft-fonts](https://github.com/storytold/craft-fonts), the optional `CRAFT_FONTS_DIR` build
input (`CRAFT_FONTS_DIR=../craft-fonts cargo xtask web`; release builds always set it). On wasm32
`crates/engine/build.rs` embeds only BIZ UDPGothic Regular (UI, and the watermark fallback), so
the module stays well under Cloudflare's 25 MiB per-file limit: measured 2026-10-06, 17.1 MB
without craft-fonts and 21.8 MB with it (brotli 3.8 MB / 6.3 MB). Without it the web build
works, but Japanese text has no glyphs.

## Deploying: headers

`cargo xtask web --serve` sends these on every response. A production server may send them too;
today they are optional:

```
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Embedder-Policy: require-corp
Cross-Origin-Resource-Policy: same-origin
```

COOP + COEP make the page *cross-origin isolated* (`crossOriginIsolated === true`). That will be
**required** for `SharedArrayBuffer`, i.e. for a future wasm-threads build (`+atomics`, nightly
`build-std`), and gives full-resolution `performance.now()`. The current render workers (below)
don't share memory, so the app runs without them; with COEP on, every subresource must be
same-origin or send CORP/CORS headers (the bundle has no third-party resources). The file names
are the same in every version, so serve them with `Cache-Control: no-cache` (not `immutable`);
`packaging/web/README.md` (shipped as `HOSTING.md`) has the hosting details.

## What works

- **A persistent library.** The library lives in the browser's storage for the page's origin:
  [OPFS](https://developer.mozilla.org/docs/Web/API/File_System_API/Origin_private_file_system)
  when the main thread can write it (`FileSystemFileHandle.createWritable`: Chrome, Edge,
  Firefox, recent Safari), otherwise IndexedDB. Both hold the same layout:
  - `library/catalog.snap`, `library/catalog.log`: the same crash-safe journal as the desktop app
    (`lightcraft-catalog`), plus `presets.json`, `view.json`, `prefs.json` and `ui.json` (panel
    layout). The catalog `Store` is a memory mirror loaded at start-up; every change is flushed in
    the background within a frame or two, each file replaced atomically, in modification order
    (`apps/lightcraft-web/src/files.rs`). View state and UI prefs are saved every second when they
    change (a tab can close without notice).
  - The page holds a Web Lock for its lifetime when the browser supports it (see below).
    Browsers without Web Locks still require care to use only one tab per library.
  - `originals/<content hash>`: the bytes of every imported photo. The catalog refers to them as
    `web/<hash>/<file name>`; the main thread keeps recently used originals in memory (≤ 768 MB)
    and loads the rest on demand (the active photo is prefetched).
  - `thumbs/<key>.jpg` + `thumbs/index.json`: the rendered-thumbnail cache, with the desktop's
    budget (2 GB, least recently used pruned to 80 %).

  A new library starts with the procedural demo photos. URL options: `?store=idb` forces
  IndexedDB, `?store=memory` keeps nothing, `?reset` deletes the stored library first (after a
  confirmation: it deletes every imported photo too). Temporary sessions render on the main
  thread and retain accepted originals up to 768 MiB. Additional imports that exceed that limit
  are refused with a notice; existing session photos stay available for rendering and export.
- **Keeping the library safe** (experimental: the library lives only in this browser):
  - The app asks for persistent storage (`navigator.storage.persist()`); when the browser doesn't
    grant it, a notice says the library may be evicted under storage pressure.
  - **File ▸ Back Up Library…** snapshots the current catalog, presets, view and preferences,
    including unsaved edits, without writing to the live store. It downloads those library files and every stored original
    (`originals/<hash>/<file name>`, so an unzipped backup is browsable; entries are stored, not
    compressed; at most 4 GB). **File ▸ Restore Library from Backup…** reads such a zip into a
    new library folder (`library-restored-<time>/`; every entry's checksum is verified and
    originals must match their content hashes). Matching stored originals are reused; damaged
    stored copies are repaired from verified backup bytes. Only then does it switch to the
    restored library (`active-library` names the folder in use) and reloads: the previous library
    stays in storage. Both are web-only
    (`file.backupLibrary`, `file.restoreLibrary`; the desktop library is a folder).
  - A failed backing save (quota exceeded, storage cleared) shows the existing unsaved toast and
    top-bar warning even when the mirror accepted the command or initial snapshot and no later
    edit occurs. Later catalog writes report `saved in memory but not written` and stay queued.
    Saving is retried every 2 s; the warning clears only after the mirror and queued edits reach
    backing storage. Live backups keep both the edits and the failed-save state intact.
    Reloading or leaving requests the browser's standard unsaved-work confirmation; the browser
    controls whether it appears (usually requiring a prior user gesture). An explicitly accepted,
    successfully stored restore can reload into its new library; the previous library is kept.
    `web.stats` reports `saveError`, `unsaved`, `unsavedIndicator` and the current `toast`.
  - A picked or dropped photo whose bytes can't be stored is not added (it would be gone after
    a reload); a notice says why.
  - One tab at a time: the page holds a Web Lock (`navigator.locks`, `lightcraft-library`); a
    second tab or window shows "LightCraft is already open in another tab" instead of loading
    its own copy (two copies would overwrite each other's saves). A failed lock request shows
    an error with retry instructions instead of leaving startup waiting. Browsers without Web
    Locks aren't protected; `?store=memory` bypasses locking and never opens the stored library.
  - When OPFS is supported, failure to open its root stops startup with retry guidance; it
    never silently opens an IndexedDB library instead. IndexedDB is selected when explicitly
    requested or OPFS is unsupported.
  - A storage read failure, including the active-library pointer or a catalog file, stops startup
    with a visible retry message before the library is opened or saved. Reload to retry, or use
    `?store=memory` for a temporary session. Only a missing file is treated as absent.
  - If the catalog cannot be opened after its files are read, the library problem window opens
    and saving is disabled; the stored library is left as it was.
  - If the WebGL2 renderer cannot start, a visible blocking message explains the failure and
    asks you to reload to retry.
  - If the editor loses its WebGL context, controls outside the canvas let you export the
    active photo as PNG, back up the library and reload. They remain available after context
    restoration, with keyboard focus kept in the recovery controls. Export looks up the active
    photo when clicked and explicitly targets its ID, even if other selected originals are
    unavailable. No active photo produces an error and leaves the export button available to retry.
    Reload asks first because unsaved and session-only work would be lost.
  - A panic (wasm is built with `panic = "abort"`) replaces the page with a message saying the
    stored library is kept and to reload, instead of a frozen canvas.
- **Importing photos with no filesystem.** *File ▸ Import Photos…* (<kbd>⌘⇧I</kbd>) opens the
  browser's file picker. You can also drop files anywhere on the page. The bytes are written to
  storage, then imported and decoded by the same engine code as the desktop app
  (`lightcraft_engine::files::{probe_bytes, load_bytes}`): JPEG/PNG/TIFF/WebP and the supported
  raw formats. "Copy into library" is the same as "Add" here.
- **Rendering in Web Workers.** Renders don't run on the main thread: up to four dedicated
  workers (`hardwareConcurrency − 1`, `?workers=N` to override, `?workers=0` for the old inline
  path) each run a second instance of the same wasm module. `index.html` compiles the module once
  and posts the compiled `WebAssembly.Module` to `worker.js`. A job crosses as JSON (the photo's
  source, develop settings, request); the worker reads the original or the cached thumbnail from
  storage itself, keeps decoded sources and the loupe's stage cache, renders, and transfers the
  RGBA bytes back. Jobs for a photo go back to the worker that decoded it. This needs no wasm
  threads or `SharedArrayBuffer`. Workers become ready only after successfully opening the
  same storage backend as the page. A storage initialization failure or backend mismatch
  retires that worker and shows a notice; remaining workers or the main thread handle renders.
  If no worker starts, rendering falls back to the main thread. Uncached main-thread file
  jobs wait for their asynchronous original read before rendering, including `?workers=0`.
  Missing originals and read errors finish with the actual error and retry guidance; they do
  not repeatedly reread storage. Reload to retry a failed read.
  Temporary sessions always render on the main thread so imported originals remain accessible.
- **Every develop control** (sliders, curves, mixer, grading, masking, crop…) works as it does
  on the desktop, since it's the same crate.
- **Export downloads the file.** *Export…* (<kbd>⌘⇧E</kbd>) runs the same `app.export` path as
  the desktop app (`lightcraft_engine::export`: JPEG/PNG/TIFF/WebP, sizing, naming). The host's
  `write` service hands each file to the browser as a download instead of writing it to disk.
- **Automation.** `await lightcraft.command("library.info", "{}")` runs any engine or UI command
  by id on the next frame and resolves to the JSON result (`web.stats` reports storage, workers
  and the render queue). This is how the headless-Chrome checks drive the page.

## Not yet

- **Exports and auto-adjustments run on the main thread** (they need the original's pixels
  there). Right after a reload, the first such command on a photo whose original isn't in memory
  yet fails with "still loading" and works a moment later.
- **Durability is "a frame later", not fsync-before-return:** a crash or tab kill in the few
  milliseconds between a command and its flush loses that command.
- **Data-safety gaps still open** (issue #107): a panic in a decoder still ends the page (and a
  photo that panics while its thumbnail renders can do so again after a reload; there is no
  `?safe` start that skips rendering yet); removing photos doesn't delete their stored originals
  (`originals/<hash>`; they are kept, and included in backups); restored-over libraries stay in
  storage until the site's data is cleared; backups larger than 4 GB need zip64, which isn't
  written yet.
- **Preset files** (import/export `.json`) aren't wired to browser pickers yet.
- **Control channel / MCP.** These are desktop-only, because they need a TCP socket.
- **AVIF export** is untested in the browser; it's the one encoder that may not be wasm-safe.

## Measuring

Open `http://127.0.0.1:8080/?bench` to run a scripted measurement:

1. Wait for the grid thumbnails.
2. Open the first photo in Detail.
3. Drag Exposure through 8 steps, exactly as a slider drag does (begin interaction → `develop.set`
   ×8 → end).

The page then logs one console line:

```
lightcraft-bench {"first_frame_ms":…,"thumbs_done_ms":…,"slider_draft_ms":[…],"slider_draft_median_ms":…,"slider_job_ms":[…],"release_full_ms":…}
```

- `slider_draft_ms`: time from the `develop.set` command until the new loupe texture is ready.
- `slider_job_ms`: the pipeline's share of that time.
- `release_full_ms`: the full-quality render after the drag ends.

The parent-run lifecycle harness uses an already built bundle on a local HTTP server:
`node apps/lightcraft-web/tests/browser-lifecycle.mjs http://127.0.0.1:8080/`.
The default WEB-only checks use Grid/selection rendering and `web.stats`, including held-original
readiness with zero workers or failed workers, terminal read failures, and backing-write failures
at startup and after an accepted edit. They do not require a thumbnail-building feature.
For the combined feature gate, set `LIGHTCRAFT_THUMBNAIL_LIFECYCLE=required` to also run the separate
thumbnail plan/progress extension. It discovers availability with `engine.commands` and fails if
the feature is missing; `=1` instead reports an explicit skip when unavailable. Command or progress
errors propagate. `web.stats.activeThumbnailReady` checks the selected photo's rendered thumbnail
slot (excluding a pending request or an embedded stand-in); demo textures cannot satisfy its
readiness assertion. The backing-write recovery fixture keeps its refusal disabled across reload
and checks that the reloaded library remains saved. These scripts do not build WASM; actual browser
execution is a separate gate.
