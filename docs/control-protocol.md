# Control protocol

`lightcraft --control 7980` (or `LIGHTCRAFT_CONTROL_PORT=7980`) starts a JSON-lines server on
`127.0.0.1:7980` (loopback only). One request per line, one reply per line, in order:

```text
→ {"id": 1, "method": "engine.execute", "params": {"command": "photo.rate", "params": {"rating": 4}}}
← {"id": 1, "ok": true, "result": null}
← {"id": 2, "ok": false, "error": "unknown command `nope`"}
```

Requests are answered on the UI thread between frames (timeout 60 s).

**Only requests are read.** Every line must be a JSON object with a string `method` (`id` and `params` are
optional; blank lines are skipped). Anything else — text that isn't JSON, a JSON array or number, an object without
`method`, invalid UTF-8, or a line longer than 4 MiB — gets one error reply
(`{"ok": false, "error": "… closing the connection"}`) and the server **closes the connection**; nothing sent after
it on that connection runs. This keeps an HTTP request (for example a web page's cross-origin `fetch` to
`127.0.0.1:<port>`) from smuggling a command in its body: its request line is rejected first. Junk never reaches the
UI thread, and at most 16 connections are served at once (further ones get an error line and are closed). Clients
that hit an error reply should reconnect. The port has no authentication, so only enable it when you need it. The MCP server's connect
mode ([mcp.md](mcp.md)) is a thin layer over this channel. Implementation:
`crates/ui-egui/src/control.rs` (methods) and `apps/lightcraft/src/control_server.rs` (transport).

## Methods

| Method | Params | Result |
|---|---|---|
| `engine.execute` (alias `ui.menu.invoke`) | `{command, params?}` | Run any engine or UI command (see `engine.commands`) |
| `engine.commands` | — | Engine + UI commands: id, label, menu, shortcut, params doc, enabled |
| `ui.menu.list` | — | Menu entries (flat: id, label, menu path, shortcut, enabled) |
| `ui.menu.tree` | — | The menu bar as shown (File … Help): items `{id, params?, label, shortcut?, enabled, checked?}`, separators, submenus — the model behind the native macOS menu bar and the in-window menus |
| `ui.inspect` | — | UI state, window, `loupe` (`source`, and `region: {full, window, pending}` when a zoomed view also renders the window on screen at no more than 100 % — `full` is the frame it is cut from, `window` its `[x, y, w, h]` — and `regionBefore` for the Before side of a Before/After view), canvas/image rects, `scroll: {grid, filmstrip}` (scroll offsets in points, `null` until drawn), active photo, selection, perf (`frameMs` = layout, `logicMs` = per-frame logic before it, `updateMs` = both, `maxUpdateMs`, `fps`, render queue …, `gpu` = adapter in use, `gpuReason` = why renders don't use the GPU, `gpuFallback` = latest render redone on the CPU and why — see `docs/gpu-pipeline.md`), status, memory (bytes per cache, see `library.memory`; plus stage caches — `budgetBytes`, `trimmed`, `sharedSourceBytes` (the one device copy of a big original) — and textures), `export: {running: {total, done, current} \| null, last}`, `notices` (warnings waiting to be shown, e.g. a damaged settings file; OK = `button:noticeOk`), `quitPrompt` (why quitting was stopped: unsaved changes; `button:quitRetry` / `button:quitAnyway` / `button:quitCancel`), `import: {done, total, imported, cancelled} \| null` (an import runs on a worker thread), `tasks` (other background work: `Find Missing Photos`, `Auto Import`), `fileDialogs` (commands waiting on a native file dialog, which runs off the UI thread; they run again with the answer when it closes) |
| `ui.widgets` | `{filter?}` | On-screen widgets `{id, rect: [x, y, w, h]}` (screen points) |
| `ui.clickWidget` / `ui.dragWidget` / `ui.hoverWidget` | `{id, count?, fx?, fy?}` / `{id, toX?, toY?, dx?, dy?, steps?}` / `{id, fx?, fy?}` | Real egui input on a widget (hover: the pointer rests on it, e.g. for preset/profile previews) |
| `ui.move` / `ui.click` / `ui.drag` | `{x, y, count?, button?}` / `{x, y, toX, toY, steps?}` | Raw pointer input, screen points |
| `ui.pointer` | `{events: [{kind: down\|drag\|up, x, y}], alt?, shift?, cmd?}` | Gesture in normalized image coordinates (Detail view) |
| `ui.key` | `{key, cmd?, shift?, alt?, ctrl?}` | Key press |
| `ui.text` | `{text}` | Text input |
| `ui.scroll` | `{dx, dy, cmd?, ctrl?, shift?, alt?}` | Wheel / two-finger scroll at the current pointer; pans over the image, modifier-scroll zooms |
| `ui.zoom` | `{factor}` | Pinch zoom at the current pointer (positive scale multiplier; 1 = unchanged). Position it first with `ui.move` or `ui.hoverWidget` |
| `ui.set` | partial UI state, e.g. `{"view": "detail"}` | Resulting UI state |
| `ui.dialog.confirm` / `ui.dialog.cancel` | — | Close the open dialog |
| `ui.resize` | `{width, height}` | Resize the window |
| `ui.screenshot` | `{path?, headless?}` | `{path, width, height}` once the frame (with finished renders) is captured. `headless: true` draws the UI on the CPU (no compositor needed); a windowed capture that gets no frame within 2 s falls back to headless automatically |
| `engine.execute {command: "app.export", params}` | export params (see `docs/mcp.md`), plus `preset`, `dir` / `path`, `ids`, `background` | Writes the files and returns `{files}`; with `background: true` (what the Export dialog and menus use) it returns `{background: true, total}` at once and the batch runs on a worker thread — poll `ui.inspect` → `export` |
| `ui.render` | `{id?, size?, path?}` | Render a photo (PNG to `path`), `{width, height}` |
| `app.quit` | — | Close the app |

Image navigation is also available directly as the UI command `view.navigate`, with
`{zoom?: "fit" | "fill" | {"percent": number}, pan?: [x, y]}`. Percentage zoom accepts fractional
values greater than 0 and at most 800; pan is the normalized image centre, with coordinates from 0 to 1.
Pinching keeps the image point under the pointer steady and zooms between Fit and 800%; two-finger
scrolling pans in both axes and respects the operating system's scrolling direction and momentum.
These gestures work in Detail (including editing tools and full-screen preview), Compare and Reference
views, and only apply over their image areas. Panning stops at the image edges.

### When the library can't be saved

With a persistent library, every command that changes something is written to the catalog journal (fsynced) before
it replies. If that write fails (disk full, volume gone, permissions), the command replies `ok: false` with
`"saved in memory but not written to disk: <reason>; LightCraft will retry"`. The change itself **is** applied (and
undoable) and stays queued: the next command, and the app's frame loop every couple of seconds, retry the write, so
nothing is lost once the disk is writable again — unless the app quits first. Meanwhile `ui.inspect` → `unsaved`
is `{ops, error}` (else `null`), `library.info` reports `unsavedOps` / `unsavedError`, and the top bar's cloud icon
shows a warning (widget `indicator:unsaved`). Queries and commands that change nothing still succeed. A failed
compaction (snapshot) is not a failed command — the log is kept whole — and only shows in `library.info` →
`lastError`.

### When the library can't be opened

If the desktop app can't open its library at launch (another program has it open, an unreadable or newer-format
catalog, a missing drive), the session starts empty and in memory — never with demo photos — and a window asks what
to do: `ui.inspect` → `libraryProblem` is `{path, error, temporarySession, pendingImport}` (else `null`); its buttons
are `button:libraryRetry`, `button:libraryChoose`, `button:libraryTemporary` (Continue Without Saving) and
`button:libraryQuit`. A temporary session shows a banner (`indicator:temporarySession`, `button:libraryReopen`) and
writes nothing. `app.openLibrary` opening a library ends it.

## External controllers

External apps can use this channel to send commands and poll the running desktop app for feedback
(e.g. knob values or LEDs). Start the app with `--control PORT`, then keep a JSON-lines connection to
`127.0.0.1:PORT`. Discover command ids, parameters and current enablement with `engine.commands`;
use control ids and ranges from `develop.controls` rather than translated labels or widget positions.

| Task | Command through `engine.execute` | Result / effect |
|---|---|---|
| Read selection and active photo | `library.state {}` | `selection: {ids, active}`; `active` is `null` when no photo is active |
| Read photo identity and settings together | `photo.inspect {}` | Active photo's `id`, `develop`, rating, flag and history in one reply |
| Read a specific photo without selecting it | `photo.inspect {id}` / `develop.get {id}` | Photo details / full develop JSON; omitting `id` reads the active photo |
| Read global slider metadata and values | `develop.controls {section?}` | Array of `{id, label, section, min, max, default, step, decimals, track, value}` for the active photo |
| Set absolute slider values | `develop.set {values: {"light.exposure": 0.7}}` | Undoable edit of the active photo; also accepts `{control, value}` |
| Move a relative control | `develop.adjust {control: "light.exposure", delta: 0.1}` | Adds to the current value, clamped to the control's range |
| Change photo | `library.select {ids: [photoId]}` / `library.next {}` / `library.previous {}` | Changes the active photo; next/previous follow the current view's order |
| Undo / redo | `edit.undo {}` / `edit.redo {}` | Restore settings; poll again for feedback |

For example, send these requests one at a time and read each reply before sending the next:

```json
{"id":1,"method":"engine.commands"}
{"id":2,"method":"engine.execute","params":{"command":"photo.inspect"}}
{"id":3,"method":"engine.execute","params":{"command":"develop.controls","params":{"section":"light"}}}
{"id":4,"method":"engine.execute","params":{"command":"develop.set","params":{"values":{"light.exposure":0.7}}}}
{"id":5,"method":"engine.execute","params":{"command":"photo.inspect"}}
```

Replies use `{id, ok, result}` or `{id, ok: false, error}`. In the photo snapshot, exposure is
`result.develop.light.exposure`; the controls array's matching `light.exposure` entry has `value`,
`min: -5`, `max: 5` and `step: 0.01`. Values use the control's units (`wb.temp` is Kelvin, even when
the UI shows a relative temperature scale). Re-read values after edits to reflect the stored result.
Local mask adjustments are in the photo snapshot's `develop.masks`; use the mask commands from the
registry (e.g. `mask.adjust`) for those, rather than treating global control values as mask values.

Feedback uses polling: the channel sends replies to requests, with no unsolicited change events.
Compare successive `photo.inspect` snapshots for photo-id or develop-setting changes, including GUI
edits, photo navigation, presets and undo/redo. GUI slider updates are visible while a drag is in
progress; ending the interaction commits it as one undo step. A controller can group its own drag
with `develop.beginInteraction {label}` and `develop.endInteraction {}` using the same commands.
Update controller feedback from reads without sending the same values back as new edits.

Each request is a separate snapshot. `photo.inspect` keeps identity and settings together; a
`develop.controls` reply has no photo id, so do not associate it with an earlier cached selection
when the user is changing photos. Refresh the selection and retry those reads after navigation.
With no active photo, `develop.controls` returns default values and `photo.inspect` / `develop.get`
without `id` return an error; check `library.state` before treating defaults as photo feedback.
Re-read state after reconnecting. Polling observes the state at each read, not every intermediate change.

The same queries and edits are available through the connected CLI:

```sh
lightcraft-cli run --connect 127.0.0.1:7980 library.state photo.inspect develop.controls section=light
lightcraft-cli run --connect 127.0.0.1:7980 develop.adjust control=light.exposure delta=0.1 photo.inspect
```

For MCP, use `lightcraft-cli mcp --connect 127.0.0.1:7980`: `run_command` reaches the commands above,
`list_controls` / `set_develop` provide slider helpers, and `resources/read` polls
`lightcraft://library`, `lightcraft://photo/active`, `lightcraft://develop/active` and
`lightcraft://controls` ([mcp.md](mcp.md#resources)). Headless CLI/MCP mode owns a separate engine
session; use connect mode for feedback from edits in the desktop app.

## Headless rendering (no window, no GPU)

The egui UI can be rasterized on the CPU (`crates/ui-egui/src/softpaint.rs`, driven by
`crates/ui-egui/src/headless.rs`): same tessellation, gamma-space premultiplied blending and
scissor clipping as egui's GPU backends, so the image matches the window (minus GPU dithering).

- **In the running app:** `{"method": "ui.screenshot", "params": {"path": "a.png", "headless": true}}`.
  The UI is drawn into an offscreen context from the app's logic tick, which keeps running when the
  window is occluded or the display sleeps/locks. Windowed screenshots fall back to this after 2 s.
- **Without any app window:** `lightcraft-cli snapshot` runs a whole app session headlessly.
  Photo development also defaults to the CPU, so the initial frames do not discover a GPU adapter
  or load a graphics driver. It answers the same control requests (same handler) from a JSON-lines script:

```text
lightcraft-cli snapshot --demo -o grid.png --size 1600x1000 [--scale 2]
lightcraft-cli snapshot --library DIR --script tour.jsonl -o shot.png
```

Headless screenshot dimensions must be finite and at least one logical point, with a finite,
positive scale. Rounded output must be at least one pixel per edge, at most 16,384 pixels
per edge and 64 million pixels in total. `snapshot` and `ui.resize` reject requests outside
these limits before layout or rasterization; supplied resize dimensions must be numbers.
UI zoom changes text and control sizes while preserving the snapshot viewport's requested
physical pixel dimensions. Later `ui.resize` requests use the current egui point scale,
matching windowed resize behavior.

  `tour.jsonl` holds one request per line (`#` comments allowed), e.g.
  `{"method": "ui.set", "params": {"view": "detail", "right": "edit", "openSections": ["optics"]}}`
  then `{"method": "ui.screenshot"}`. Replies are printed to stdout. A failed request
  (`"ok": false`) does not stop the script — the remaining lines and the final screenshot still
  run — but the exit status is non-zero when any request failed, as with `run --keep-going`, so
  CI and nightly runs can judge a snapshot by its exit status. A `ui.screenshot` without
  `path` writes `-o` (then `OUT-2.png`, `OUT-3.png`, …); `ui.settle {timeoutMs?}` waits until no
  renders are in flight. Each request runs frames until it is answered and its injected input
  (clicks, keys, drags) has played out. Widget ids for `ui.clickWidget` come from `ui.widgets`
  (e.g. `button:upright-auto`). A 1600×1000 demo snapshot takes ~1–2 s (debug build).
