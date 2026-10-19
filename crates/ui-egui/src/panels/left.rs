//! The left sidebar: library sources, albums, Local folders, and library-wide filters.

use egui::{Align2, Rect, Sense, pos2, vec2};
use lightcraft_catalog::{Album, AlbumId, FolderNode, KeywordNode};
use lightcraft_engine::LibrarySource;
use serde_json::json;

use crate::LightcraftApp;
use crate::icons::{Icon, paint};
use crate::theme::Tokens;
use crate::widgets::{icon_button, register};

/// How wide the sidebar's content needs to be, from the widest row of the last frame: rows are
/// drawn at this width (or the panel's, if wider), and the sidebar scrolls sideways when it
/// exceeds the panel. One frame behind, which needs no second layout pass.
pub(crate) fn content_width(ctx: &egui::Context) -> f32 {
    ctx.data(|d| d.get_temp::<f32>(egui::Id::new("left-content-width"))).unwrap_or(0.0)
}

/// The most a row asks for its name: longer ones are cut when there is no more room.
const MAX_NAME_NEED: f32 = 140.0;

/// A row says how wide it needs to be.
fn note_width(ui: &egui::Ui, w: f32) {
    ui.data_mut(|d| {
        let m = d.get_temp_mut_or_insert_with::<f32>(egui::Id::new("left-content-width-next"), || 0.0);
        *m = m.max(w);
    });
}

fn row(
    app: &mut LightcraftApp,
    ui: &mut egui::Ui,
    id: &str,
    icon: Icon,
    label: &str,
    count: Option<usize>,
    selected: bool,
    indent: f32,
) -> egui::Response {
    row_named(app, ui, id, icon, label, None, count, selected, indent)
}

/// [`row`] whose spoken name is `spoken` when the painted `label` is a shortened form of it.
fn row_named(
    app: &mut LightcraftApp,
    ui: &mut egui::Ui,
    id: &str,
    icon: Icon,
    label: &str,
    spoken: Option<&str>,
    count: Option<usize>,
    selected: bool,
    indent: f32,
) -> egui::Response {
    let label = if matches!(id, "all" | "recentlyAdded" | "picks" | "missing" | "recentlyDeleted") { crate::i18n::tr(label) } else { label };
    let t = Tokens::get(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 29.0), Sense::click());
    register(ui.ctx(), format!("source:{id}"), r);
    let said = spoken.unwrap_or(label);
    let name = match count {
        Some(n) => crate::i18n::tr_format!("{label}, {n} photos", label = said, n = n),
        None => said.to_string(),
    };
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &name));
    // what is visible of the row: when the sidebar is scrolled sideways its bars and counts end at
    // the panel's edge, not at the end of the (wider) content
    let visible_right: f32 = ui.data(|d| d.get_temp(egui::Id::new("left-visible-right"))).unwrap_or(f32::MAX);
    let edge = r.right().min(visible_right);
    let inner = Rect::from_min_max(r.min + vec2(8.0, 0.0), pos2((r.right() - 8.0).min(edge - 8.0).max(r.left() + 8.0), r.bottom()));
    if selected {
        ui.painter().rect_filled(inner, 4.0, t.canvas);
        register(ui.ctx(), format!("highlight:{id}"), inner);
    } else if resp.hovered() {
        ui.painter().rect_filled(inner, 4.0, t.hover.gamma_multiply(0.6));
    }
    paint(
        ui.painter(),
        Rect::from_min_size(pos2(r.left() + 18.0 + indent, r.center().y - 8.0), vec2(16.0, 16.0)),
        icon,
        if selected { t.text } else { t.icon },
    );
    let font = t.font(13.5);
    let color = if selected { t.text } else { t.text_label };
    let count_galley = count.filter(|_| app.ui.show_counts).map(|n| ui.painter().layout_no_wrap(n.to_string(), t.font(12.5), t.text_dim));
    let label_left = r.left() + 42.0 + indent;
    let count_left = count_galley.as_ref().map_or(edge - 18.0, |g| edge - 18.0 - g.size().x);
    // the name gives way to the count: cut with an ellipsis, in full on hover
    let room = count_left - 8.0 - label_left;
    let measure = |s: &str| ui.painter().layout_no_wrap(s.to_string(), font.clone(), color).size().x;
    let full_w = measure(label);
    let shown = if full_w <= room { label.to_string() } else { crate::widgets::elide_head(label, room.max(0.0), measure) };
    let label_rect = ui.painter().text(pos2(label_left, r.center().y), Align2::LEFT_CENTER, &shown, font.clone(), color);
    register(ui.ctx(), format!("label:{id}"), label_rect);
    let resp = if shown != label { resp.on_hover_text(label) } else { resp };
    // a row asks for room for its name up to a share of a panel, so one very long name does not
    // make everything scroll: depth does that
    let mut needed = 42.0 + indent + full_w.min(MAX_NAME_NEED) + 18.0;
    if let Some(galley) = count_galley {
        let rect = Rect::from_min_size(pos2(edge - 18.0 - galley.size().x, r.center().y - galley.size().y / 2.0), galley.size());
        needed += rect.width() + 16.0;
        register(ui.ctx(), format!("count:{id}"), rect);
        ui.painter().galley(rect.min, galley, t.text_dim);
    }
    note_width(ui, needed);
    let _ = app;
    resp
}

/// A collapsible section header (Albums, Local, By Date, Keywords): the bold title with a
/// disclosure chevron after it; a click folds or unfolds the section (kept in the UI state, so it
/// survives restarts). Returns the header's rect and whether the section is now open.
fn sidebar_section_header(app: &mut LightcraftApp, ui: &mut egui::Ui, id: &str, title: &str) -> (Rect, bool) {
    let t = Tokens::get(ui.ctx());
    let title = crate::i18n::tr(title);
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
    register(ui.ctx(), format!("sidebarSection:{id}"), r);
    if resp.clicked() {
        app.ui.toggle_sidebar_section(id);
    }
    let open = !app.ui.sidebar_section_collapsed(id);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::CollapsingHeader, true, open, title));
    let text = ui.painter().text(pos2(r.left() + 18.0, r.center().y), Align2::LEFT_CENTER, title, t.semibold(13.5), t.text_label);
    let c = pos2(text.right() + 10.0, r.center().y);
    let col = if resp.hovered() { t.text } else { t.text_dim };
    let pts = if open {
        vec![c + vec2(-4.0, -2.0), c + vec2(4.0, -2.0), c + vec2(0.0, 3.0)]
    } else {
        vec![c + vec2(-2.0, -4.0), c + vec2(3.0, 0.0), c + vec2(-2.0, 4.0)]
    };
    ui.painter().add(egui::Shape::convex_polygon(pts, col, egui::Stroke::NONE));
    (r, open)
}

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let frame = egui::Frame::NONE.fill(t.chrome).stroke(egui::Stroke::new(1.0, t.divider));
    let width = app.ui.left_width;
    let resized = super::resizable_side(ui, true, "left_panel", frame, width, crate::state::LEFT_WIDTH, 0.0, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let (hr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
        ui.painter().text(pos2(hr.left() + 18.0, hr.center().y), Align2::LEFT_CENTER, crate::i18n::tr("My Photos"), t.semibold(15.0), t.text);
        let counts = app.caches.counts(&app.session.catalog);
        let (total, picks, deleted) = (counts.total, counts.picks, counts.deleted);
        egui::ScrollArea::both().id_salt("left-scroll").auto_shrink([false, false]).show_viewport(ui, |ui, viewport| {
            // rows are as wide as the widest one needs (last frame), at least the panel
            let wide = viewport.width().max(content_width(ui.ctx()));
            ui.set_min_width(wide);
            ui.set_max_width(wide);
            ui.data_mut(|d| {
                d.insert_temp(egui::Id::new("left-content-width-next"), 0.0f32);
                // where the visible part of the content ends (screen x), for what stays at the edge
                d.insert_temp(egui::Id::new("left-visible-right"), ui.cursor().left() + viewport.max.x);
            });
            let src = app.session.source;
            for (id, icon, label, count, s) in [
                ("all", Icon::Photos, "All Photos", Some(total), LibrarySource::All),
                ("recentlyAdded", Icon::Clock, "Recently Added", None, LibrarySource::RecentlyAdded),
                ("picks", Icon::FlagPick, "Picks", Some(picks), LibrarySource::Picks),
            ] {
                if row(app, ui, id, icon, label, count, src == s, 0.0).clicked() {
                    let _ = app.run("library.source", json!({"kind": id}));
                }
            }
            // photos whose files can't be found (checked every few seconds, not every frame)
            let missing = missing_count(app, ui);
            if (missing > 0 || src == LibrarySource::Missing)
                && row(app, ui, "missing", Icon::Folder, "Missing Photos", Some(missing), src == LibrarySource::Missing, 0.0).clicked()
            {
                let _ = app.run("library.source", json!({"kind": "missing"}));
            }
            ui.add_space(10.0);
            // Albums header
            let (ar, albums_open) = sidebar_section_header(app, ui, "albums", "Albums");
            // the + stays at the visible edge when the sidebar is scrolled sideways
            let plus_right = (ar.left() + viewport.max.x).min(ar.right());
            let mut hdr = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(Rect::from_min_max(pos2(plus_right - 50.0, ar.top()), pos2(plus_right, ar.bottom())))
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
            );
            let plus = icon_button(&mut hdr, "albumNew", Icon::Plus, vec2(26.0, 26.0), false, true, "Create Album");
            egui::Popup::menu(&plus).show(|ui| {
                if ui.button(crate::i18n::tr("Create Album…")).clicked() {
                    app.ui.dialog = Some(crate::state::Dialog::NewAlbum { name: String::new(), folder: false });
                }
                if ui.button(crate::i18n::tr("Create Smart Album…")).clicked() {
                    app.ui.dialog = Some(crate::state::Dialog::SmartRules {
                        id: None,
                        name: String::new(),
                        rules: lightcraft_catalog::RuleSet { rules: vec![crate::panels::rules_editor::new_rule()], ..Default::default() },
                    });
                }
                if ui.button(crate::i18n::tr("Create Smart Album from Filter…")).clicked() {
                    app.ui.dialog = Some(crate::state::Dialog::NewSmartAlbum { name: String::new() });
                }
                if ui.button(crate::i18n::tr("Create Folder…")).clicked() {
                    app.ui.dialog = Some(crate::state::Dialog::NewAlbum { name: String::new(), folder: true });
                }
            });
            if albums_open {
                let albums: Vec<Album> = app.session.catalog.albums().cloned().collect();
                albums_tree(app, ui, &albums, None, 0.0);
            }
            ui.add_space(10.0);
            local_section(app, ui);
            // By date
            let (_, dates_open) = sidebar_section_header(app, ui, "byDate", "By Date");
            let groups = if dates_open { app.caches.date_groups(&app.session.catalog) } else { Default::default() };
            for g in groups.iter() {
                // year → month → day; a click filters by that prefix, the triangle opens a level
                if date_row(app, ui, &g.year, &crate::i18n::date_group_label(&g.year, true), g.count, 0.0) {
                    for (m, n) in &g.months {
                        let label = crate::i18n::date_group_label(m, true);
                        if date_row(app, ui, m, &label, *n, 16.0) {
                            for (d, n) in g.days.iter().filter(|(d, _)| d.starts_with(m.as_str())) {
                                let label = crate::i18n::date_group_label(d, true);
                                date_row(app, ui, d, &label, *n, 32.0);
                            }
                        }
                    }
                }
            }
            folders_section(app, ui);
            keywords_section(app, ui);
            ui.add_space(10.0);
            if row(app, ui, "recentlyDeleted", Icon::Trash, "Recently Deleted", Some(deleted), src == LibrarySource::RecentlyDeleted, 0.0).clicked() {
                let _ = app.run("library.source", json!({"kind": "recentlyDeleted"}));
            }
            // what the rows asked for becomes next frame's width
            let next = ui.data(|d| d.get_temp::<f32>(egui::Id::new("left-content-width-next"))).unwrap_or(0.0);
            if (next - content_width(ui.ctx())).abs() > 0.5 {
                ui.data_mut(|d| d.insert_temp(egui::Id::new("left-content-width"), next));
                ui.ctx().request_repaint();
            }
        });
    });
    if let Some(w) = resized {
        app.ui.left_width = w;
    }
}

/// How many library photos have no file (Local browse records are not checked; see
/// `cmd::missing::checked_path`). Checking stats every file, which on a network share takes
/// seconds, so it runs on a worker thread: the count shown is the last finished one, refreshed
/// at most every 5 s, and at once (after the running check) when the catalog changed.
fn missing_count(app: &mut LightcraftApp, ui: &mut egui::Ui) -> usize {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
    #[derive(Clone, Default)]
    struct Job {
        n: std::sync::Arc<AtomicUsize>,
        running: std::sync::Arc<AtomicBool>,
        at: f64,
        rev: u64,
    }
    let id = egui::Id::new("missing-count");
    let now = ui.input(|i| i.time);
    let rev = app.session.catalog.revision;
    let mut job: Job = ui.data(|d| d.get_temp(id)).unwrap_or(Job { rev: u64::MAX, at: f64::MIN, ..Default::default() });
    let fresh = now - job.at < 5.0 && job.rev == rev;
    if !fresh && !job.running.load(Relaxed) {
        // the same scope as the Missing Photos view: library photos only, never Local browse records
        let paths = lightcraft_engine::cmd::missing::candidates(&app.session.catalog);
        job.running.store(true, Relaxed);
        job.at = now;
        job.rev = rev;
        let (n, running, ctx) = (job.n.clone(), job.running.clone(), ui.ctx().clone());
        let work = move || {
            let missing = if cfg!(target_arch = "wasm32") { 0 } else { paths.iter().filter(|p| !std::path::Path::new(p).exists()).count() };
            n.store(missing, Relaxed);
            running.store(false, Relaxed);
            ctx.request_repaint();
        };
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::spawn(work);
        #[cfg(target_arch = "wasm32")]
        work();
        ui.data_mut(|d| d.insert_temp(id, job.clone()));
    }
    job.n.load(Relaxed)
}

/// Folders on this computer to browse without adding (Lightroom's Local): Pictures, Desktop,
/// Downloads, the home folder, the folder being browsed, and Browse Folder….
fn local_section(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    if cfg!(target_arch = "wasm32") {
        return;
    }
    let (_, open) = sidebar_section_header(app, ui, "local", "Local");
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_default();
    let mut builtin: Vec<(String, String)> = Vec::new();
    if !home.is_empty() {
        for (name, sub) in [("Pictures", "Pictures"), ("Desktop", "Desktop"), ("Downloads", "Downloads"), ("Home", "")] {
            // joined with the platform's separator, like the paths browsing gives back
            let p = if sub.is_empty() { home.clone() } else { std::path::Path::new(&home).join(sub).to_string_lossy().to_string() };
            // (checked off the UI thread: a home folder can be on a network share)
            if fs_cached(ui, "is-dir", &p, 5.0, |p| std::path::Path::new(p).is_dir()) == Some(true) {
                builtin.push((crate::i18n::tr(name).to_string(), p));
            }
        }
    }
    let browsing = app.session.browse.clone().filter(|_| app.session.source == LibrarySource::Folder);
    let current = browsing.as_ref().map(|b| b.path.clone());
    let local = local_places(builtin, &app.ui.local_roots, current.as_deref(), app.ui.local_browse_root.as_deref(), &app.ui.hidden_locations);
    if current.is_some() {
        app.ui.local_browse_root = local.browse_root.clone();
    }
    if !open {
        ui.add_space(10.0);
        return;
    }
    for (i, (name, path)) in local.places.iter().enumerate() {
        let transient = local.browse_root.as_deref() == Some(path.as_str());
        let reveal = local.owner == Some(i);
        folder_tree(app, ui, name, path, 0.0, current.as_deref(), reveal, transient);
    }
    if app.services.pick_folder.is_some() && row(app, ui, "local:browse", Icon::Plus, crate::i18n::tr("Browse Folder…"), None, false, 0.0).clicked()
    {
        let picked = app.services.pick_folder.as_mut().and_then(|f| f());
        if let Some(path) = picked {
            match app.run("library.browse", json!({"path": path})) {
                // the picked folder stays in Local (and comes back if it was hidden)
                Ok(r) => {
                    let dir = r["path"].as_str().unwrap_or(&path).to_string();
                    let _ = app.run("local.addRoot", json!({"path": dir}));
                }
                Err(e) => app.toast(ui.ctx(), e),
            }
        }
    }
    let hidden = app.ui.hidden_locations.len();
    if hidden > 0 {
        let label = crate::i18n::tr_format!("Show {hidden} hidden location{}", if hidden == 1 { "" } else { "s" }, hidden = hidden);
        if row(app, ui, "local:restoreHidden", Icon::Folder, &label, None, false, 0.0)
            .on_hover_text(crate::i18n::tr("Put the locations you removed from Local back (no files change)"))
            .clicked()
        {
            let _ = app.run("local.restoreHidden", json!({}));
        }
    }
    ui.add_space(10.0);
}

/// Whether two paths name the same folder, however they are spelled (separators, trailing
/// slash, `.`/`..`, drive-letter case; see `lightcraft_catalog::query::folder_key`).
pub(crate) fn same_folder(a: &str, b: &str) -> bool {
    a == b || lightcraft_catalog::query::folder_key(a) == lightcraft_catalog::query::folder_key(b)
}

/// Local's top-level folders and how the folder being browsed sits among them.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct LocalPlaces {
    /// (label, path) of each top-level folder, in order.
    pub places: Vec<(String, String)>,
    /// The top-level folder the browsed folder lies in (the innermost one): its tree opens on
    /// the way down to it. None when that way passes through a hidden folder.
    pub owner: Option<usize>,
    /// A folder listed only for this session because the browsed folder is in no saved
    /// location (browsed from a breadcrumb, the CLI…); it stays while browsing below it.
    pub browse_root: Option<String>,
}

fn folder_label(path: &str) -> String {
    std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string())
}

/// Local's top-level folders, in order: the built-in places, the folders kept with Browse
/// Folder… / Keep in Local (`saved`), then — only when the browsed folder lies in none of those —
/// a session root for it: the previous one (`browse_root`) while browsing stays below it, else
/// the browsed folder itself. A folder inside a listed one is shown inside that one's tree (the
/// root stays; its siblings stay reachable), never as a root of its own. Each folder is listed
/// once however its path is spelled (the first spelling wins), and hidden ones are left out —
/// they stay reachable through breadcrumbs and Browse Folder….
pub(crate) fn local_places(
    builtin: Vec<(String, String)>,
    saved: &[String],
    browsing: Option<&str>,
    browse_root: Option<&str>,
    hidden: &[String],
) -> LocalPlaces {
    use lightcraft_catalog::query::{folder_key, folder_within};
    let is_hidden = |p: &str| hidden.iter().any(|h| same_folder(h, p));
    let mut places = builtin;
    for path in saved {
        if !places.iter().any(|(_, p)| same_folder(p, path)) {
            places.push((folder_label(path), path.clone()));
        }
    }
    places.retain(|(_, p)| !is_hidden(p));
    let mut out = LocalPlaces::default();
    if let Some(c) = browsing
        && !places.iter().any(|(_, p)| folder_within(c, p))
    {
        let root = browse_root.filter(|r| folder_within(c, r)).unwrap_or(c);
        if !is_hidden(root) {
            places.push((folder_label(root), root.to_string()));
            out.browse_root = Some(root.to_string());
        }
    }
    if let Some(c) = browsing {
        // A tree never opens on the way down through a hidden folder: hiding a kept folder
        // beneath Home would otherwise reveal it again inside Home's (possibly huge) tree.
        let through_hidden = |p: &str| hidden.iter().any(|h| folder_within(c, h) && folder_within(h, p));
        out.owner = places
            .iter()
            .enumerate()
            .filter(|(_, (_, p))| folder_within(c, p) && !through_hidden(p))
            .max_by_key(|(_, (_, p))| folder_key(p).len())
            .map(|(i, _)| i);
    }
    out.places = places;
    out
}

/// The subfolders of `path` (not hidden ones), sorted; listed on a worker thread at most every
/// 2 s per folder (empty until the first listing).
fn subfolders(ui: &egui::Ui, path: &str) -> Vec<(String, String)> {
    fs_cached(ui, "subfolders", path, 2.0, list_subfolders).unwrap_or_default()
}

fn list_subfolders(path: &str) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = std::fs::read_dir(path)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    (!name.starts_with('.')).then(|| (name, e.path().to_string_lossy().to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by_key(|(n, _)| n.to_lowercase());
    v
}

/// How many [`fs_cached`] answers for `ctx` are being worked out right now. Rows appear (and
/// the sidebar below them moves) when they land, so the headless driver counts them as pending
/// work and waits for them before acting on widget positions.
pub(crate) fn fs_cached_running(ctx: &egui::Context) -> usize {
    fs_running_counter(ctx).load(std::sync::atomic::Ordering::Acquire)
}

fn fs_running_counter(ctx: &egui::Context) -> std::sync::Arc<std::sync::atomic::AtomicUsize> {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<std::sync::Arc<std::sync::atomic::AtomicUsize>>(egui::Id::new("fs-cached-running")).clone())
}

/// A file-system answer for `path` (`f(path)`), kept per `kind` and path and refreshed on a worker
/// thread at most every `every` seconds: a folder on a sleeping NAS, a dropped share or a
/// spinning-up drive never blocks a frame. `None` until the first answer arrives.
pub(crate) fn fs_cached<T: Clone + Send + 'static>(ui: &egui::Ui, kind: &'static str, path: &str, every: f64, f: fn(&str) -> T) -> Option<T> {
    struct Entry<T> {
        value: Option<T>,
        at: Option<f64>,
        running: bool,
    }
    type Cell<T> = std::sync::Arc<std::sync::Mutex<Entry<T>>>;
    let id = egui::Id::new(("fs-cached", kind, path.to_string()));
    let now = ui.input(|i| i.time);
    let cell: Cell<T> = match ui.data(|d| d.get_temp::<Cell<T>>(id)) {
        Some(c) => c,
        None => {
            let c: Cell<T> = std::sync::Arc::new(std::sync::Mutex::new(Entry { value: None, at: None, running: false }));
            ui.data_mut(|d| d.insert_temp(id, c.clone()));
            c
        }
    };
    let lock = |c: &Cell<T>| c.lock().unwrap_or_else(std::sync::PoisonError::into_inner).value.clone();
    let start = {
        let mut e = cell.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let due = !e.running && e.at.is_none_or(|at| now - at >= every || now < at);
        if due {
            e.running = true;
            e.at = Some(now);
        }
        due
    };
    if start {
        use std::sync::atomic::Ordering;
        let running = fs_running_counter(ui.ctx());
        running.fetch_add(1, Ordering::AcqRel);
        let (out, path, repaint, done) = (cell.clone(), path.to_string(), ui.ctx().clone(), running.clone());
        let work = move || {
            let v = f(&path);
            let mut e = out.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            e.value = Some(v);
            e.running = false;
            drop(e);
            // after the answer is stored: a frame that sees the count drop also sees the answer
            done.fetch_sub(1, Ordering::AcqRel);
            repaint.request_repaint();
        };
        #[cfg(not(target_arch = "wasm32"))]
        if std::thread::Builder::new().name("lc-fs-list".into()).spawn(work).is_err() {
            cell.lock().unwrap_or_else(std::sync::PoisonError::into_inner).running = false;
            running.fetch_sub(1, Ordering::AcqRel);
        }
        #[cfg(target_arch = "wasm32")]
        work();
    }
    lock(&cell)
}

/// A folder on disk with a disclosure triangle: click browses it, the triangle lists its
/// subfolders. In the tree that holds the folder being browsed (`reveal`), the folders on the
/// way down to it open whenever the browsed folder changes, so it shows highlighted in place.
/// `transient`: a top-level row listed for this session only (it offers Keep in Local).
#[allow(clippy::too_many_arguments)]
fn folder_tree(
    app: &mut LightcraftApp,
    ui: &mut egui::Ui,
    name: &str,
    path: &str,
    indent: f32,
    current: Option<&str>,
    reveal: bool,
    transient: bool,
) {
    let t = Tokens::get(ui.ctx());
    let open_id = egui::Id::new(("folder-open", path.to_string()));
    let sel = current.is_some_and(|c| same_folder(c, path));
    let on_the_way = !sel && current.is_some_and(|c| lightcraft_catalog::query::folder_within(c, path));
    let mut open: bool = ui.data(|d| d.get_temp(open_id)).unwrap_or(false);
    if reveal && on_the_way {
        // opened once per browsed folder: collapsing it again afterwards sticks
        let revealed_id = egui::Id::new(("folder-revealed", path.to_string()));
        let target = current.map(str::to_string);
        if ui.data(|d| d.get_temp::<Option<String>>(revealed_id)) != Some(target.clone()) {
            open = true;
            ui.data_mut(|d| {
                d.insert_temp(open_id, true);
                d.insert_temp(revealed_id, target);
            });
        }
    }
    let resp = row(app, ui, &format!("local:{path}"), Icon::Folder, name, None, sel, indent + 12.0).on_hover_text(path);
    let c = pos2(resp.rect.left() + 10.0 + indent, resp.rect.center().y);
    let tri = Rect::from_center_size(c, vec2(14.0, 14.0));
    let tr = ui.interact(tri, egui::Id::new(("folder-tri", path.to_string())), Sense::click());
    register(ui.ctx(), format!("folderToggle:{path}"), tri);
    let col = if tr.hovered() { t.text } else { t.text_dim };
    let pts = if open {
        vec![c + vec2(-4.0, -2.0), c + vec2(4.0, -2.0), c + vec2(0.0, 3.0)]
    } else {
        vec![c + vec2(-2.0, -4.0), c + vec2(3.0, 0.0), c + vec2(-2.0, 4.0)]
    };
    ui.painter().add(egui::Shape::convex_polygon(pts, col, egui::Stroke::NONE));
    if tr.clicked() {
        open = !open;
        ui.data_mut(|d| d.insert_temp(open_id, open));
    } else if resp.clicked()
        && let Err(e) = app.run("library.browse", json!({"path": path}))
    {
        app.toast(ui.ctx(), e);
    }
    resp.context_menu(|ui| {
        if (transient || indent > 0.0)
            && ui
                .button(if transient { "Keep in Local" } else { "Add to Local" })
                .on_hover_text(crate::i18n::tr("List this folder in Local from now on"))
                .clicked()
        {
            let _ = app.run("local.addRoot", json!({"path": path}));
            ui.close();
        }
        if indent == 0.0
            && ui
                .button(crate::i18n::tr("Remove from Local"))
                .on_hover_text(crate::i18n::tr("Hides this shortcut only; the folder and its photos stay as they are"))
                .clicked()
        {
            let _ = app.run("local.hide", json!({"path": path}));
            ui.close();
        }
        if ui.button(crate::i18n::tr("Rename Folder…")).clicked() {
            app.ui.dialog = Some(crate::state::Dialog::TextPrompt {
                title: crate::i18n::tr_format!("Rename “{name}”", name = name),
                hint: "Folder name (renamed on disk; its photos follow)".into(),
                value: name.to_string(),
                command: "folder.rename".into(),
                params: json!({"path": path}),
                key: "name".into(),
            });
        }
        if app.services.pick_folder.is_some() && ui.button(crate::i18n::tr("Move Folder To…")).clicked() {
            let into = app.services.pick_folder.as_mut().and_then(|f| f());
            if let Some(into) = into {
                match app.run("folder.move", json!({"path": path, "into": into})) {
                    Ok(r) => app.toast(ui.ctx(), crate::i18n::tr_format!("Moved; {} photo(s) relinked", r["relinked"])),
                    Err(e) => app.toast(ui.ctx(), e),
                }
            }
        }
        if app.services.reveal.is_some()
            && ui.button(crate::i18n::tr(crate::menus::reveal_label())).clicked()
            && let Some(f) = app.services.reveal.as_mut()
        {
            let _ = f(path);
        }
    });
    if open && indent < 12.0 * 8.0 {
        for (n, p) in subfolders(ui, path) {
            folder_tree(app, ui, &n, &p, indent + 12.0, current, reveal, false);
        }
    }
}

/// Date/keyword counts describe the whole library, so clicking their rows must leave an
/// album, Local folder or other source. Keep the user's other filters (shown as grid chips).
fn library_filter(app: &mut LightcraftApp, ctx: &egui::Context, patch: serde_json::Value) -> Result<(), String> {
    let (_, before) = app.session.visible_shared();
    app.run("library.source", json!({"kind": "all", "filter": patch, "keepSelection": true}))?;
    let (_, after) = app.session.visible_shared();
    // A kept active ID can move in the new results. Equal results leave deliberate user
    // scrolling alone; an active-ID change is already handled by the grid and filmstrip.
    if before != after {
        super::grid::follow_navigation(ctx);
    }
    Ok(())
}

/// One By Date row (`key`: `YYYY`, `YYYY-MM` or `YYYY-MM-DD`); returns whether it is open.
fn date_row(app: &mut LightcraftApp, ui: &mut egui::Ui, key: &str, label: &str, count: usize, indent: f32) -> bool {
    let t = Tokens::get(ui.ctx());
    let open_id = egui::Id::new(("date-open", key.to_string()));
    let mut open: bool = ui.data(|d| d.get_temp(open_id)).unwrap_or(false);
    let sel = app.session.source == LibrarySource::All && app.session.filter.date.as_deref() == Some(key);
    let resp = row(app, ui, &format!("date:{key}"), Icon::Clock, label, Some(count), sel, indent);
    if key.len() < 10 {
        let c = pos2(resp.rect.left() + 10.0 + indent, resp.rect.center().y);
        let tri = Rect::from_center_size(c, vec2(14.0, 14.0));
        let tr = ui.interact(tri, egui::Id::new(("date-tri", key.to_string())), Sense::click());
        register(ui.ctx(), format!("dateToggle:{key}"), tri);
        let col = if tr.hovered() { t.text } else { t.text_dim };
        let pts = if open {
            vec![c + vec2(-4.0, -2.0), c + vec2(4.0, -2.0), c + vec2(0.0, 3.0)]
        } else {
            vec![c + vec2(-2.0, -4.0), c + vec2(3.0, 0.0), c + vec2(-2.0, 4.0)]
        };
        ui.painter().add(egui::Shape::convex_polygon(pts, col, egui::Stroke::NONE));
        if tr.clicked() {
            open = !open;
            ui.data_mut(|d| d.insert_temp(open_id, open));
        }
    }
    if resp.clicked() {
        let v = if sel { serde_json::Value::Null } else { json!(key) };
        if let Err(e) = library_filter(app, ui.ctx(), json!({"date": v})) {
            app.toast(ui.ctx(), e);
        }
    }
    open
}

fn albums_tree(app: &mut LightcraftApp, ui: &mut egui::Ui, all: &[Album], parent: Option<AlbumId>, indent: f32) {
    let mut kids: Vec<&Album> = all.iter().filter(|a| a.parent == parent).collect();
    kids.sort_by_key(|a| (!a.folder, a.name.to_lowercase()));
    for a in kids {
        if a.folder {
            let open_id = egui::Id::new(("folder-open", a.id.0));
            let open: bool = ui.data(|d| d.get_temp(open_id)).unwrap_or(true);
            let resp = row(app, ui, &format!("folder:{}", a.id.0), Icon::Folder, &a.name, None, false, indent);
            if resp.clicked() {
                ui.data_mut(|d| d.insert_temp(open_id, !open));
            }
            folder_menu(app, &resp, a);
            if open {
                albums_tree(app, ui, all, Some(a.id), indent + 16.0);
            }
        } else {
            let sel = app.session.source == LibrarySource::Album(a.id);
            let icon = if a.is_smart() { Icon::SmartAlbum } else { Icon::Album };
            // cached: a smart album's count scans the catalog
            let now = (app.session.clock)();
            let n = app.caches.album_counts(&app.session.catalog, &now).get(&a.id).copied().unwrap_or(0);
            // the album B adds to is marked "+"
            let target =
                app.session.target_album.filter(|t| app.session.catalog.album(*t).is_some()).or_else(|| app.session.catalog.quick_collection());
            let label = if target == Some(a.id) { format!("{} +", a.name) } else { a.name.clone() };
            let mut resp = row(app, ui, &format!("album:{}", a.id.0), icon, &label, Some(n), sel, indent);
            if !a.is_smart() {
                drop_target(app, ui, &resp, a);
            }
            if let Some(rules) = &a.smart {
                resp = resp.on_hover_text(crate::i18n::tr_format!("Smart album: {}", crate::i18n::filter_label(rules, &app.session.catalog)));
            }
            if resp.clicked() {
                let _ = app.run("library.source", json!({"kind": "album", "id": a.id.0}));
            }
            folder_menu(app, &resp, a);
        }
    }
}

/// An album row while photos are dragged from the grid: highlighted under the pointer; a
/// release there adds them.
fn drop_target(app: &mut LightcraftApp, ui: &mut egui::Ui, resp: &egui::Response, a: &Album) {
    let Some(ids) = app.ui.dragging_photos.clone() else { return };
    let over = ui.input(|i| i.pointer.latest_pos()).is_some_and(|p| resp.rect.contains(p));
    if !over {
        return;
    }
    let t = Tokens::get(ui.ctx());
    ui.painter().rect_stroke(resp.rect.shrink2(vec2(8.0, 1.0)), 4.0, egui::Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
    if ui.input(|i| i.pointer.any_released()) {
        let n = ids.len();
        match app.run("album.addPhotos", json!({"id": a.id.0, "ids": ids})) {
            Ok(_) => app.toast(ui.ctx(), crate::i18n::tr_format!("Added {n} photo{} to “{}”", if n == 1 { "" } else { "s" }, a.name, n = n)),
            Err(e) => app.toast(ui.ctx(), e),
        }
        app.ui.dragging_photos = None;
    }
}

fn folder_menu(app: &mut LightcraftApp, resp: &egui::Response, a: &Album) {
    resp.context_menu(|ui| {
        if !a.folder && !a.is_smart() && ui.button(crate::i18n::tr("Add Selected Photos")).clicked() {
            let _ = app.run("album.addPhotos", json!({"id": a.id.0}));
        }
        if !a.folder && !a.is_smart() {
            let is_target = app.session.target_album == Some(a.id) || (app.session.target_album.is_none() && a.quick);
            if !is_target && ui.button(crate::i18n::tr("Set as Target Album (B adds to it)")).clicked() {
                let _ = app.run("album.setTarget", json!({"id": if a.quick { serde_json::Value::Null } else { json!(a.id.0) }}));
            }
            if is_target && !a.quick && ui.button(crate::i18n::tr("Stop Using as Target Album")).clicked() {
                let _ = app.run("album.setTarget", json!({"id": null}));
            }
        }
        if a.quick && ui.button(crate::i18n::tr("Clear Quick Collection")).clicked() {
            let _ = app.run("album.clearQuick", json!({}));
        }
        if a.is_smart() && ui.button(crate::i18n::tr("Edit Smart Album…")).clicked() {
            // older smart albums keep their filter fields; the editor works on the rule set
            let rules = a.smart.as_ref().and_then(|f| f.rule_set.clone()).unwrap_or_default();
            app.ui.dialog = Some(crate::state::Dialog::SmartRules { id: Some(a.id.0), name: a.name.clone(), rules });
        }
        if a.is_smart() && ui.button(crate::i18n::tr("Update Rules from Current Filter")).clicked() {
            let _ = app.run("album.setRules", json!({"id": a.id.0, "fromView": true}));
        }
        if !a.folder {
            // export: show the album, select its photos, then the dialog / a preset
            let show_all = |app: &mut LightcraftApp| {
                let _ = app.run("library.source", json!({"kind": "album", "id": a.id.0}));
                let _ = app.run("library.selectAll", json!({}));
            };
            let has_photos = app.session.catalog.album_count(a.id) > 0;
            if ui.add_enabled(has_photos, egui::Button::new(crate::i18n::tr("Export Album…"))).clicked() {
                show_all(app);
                let _ = app.run("dialog.export", json!({}));
            }
            ui.add_enabled_ui(has_photos, |ui| {
                ui.menu_button(crate::i18n::tr("Export Album with Preset"), |ui| {
                    for (p, _) in app.session.all_export_presets() {
                        if ui.button(&p.name).clicked() {
                            show_all(app);
                            if let Err(e) = app.run("app.export", json!({"preset": p.name, "background": true})) {
                                app.toast(ui.ctx(), e);
                            }
                        }
                    }
                });
            });
            ui.separator();
        }
        // move into another folder (not into itself or one of its own subfolders)
        let mut folders: Vec<(u64, String)> =
            app.session.catalog.albums().filter(|f| f.folder && !is_within(app, f.id, a.id)).map(|f| (f.id.0, f.name.clone())).collect();
        folders.sort_by_key(|(_, n)| n.to_lowercase());
        ui.menu_button(crate::i18n::tr("Move to"), |ui| {
            if ui.add_enabled(a.parent.is_some(), egui::Button::new(crate::i18n::tr("Top Level"))).clicked() {
                let _ = app.run("album.move", json!({"id": a.id.0, "parent": null}));
            }
            for (fid, name) in &folders {
                if ui.add_enabled(a.parent.map(|p| p.0) != Some(*fid), egui::Button::new(name)).clicked() {
                    let _ = app.run("album.move", json!({"id": a.id.0, "parent": fid}));
                }
            }
        });
        if ui.button(crate::i18n::tr("Rename…")).clicked() {
            app.ui.dialog = Some(crate::state::Dialog::RenameAlbum { id: a.id.0, name: a.name.clone() });
        }
        if ui.button(crate::i18n::tr("Delete")).clicked() {
            let _ = app.run("album.delete", json!({"id": a.id.0}));
        }
    });
}

/// Whether `id` is `ancestor` or lies inside it.
fn is_within(app: &LightcraftApp, id: lightcraft_catalog::AlbumId, ancestor: lightcraft_catalog::AlbumId) -> bool {
    let mut cur = Some(id);
    let mut guard = 0;
    while let Some(c) = cur {
        if c == ancestor {
            return true;
        }
        cur = app.session.catalog.album(c).and_then(|x| x.parent);
        guard += 1;
        if guard > 64 {
            break;
        }
    }
    false
}

/// "Folders": where on disk the library's photos were imported from, with photo counts (see
/// `lightcraft_catalog::folders`). A click makes that folder the source, like an album or a
/// Local folder: its photos and those of the folders inside it fill the grid. The triangle opens
/// a level. Only folders holding imported photos are
/// listed; every folder on disk is under Local.
fn folders_section(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let tree = app.caches.folder_tree(&app.session.catalog);
    if tree.is_empty() {
        return;
    }
    ui.add_space(10.0);
    if sidebar_section_header(app, ui, "folders", "Folders").1 {
        reveal_chosen(app, ui, &tree);
        folder_rows(app, ui, &tree, 0.0);
    }
}

/// Whenever the shown folder changes (a click, an agent, a rename or its undo), open the rows
/// above it so it is on screen; folding one by hand afterwards sticks until the choice changes.
fn reveal_chosen(app: &LightcraftApp, ui: &egui::Ui, tree: &[FolderNode]) {
    let shown = app.session.library_folder.clone().filter(|_| app.session.source == LibrarySource::LibraryFolder);
    let chosen = shown.filter(|c| !lightcraft_catalog::query::folder_key(c).is_empty());
    let seen = egui::Id::new("libfolder-revealed");
    let now = chosen.as_deref().map(lightcraft_catalog::query::folder_key);
    if ui.data(|d| d.get_temp::<Option<String>>(seen)) == Some(now.clone()) {
        return;
    }
    ui.data_mut(|d| d.insert_temp(seen, now));
    let Some(chosen) = chosen else { return };
    fn open_above(ui: &egui::Ui, nodes: &[FolderNode], chosen: &str) {
        for n in nodes {
            if lightcraft_catalog::query::folder_within(chosen, &n.path) && !same_folder(chosen, &n.path) {
                let key = lightcraft_catalog::query::folder_key(&n.path);
                ui.data_mut(|d| d.insert_temp(egui::Id::new(("libfolder-open", key)), true));
                open_above(ui, &n.children, chosen);
            }
        }
    }
    open_above(ui, tree, &chosen);
}

fn folder_rows(app: &mut LightcraftApp, ui: &mut egui::Ui, nodes: &[FolderNode], indent: f32) {
    let t = Tokens::get(ui.ctx());
    for n in nodes {
        let key = lightcraft_catalog::query::folder_key(&n.path);
        let open_id = egui::Id::new(("libfolder-open", key.clone()));
        // a disk starts open, and so does a folder that holds nothing itself and leads to one
        // folder (`Users` → `me`): the first row where the library branches is what you look for
        let leads_on = n.own == 0 && n.children.len() == 1;
        let mut open: bool = ui.data(|d| d.get_temp(open_id)).unwrap_or(n.volume || leads_on);
        // a row whose path would cover other disks' photos too only opens and closes
        let selectable = n.selectable;
        let sel = selectable
            && app.session.source == LibrarySource::LibraryFolder
            && app.session.library_folder.as_deref().is_some_and(|f| same_folder(f, &n.path));
        let name = if selectable { n.name.clone() } else { crate::i18n::tr("This Computer").to_string() };
        let resp = row_named(app, ui, &format!("libfolder:{}", n.path), Icon::Folder, &name, Some(&name), Some(n.count), sel, indent);
        let mut toggled = false;
        if !n.children.is_empty() {
            // disclosure triangle left of the icon
            let c = pos2(resp.rect.left() + 10.0 + indent, resp.rect.center().y);
            let tri = Rect::from_center_size(c, vec2(14.0, 14.0));
            let tr = ui.interact(tri, egui::Id::new(("libfolder-tri", key)), Sense::click());
            register(ui.ctx(), format!("libraryFolderToggle:{}", n.path), tri);
            let col = if tr.hovered() { t.text } else { t.text_dim };
            let pts = if open {
                vec![c + vec2(-4.0, -2.0), c + vec2(4.0, -2.0), c + vec2(0.0, 3.0)]
            } else {
                vec![c + vec2(-2.0, -4.0), c + vec2(3.0, 0.0), c + vec2(-2.0, 4.0)]
            };
            ui.painter().add(egui::Shape::convex_polygon(pts, col, egui::Stroke::NONE));
            toggled = tr.clicked();
            tr.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, if open { crate::i18n::tr("Collapse") } else { crate::i18n::tr("Expand") })
            });
            // the triangle sits on the row and takes its clicks: the menu opens from it too
            row_menu(app, &tr, n);
        }
        let tip = if n.children.is_empty() { n.path.clone() } else { format!("{} ({})", n.path, crate::i18n::tr("includes the folders inside it")) };
        let resp = resp.on_hover_text(tip);
        if resp.clicked() && !toggled {
            if selectable {
                // a source like an album or a Local folder: it replaces what the grid showed
                let _ = app.run("library.source", json!({"kind": "libraryFolder", "path": n.path}));
            } else {
                toggled = true;
            }
        }
        if toggled {
            open = !open;
            ui.data_mut(|d| d.insert_temp(open_id, open));
        }
        row_menu(app, &resp, n);
        if open && !n.children.is_empty() {
            folder_rows(app, ui, &n.children, indent + 16.0);
        }
    }
}

/// The context menu of a Folders row: a folder's own, a disk's (remove it), none for the
/// startup disk.
fn row_menu(app: &mut LightcraftApp, resp: &egui::Response, n: &FolderNode) {
    if !n.volume {
        folder_menu_for_library(app, resp, n);
    } else if n.path != "/" {
        resp.context_menu(|ui| {
            if ui
                .button(crate::i18n::tr("Remove Disk from Library…"))
                .on_hover_text(crate::i18n::tr("Moves every photo imported from this disk to Recently Deleted; no file is touched"))
                .clicked()
            {
                app.ui.dialog = Some(crate::state::Dialog::RemoveFolder { path: n.path.clone(), name: n.name.clone(), count: n.count, disk: true });
                ui.close();
            }
        });
    }
}

/// The context menu of a folder row: the folder's disk actions (the same as Local's, photos
/// follow) and taking its photos out of the library.
fn folder_menu_for_library(app: &mut LightcraftApp, resp: &egui::Response, n: &FolderNode) {
    let path = n.path.as_str();
    let name = std::path::Path::new(path).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string());
    // what dialogs call it: the last two names, so same-named folders are told apart
    let label = lightcraft_catalog::folders::folder_label(path);
    resp.context_menu(|ui| {
        if ui.button(crate::i18n::tr("Rename Folder…")).clicked() {
            app.ui.dialog = Some(crate::state::Dialog::TextPrompt {
                title: crate::i18n::tr_format!("Rename “{name}”", name = label),
                hint: "Folder name (renamed on disk; its photos follow)".into(),
                value: name.clone(),
                command: "folder.rename".into(),
                params: json!({"path": path}),
                key: "name".into(),
            });
            ui.close();
        }
        if app.services.pick_folder.is_some() && ui.button(crate::i18n::tr("Move Folder To…")).clicked() {
            let into = app.services.pick_folder.as_mut().and_then(|f| f());
            if let Some(into) = into {
                match app.run("folder.move", json!({"path": path, "into": into})) {
                    Ok(r) => app.toast(ui.ctx(), format!("Moved; {} photo(s) relinked", r["relinked"])),
                    Err(e) => app.toast(ui.ctx(), e),
                }
            }
            ui.close();
        }
        if app.services.reveal.is_some()
            && ui.button(crate::i18n::tr("Show in Finder")).clicked()
            && let Some(f) = app.services.reveal.as_mut()
        {
            let _ = f(path);
            ui.close();
        }
        ui.separator();
        if ui
            .button(crate::i18n::tr("Remove from Library…"))
            .on_hover_text(crate::i18n::tr("Moves the photos imported from this folder to Recently Deleted; no file is touched"))
            .clicked()
        {
            app.ui.dialog = Some(crate::state::Dialog::RemoveFolder { path: path.to_string(), name: label.clone(), count: n.count, disk: false });
            ui.close();
        }
    });
}

/// "Keywords": the library's keyword tree with photo counts (`a|b|c` keywords nest). A click
/// filters the grid by the keyword (children included), the triangle opens a level, and the
/// context menu renames, merges or deletes the keyword across the library.
fn keywords_section(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let tree = app.caches.keyword_tree(&app.session.catalog);
    if tree.is_empty() {
        return;
    }
    ui.add_space(10.0);
    if sidebar_section_header(app, ui, "keywords", "Keywords").1 {
        keyword_rows(app, ui, &tree, 0.0);
    }
}

fn keyword_rows(app: &mut LightcraftApp, ui: &mut egui::Ui, nodes: &[KeywordNode], indent: f32) {
    let t = Tokens::get(ui.ctx());
    for n in nodes {
        let open_id = egui::Id::new(("kw-open", n.path.to_lowercase()));
        let mut open: bool = ui.data(|d| d.get_temp(open_id)).unwrap_or(false);
        let sel = app.session.source == LibrarySource::All
            && app.session.filter.keyword.as_deref().is_some_and(|k| k.to_lowercase() == n.path.to_lowercase());
        let resp = row(app, ui, &format!("keyword:{}", n.path), Icon::Tag, &n.name, Some(n.count), sel, indent);
        if !n.children.is_empty() {
            // disclosure triangle left of the icon
            let c = pos2(resp.rect.left() + 10.0 + indent, resp.rect.center().y);
            let tri = Rect::from_center_size(c, vec2(14.0, 14.0));
            let tr = ui.interact(tri, egui::Id::new(("kw-tri", n.path.to_lowercase())), Sense::click());
            register(ui.ctx(), format!("keywordToggle:{}", n.path), tri);
            let col = if tr.hovered() { t.text } else { t.text_dim };
            let pts = if open {
                vec![c + vec2(-4.0, -2.0), c + vec2(4.0, -2.0), c + vec2(0.0, 3.0)]
            } else {
                vec![c + vec2(-2.0, -4.0), c + vec2(3.0, 0.0), c + vec2(-2.0, 4.0)]
            };
            ui.painter().add(egui::Shape::convex_polygon(pts, col, egui::Stroke::NONE));
            if tr.clicked() {
                open = !open;
                ui.data_mut(|d| d.insert_temp(open_id, open));
            }
        }
        let resp = resp.on_hover_text(if n.children.is_empty() { n.path.clone() } else { format!("{} (includes the keywords below it)", n.path) });
        if resp.clicked() {
            let v = if sel { serde_json::Value::Null } else { json!(n.path) };
            if let Err(e) = library_filter(app, ui.ctx(), json!({"keyword": v})) {
                app.toast(ui.ctx(), e);
            }
        }
        resp.context_menu(|ui| {
            let has_sel = app.session.active().is_some();
            if ui.add_enabled(has_sel, egui::Button::new(crate::i18n::tr("Add to Selected Photos"))).clicked() {
                let _ = app.run("photo.setMeta", json!({"addKeywords": [n.path]}));
            }
            if ui.add_enabled(has_sel, egui::Button::new(crate::i18n::tr("Remove from Selected Photos"))).clicked() {
                let _ = app.run("photo.setMeta", json!({"removeKeywords": [n.path]}));
            }
            ui.separator();
            if ui.button(crate::i18n::tr("Rename Keyword…")).clicked() {
                app.ui.dialog = Some(crate::state::Dialog::RenameKeyword { from: n.path.clone(), to: n.path.clone() });
            }
            if ui.button(crate::i18n::tr("Merge into…")).clicked() {
                app.ui.dialog = Some(crate::state::Dialog::MergeKeywords { from: vec![n.path.clone()], into: String::new() });
            }
            if ui.button(crate::i18n::tr("Delete Keyword")).clicked() {
                let _ = app.run("keyword.delete", json!({"keyword": n.path}));
            }
        });
        if open && !n.children.is_empty() {
            keyword_rows(app, ui, &n.children, indent + 16.0);
        }
    }
}

#[cfg(test)]
mod tests_keyword_selection;

#[cfg(test)]
mod navigation_tests {
    use lightcraft_catalog::{AlbumId, Filter, Flag, Op, Photo, PhotoId, Source};
    use lightcraft_engine::{Browse, LibrarySource, Session};

    use super::{date_row, keyword_rows};
    use crate::{LightcraftApp, Services};
    use serde_json::json;

    /// The album and Local folder contain only photo 1. All date/keyword matches live elsewhere;
    /// same-metadata Local/deleted photos must stay out of library-wide navigation.
    fn library() -> (LightcraftApp, AlbumId) {
        let mut s = Session::new();
        for (id, date, keyword, rating, local, deleted) in [
            (1, "2025-01-01", "Work", 5, false, false),
            (2, "2026-04-12", "Travel|Coast", 5, false, false),
            (3, "2026-04-20", "Travel|Mountains", 5, false, false),
            (4, "2026-05-01", "Travel|Coast", 2, false, false),
            (5, "2026-04-12", "Travel|Coast", 5, true, false),
            (6, "2026-04-12", "Travel|Coast", 5, false, true),
            (7, "2026-04-12", "Other", 5, false, false),
            (8, "2026-04-12", "Travel|Coast", 1, false, false),
        ] {
            let folder = if id == 1 { "album" } else { "elsewhere" };
            let path = format!("/synthetic-sidebar/{folder}/{id}.jpg");
            let imported = if id == 1 { "2026-10-01T00:00:00" } else { "2020-01-01T00:00:00" };
            let mut p = Photo::new(PhotoId(id), Source::File { path }, &format!("{id}.jpg"), "JPEG", 4, 3, imported);
            p.captured = Some(format!("{date}T12:00:00"));
            p.meta.keywords = vec![keyword.into()];
            p.rating = rating;
            p.local = local;
            p.deleted = deleted;
            p.flag = if id == 1 { Flag::Pick } else { Flag::None };
            s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
        }
        let album = s.execute("album.create", &json!({"name": "Unrelated album"})).unwrap();
        let album = AlbumId(album["id"].as_u64().unwrap());
        s.execute("album.addPhotos", &json!({"id": album.0, "ids": [1]})).unwrap();
        s.browse = Some(Browse { path: "/synthetic-sidebar/album".into(), subfolders: false });
        s.library_folder = Some("/synthetic-sidebar/album".into());
        (LightcraftApp::new(s, Services::default()), album)
    }

    /// Draw the actual filter rows without a photo renderer, filesystem access or worker jobs.
    fn frame(app: &mut LightcraftApp, ctx: &egui::Context, events: Vec<egui::Event>) -> Vec<(String, egui::Rect)> {
        let _ = crate::widgets::take_registry(ctx);
        let raw =
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 600.0))), events, ..Default::default() };
        let _ = ctx.run_ui(raw, |ui| {
            let groups = app.caches.date_groups(&app.session.catalog);
            for g in groups.iter() {
                date_row(app, ui, &g.year, &g.year, g.count, 0.0);
                for (m, count) in &g.months {
                    date_row(app, ui, m, m, *count, 16.0);
                }
                for (d, count) in &g.days {
                    date_row(app, ui, d, d, *count, 32.0);
                }
            }
            let tree = app.caches.keyword_tree(&app.session.catalog);
            keyword_rows(app, ui, &tree, 0.0);
        });
        crate::widgets::take_registry(ctx)
    }

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        ctx
    }

    fn click(app: &mut LightcraftApp, ctx: &egui::Context, id: &str) {
        frame(app, ctx, vec![]);
        let widgets = frame(app, ctx, vec![]);
        let pos = widgets.iter().find(|(w, _)| w == id).unwrap_or_else(|| panic!("missing {id}: {widgets:?}")).1.center();
        frame(app, ctx, vec![egui::Event::PointerMoved(pos)]);
        for pressed in [true, false] {
            frame(app, ctx, vec![egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() }]);
        }
    }

    fn visible(app: &mut LightcraftApp) -> Vec<u64> {
        let mut ids: Vec<u64> = app.session.visible().iter().map(|id| id.0).collect();
        ids.sort_unstable();
        ids
    }

    #[test]
    fn date_rows_navigate_the_whole_library_from_other_sources() {
        let (mut app, album) = library();
        for source in [
            LibrarySource::Album(album),
            LibrarySource::Folder,
            LibrarySource::LibraryFolder,
            LibrarySource::Picks,
            LibrarySource::RecentlyAdded,
            LibrarySource::RecentlyDeleted,
        ] {
            for (date, expected) in [("2026", vec![2, 3, 7]), ("2026-04", vec![2, 3, 7]), ("2026-04-12", vec![2, 7])] {
                app.session.source = source;
                app.session.filter = Filter { rating: 5, ..Default::default() };
                assert!(!visible(&mut app).contains(&2), "the date match is outside {source:?}");
                let ctx = context();
                click(&mut app, &ctx, &format!("source:date:{date}"));
                assert_eq!(app.session.source, LibrarySource::All, "{source:?}, {date}");
                assert_eq!(app.session.filter.date.as_deref(), Some(date));
                assert_eq!(app.session.filter.rating, 5, "independent filters remain active");
                assert_eq!(visible(&mut app), expected, "{source:?}, {date}");
                assert!(app.session.active().is_some_and(|id| expected.contains(&id.0)), "selection follows the final filtered view");
                click(&mut app, &ctx, &format!("source:date:{date}"));
                assert_eq!(app.session.filter.date, None, "clicking the active global date clears it");
                assert_eq!(visible(&mut app), [1, 2, 3, 7]);
            }
        }
    }

    #[test]
    fn keyword_rows_navigate_the_whole_library_including_children() {
        let (mut app, album) = library();
        for source in [
            LibrarySource::Album(album),
            LibrarySource::Folder,
            LibrarySource::LibraryFolder,
            LibrarySource::Picks,
            LibrarySource::RecentlyAdded,
            LibrarySource::RecentlyDeleted,
        ] {
            for (keyword, expected) in [("Travel", vec![2, 3]), ("Travel|Coast", vec![2])] {
                app.session.source = source;
                app.session.filter = Filter { rating: 5, ..Default::default() };
                assert!(!visible(&mut app).contains(&2), "the keyword match is outside {source:?}");
                let ctx = context();
                if keyword.contains('|') {
                    click(&mut app, &ctx, "keywordToggle:Travel");
                    assert_eq!(app.session.source, source, "expanding the tree does not navigate");
                    assert_eq!(app.session.filter.keyword, None);
                }
                click(&mut app, &ctx, &format!("source:keyword:{keyword}"));
                assert_eq!(app.session.source, LibrarySource::All, "{source:?}, {keyword}");
                assert_eq!(app.session.filter.keyword.as_deref(), Some(keyword));
                assert_eq!(app.session.filter.rating, 5);
                assert_eq!(visible(&mut app), expected, "{source:?}, {keyword}");
                assert!(app.session.active().is_some_and(|id| expected.contains(&id.0)));
                click(&mut app, &ctx, &format!("source:keyword:{keyword}"));
                assert_eq!(app.session.filter.keyword, None);
                assert_eq!(visible(&mut app), [1, 2, 3, 7]);
            }
        }
    }

    #[test]
    fn existing_scoped_filters_are_selected_globally_instead_of_cleared() {
        let (mut app, album) = library();
        let ctx = context();
        app.session.source = LibrarySource::Album(album);
        app.session.filter = Filter { date: Some("2026-04-12".into()), keyword: Some("travel".into()), rating: 5, ..Default::default() };
        assert!(visible(&mut app).is_empty());
        click(&mut app, &ctx, "source:date:2026-04-12");
        assert_eq!(app.session.filter.date.as_deref(), Some("2026-04-12"));
        assert_eq!(app.session.filter.keyword.as_deref(), Some("travel"));
        assert_eq!(visible(&mut app), [2], "date, hierarchical keyword and rating combine");
        app.session.source = LibrarySource::Folder;
        click(&mut app, &ctx, "source:keyword:Travel");
        assert_eq!(app.session.source, LibrarySource::All);
        assert_eq!(app.session.filter.keyword.as_deref(), Some("Travel"), "case-insensitive scoped keyword is selected, not cleared");
        assert_eq!(visible(&mut app), [2]);
    }

    fn selected(app: &LightcraftApp) -> Vec<u64> {
        let mut ids: Vec<_> = app.session.selection.ids.iter().map(|id| id.0).collect();
        ids.sort_unstable();
        ids
    }

    #[test]
    fn global_filter_navigation_prunes_hidden_selection_before_shift_and_bulk_commands() {
        for (row, initial, kept, end, targets) in [
            ("source:date:2026", vec![2, 3, 5], vec![2, 3], 7, vec![2, 3, 7]),
            ("source:date:2026-04-12", vec![3, 2, 5], vec![2], 7, vec![2, 7]),
            ("source:keyword:Travel", vec![2, 5, 7], vec![2], 3, vec![2, 3]),
            ("source:keyword:Travel|Coast", vec![2, 3, 5], vec![2], 2, vec![2]),
        ] {
            let (mut app, _) = library();
            app.session.source = LibrarySource::Folder;
            app.session.browse.as_mut().unwrap().path = "/synthetic-sidebar/elsewhere".into();
            app.session.filter.rating = 5;
            assert!(initial.iter().all(|id| visible(&mut app).contains(id)), "initial selection is visible in Local");
            app.run("library.select", json!({"ids": initial, "active": 2})).unwrap();
            let ctx = context();
            if row.ends_with("Travel|Coast") {
                click(&mut app, &ctx, "keywordToggle:Travel");
            }
            click(&mut app, &ctx, row);
            assert_eq!(app.session.source, LibrarySource::All, "{row}");
            assert_eq!(selected(&app), kept, "keep all matching selections, excluding Local-only and other filtered photos: {row}");
            assert_eq!(app.session.active(), Some(PhotoId(2)), "keep the matching active photo: {row}");
            // Shift-click dispatches this range command against the final globally filtered order.
            app.run("library.select", json!({"ids": [end], "mode": "range"})).unwrap();
            assert_eq!(selected(&app), targets, "Shift must not resurrect hidden targets: {row}");
            let mut command_targets: Vec<_> = app.session.targets(&json!({})).iter().map(|id| id.0).collect();
            command_targets.sort_unstable();
            assert_eq!(command_targets, targets, "{row}");
            app.run("photo.rate", json!({"rating": 4})).unwrap();
            for (id, original) in [(1, 5), (2, 5), (3, 5), (4, 2), (5, 5), (6, 5), (7, 5), (8, 1)] {
                assert_eq!(
                    app.session.catalog.photo(PhotoId(id)).unwrap().rating,
                    if targets.contains(&id) { 4 } else { original },
                    "bulk command touched a hidden or unselected photo {id}: {row}"
                );
            }
        }
    }

    #[test]
    fn global_filter_navigation_keeps_selected_matches_when_the_active_photo_is_filtered_out() {
        for row in ["source:date:2026-04-12", "source:keyword:Travel|Coast"] {
            let (mut app, _) = library();
            app.session.source = LibrarySource::Folder;
            app.session.browse.as_mut().unwrap().path = "/synthetic-sidebar/elsewhere".into();
            app.session.filter.rating = 5;
            for (id, exposure) in [(3, 1.25), (7, 2.5)] {
                let mut settings = lightcraft_develop::DevelopSettings::default();
                settings.light.exposure = exposure;
                app.session
                    .catalog
                    .apply(Op::SetDevelop { id: PhotoId(id), settings: std::sync::Arc::new(settings), label: "fixture".into(), edited: None })
                    .unwrap();
            }
            let mut meta = app.session.catalog.photo(PhotoId(7)).unwrap().meta.clone();
            meta.keywords = vec!["Travel|Coast".into()];
            app.session.catalog.apply(Op::SetMeta { id: PhotoId(7), meta: Box::new(meta) }).unwrap();
            app.run("library.select", json!({"ids": [3, 2, 5], "active": 3})).unwrap();
            assert!(app.session.catalog.photo(PhotoId(3)).unwrap().versions.is_empty());
            assert!(app.session.catalog.photo(PhotoId(7)).unwrap().versions.is_empty());
            let ctx = context();
            if row.ends_with("Travel|Coast") {
                click(&mut app, &ctx, "keywordToggle:Travel");
            }
            click(&mut app, &ctx, row);
            assert_eq!(selected(&app), [2], "{row}");
            assert_eq!(app.session.active(), Some(PhotoId(2)), "prefer a kept selected match over an unrelated first result: {row}");
            assert_eq!(app.session.targets(&json!({})), [PhotoId(2)]);
            assert_eq!(app.session.previous_active, Some(PhotoId(3)), "navigation records the real origin, not a temporary first result: {row}");
            let origin = app.session.catalog.photo(PhotoId(3)).unwrap();
            assert_eq!(origin.versions.len(), 1, "the edited origin receives its automatic version: {row}");
            assert!(origin.versions[0].auto);
            assert_eq!(origin.versions[0].settings.light.exposure, 1.25);
            assert!(
                app.session.catalog.photo(PhotoId(7)).unwrap().versions.is_empty(),
                "the unvisited first result must not receive a version: {row}"
            );
            let pasted = app.run("develop.pastePrevious", json!({"groups": ["light"]})).unwrap();
            assert_eq!(pasted["from"], 3, "{row}");
            assert_eq!(pasted["changed"], 1);
            assert_eq!(app.session.develop_of(PhotoId(2)).unwrap().light.exposure, 1.25, "paste uses the origin's distinct settings: {row}");
            assert_eq!(app.session.develop_of(PhotoId(7)).unwrap().light.exposure, 2.5);
            assert!(app.session.catalog.photo(PhotoId(7)).unwrap().versions.is_empty());
        }
    }

    #[test]
    fn global_date_navigation_preserves_an_empty_date_range_intersection() {
        let (mut app, album) = library();
        app.session.source = LibrarySource::Album(album);
        app.run("library.select", json!({"ids": [1]})).unwrap();
        app.session.filter.date_from = Some("2026-05-01".into());
        app.session.filter.date_to = Some("2026-05-31".into());
        let ctx = context();
        click(&mut app, &ctx, "source:date:2026-04-12");
        assert_eq!(app.session.source, LibrarySource::All);
        assert!(visible(&mut app).is_empty());
        assert_eq!(app.session.source_total(), Some(6), "counts describe the imported, non-deleted library");
        assert_eq!(app.session.filter.date.as_deref(), Some("2026-04-12"));
        assert_eq!(app.session.filter.date_from.as_deref(), Some("2026-05-01"));
        assert_eq!(app.session.filter.date_to.as_deref(), Some("2026-05-31"));
        assert!(selected(&app).is_empty());
        assert_eq!(app.session.active(), None);
        assert!(app.session.targets(&json!({})).is_empty(), "an empty result leaves no hidden command targets");
        let chips = lightcraft_engine::filter_chips(&app.session.filter, &app.session.catalog);
        assert!(chips.iter().any(|c| c.clear == json!({"date": null})));
        assert!(chips.iter().any(|c| c.clear == json!({"dateFrom": null, "dateTo": null})));
        click(&mut app, &ctx, "source:date:2026-04-12");
        assert_eq!(visible(&mut app), [4], "clearing only the clicked date leaves the date range active");
        assert_eq!(app.session.filter.date, None);
        assert_eq!(app.session.filter.date_from.as_deref(), Some("2026-05-01"));
        assert_eq!(app.session.filter.date_to.as_deref(), Some("2026-05-31"));
        assert_eq!(app.session.targets(&json!({})), [PhotoId(4)]);
    }

    /// Draw only the real navigation rows and scrolling photo panel. Thumbnail jobs are held
    /// by an offload, so these fixed-frame tests never decode synthetic paths or await workers.
    struct HeldRenders;

    impl crate::render::RenderOffload for HeldRenders {
        fn try_start(&mut self, _: crate::render::Slot, job: lightcraft_engine::media::RenderJob) -> Option<lightcraft_engine::media::RenderJob> {
            Some(job)
        }

        fn finished(&mut self) -> Vec<(crate::render::Slot, lightcraft_engine::media::RenderResult, f64)> {
            Vec::new()
        }
    }

    struct NavigationView {
        app: LightcraftApp,
        view: crate::headless::HeadlessView,
        film: bool,
        widgets: Vec<(String, egui::Rect)>,
        canvas: egui::Rect,
    }

    impl NavigationView {
        fn new(film: bool) -> Self {
            let mut s = Session::new();
            for id in 1..=40 {
                let mut p = Photo::new(
                    PhotoId(id),
                    Source::File { path: format!("/synthetic-sidebar/scroll/{id}.jpg") },
                    &format!("{id}.jpg"),
                    "JPEG",
                    4,
                    3,
                    "2026-01-01T00:00:00",
                );
                p.captured = Some(if id == 1 { "2026-04-01T00:00:00" } else { "2026-04-20T00:00:00" }.into());
                p.meta.keywords = vec!["Travel".into()];
                s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
            }
            let album = s.execute("album.create", &json!({"name": "Kept active"})).unwrap()["id"].as_u64().unwrap();
            s.execute("album.addPhotos", &json!({"id": album, "ids": [1]})).unwrap();
            s.execute("library.source", &json!({"kind": "album", "id": album})).unwrap();
            s.execute("library.select", &json!({"ids": [1]})).unwrap();
            s.sort.group = lightcraft_catalog::GroupBy::None;
            let mut app = LightcraftApp::new(s, Services::default());
            app.renderer = crate::render::Renderer::with_worker_threads(1);
            app.renderer.set_offload(Box::new(HeldRenders));
            app.ui.thumb_size = 240.0;
            Self { app, view: crate::headless::HeadlessView::new(), film, widgets: vec![], canvas: egui::Rect::NOTHING }
        }

        fn frame(&mut self, events: Vec<egui::Event>) {
            let ctx = self.view.ctx.clone();
            let _ = crate::widgets::take_registry(&ctx);
            let raw = crate::headless::HeadlessView::raw_input(egui::vec2(1000.0, 600.0), 1.0, self.view.frames() as f64 / 60.0, events);
            let app = &mut self.app;
            let film = self.film;
            let canvas = &mut self.canvas;
            self.view.run(raw, |ui| {
                egui::Panel::left("test-navigation").default_size(260.0).size_range(260.0..=260.0).resizable(false).show(ui, |ui| {
                    date_row(app, ui, "2026", "2026", 40, 0.0);
                    let tree = app.caches.keyword_tree(&app.session.catalog);
                    keyword_rows(app, ui, &tree, 0.0);
                });
                egui::CentralPanel::default().show(ui, |ui| {
                    if film {
                        *canvas = egui::Rect::from_min_size(ui.min_rect().min, egui::vec2(ui.available_width(), 120.0));
                        super::super::detail::filmstrip(app, ui, *canvas);
                    } else {
                        super::super::grid::show(app, ui);
                        *canvas = app.canvas_rect.unwrap();
                    }
                });
            });
            self.widgets = crate::widgets::take_registry(&ctx);
        }

        fn idle(&mut self) {
            for _ in 0..24 {
                self.frame(vec![]);
            }
        }

        fn click(&mut self, id: &str) {
            self.frame(vec![]);
            let pos = self.widgets.iter().find(|(w, _)| w == id).unwrap().1.center();
            self.frame(vec![egui::Event::PointerMoved(pos)]);
            for pressed in [true, false] {
                self.frame(vec![egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() }]);
            }
            self.idle();
        }

        fn offset(&self) -> f32 {
            if self.film { self.app.film_scroll.unwrap() } else { self.app.grid_scroll.unwrap() }
        }
    }

    #[test]
    fn global_navigation_follows_kept_active_without_overriding_later_user_scroll() {
        for film in [false, true] {
            for row in ["source:date:2026", "source:keyword:Travel"] {
                let mut h = NavigationView::new(film);
                h.idle();
                assert_eq!(h.app.session.active(), Some(PhotoId(1)));
                assert_eq!(h.offset(), 0.0);
                h.click(row);
                assert_eq!(h.app.session.source, LibrarySource::All);
                assert_eq!(h.app.session.visible().len(), 40);
                assert_eq!(h.app.session.active(), Some(PhotoId(1)), "navigation preserves the active ID");
                let followed = h.offset();
                assert!(followed > 500.0, "kept active moved from one-photo album to end of global {row} (film={film}): {followed}");
                let key = if film { "film:1" } else { "thumb:1" };
                let cell = h.widgets.iter().find(|(w, _)| w == key).unwrap().1;
                assert!(cell.intersects(h.canvas), "follow brings active into view: {cell:?} vs {:?}", h.canvas);
                // The same panels are drawn on every pass, including click and wheel frames:
                // reappearance or an active-ID change cannot accidentally make this test pass.
                h.frame(vec![egui::Event::PointerMoved(h.canvas.center())]);
                h.frame(vec![egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, 400.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: Default::default(),
                }]);
                h.idle();
                let scrolled = h.offset();
                assert!(scrolled < followed - 100.0, "wheel moves away from active: {followed} -> {scrolled}");
                // Clearing this global row leaves exactly the same photos in the same
                // order. It must not pull the user back to the unchanged active photo.
                h.click(row);
                assert!(h.app.session.filter.date.is_none() && h.app.session.filter.keyword.is_none());
                assert!((h.offset() - scrolled).abs() < 1.0, "equal navigation results must keep deliberate scroll: {scrolled} -> {}", h.offset());
                h.app.run("photo.rate", json!({"rating": 3})).unwrap();
                h.idle();
                assert!((h.offset() - scrolled).abs() < 1.0, "unrelated catalog redraw must keep deliberate scroll: {scrolled} -> {}", h.offset());
                assert_eq!(h.app.session.active(), Some(PhotoId(1)));
                assert!(h.view.frames() < 145, "bounded frames; no settle loops or image rendering");
            }
        }
    }

    #[test]
    fn global_rows_drop_local_folder_scope_left_by_a_folder_rename() {
        for (row, expected) in [("source:date:2026-04-12", vec![2, 7]), ("source:keyword:Travel", vec![2, 3])] {
            let (mut app, _) = library();
            // This is the internal scope follow_folder leaves after a successful Local rename.
            app.session.source = LibrarySource::Folder;
            app.session.browse.as_mut().unwrap().path = "/synthetic-sidebar/renamed".into();
            app.session.filter = Filter { folder: Some("/synthetic-sidebar/renamed".into()), subfolders: true, rating: 5, ..Default::default() };
            let ctx = context();
            click(&mut app, &ctx, row);
            assert_eq!(app.session.source, LibrarySource::All);
            assert!(app.session.filter.folder.is_none());
            assert!(!app.session.filter.subfolders);
            assert_eq!(app.session.filter.rating, 5, "independent rating filtering survives global navigation");
            assert_eq!(visible(&mut app), expected);
            assert!(app.session.selection.ids.iter().all(|id| expected.contains(&id.0)));
            click(&mut app, &ctx, row);
            assert_eq!(visible(&mut app), [1, 2, 3, 7], "clearing the row must not restore Local scope or Local-only matches");
        }
    }

    #[test]
    fn date_disclosure_leaves_source_filters_and_results_alone() {
        let (mut app, album) = library();
        let ctx = context();
        app.session.source = LibrarySource::Album(album);
        let filter = app.session.filter.clone();
        assert_eq!(visible(&mut app), [1]);
        click(&mut app, &ctx, "dateToggle:2026");
        assert_eq!(app.session.source, LibrarySource::Album(album));
        assert_eq!(app.session.filter, filter);
        assert_eq!(visible(&mut app), [1]);
    }
}

#[cfg(test)]
mod tests {
    use super::local_places;
    use crate::widgets::elide_head;

    /// Width = characters, so a limit of 12 is "12 characters".
    fn fit(text: &str, max: usize) -> String {
        elide_head(text, max as f32, |s| s.chars().count() as f32)
    }

    #[test]
    fn a_label_that_fits_is_left_alone() {
        assert_eq!(fit("/Users/me/Pictures", 18), "/Users/me/Pictures");
    }

    #[test]
    fn a_long_path_loses_its_leading_folders_not_its_end() {
        assert_eq!(fit("/Users/me/Pictures/Lightroom", 20), "…/Pictures/Lightroom");
        assert_eq!(fit("/Users/me/Pictures/Lightroom", 12), "…/Lightroom");
    }

    #[test]
    fn a_single_name_too_long_is_cut_at_the_end() {
        assert_eq!(fit("2024-summer-holiday", 8), "2024-su…");
        assert_eq!(fit("2024-summer-holiday", 1), "…");
        assert_eq!(fit("2024-summer-holiday", 0), "…");
    }

    #[test]
    fn multibyte_names_are_cut_on_character_boundaries() {
        assert_eq!(fit("/写真/夏休み旅行の記録", 6), "夏休み旅行…");
    }

    fn names(v: &[(String, String)]) -> Vec<&str> {
        v.iter().map(|(n, _)| n.as_str()).collect()
    }

    /// A built-in location browsed (or picked) under another spelling of its path is the same
    /// row, not a second root — for every built-in, not just named ones.
    #[test]
    fn equivalent_paths_are_one_local_location() {
        for (home, sep) in [("D:\\Example", '\\'), ("D:/Example", '/'), ("/home/example", '/')] {
            let builtin: Vec<(String, String)> =
                ["Pictures", "Desktop", "Downloads"].iter().map(|s| (s.to_string(), format!("{home}{sep}{s}"))).collect();
            for (_, p) in builtin.clone() {
                // the browsed spelling: other separators, a trailing one, a different drive-letter case
                let flipped = p.replace(['/', '\\'], if sep == '/' { "\\" } else { "/" });
                for browsing in [flipped.clone(), format!("{p}{sep}"), p.replacen("D:", "d:", 1)] {
                    let l = local_places(builtin.clone(), std::slice::from_ref(&browsing), Some(&browsing), None, &[]);
                    assert_eq!(names(&l.places), ["Pictures", "Desktop", "Downloads"], "{p} browsed as {browsing}");
                    assert_eq!(l.places[l.owner.unwrap()].1, p, "the built-in row is the one highlighted");
                }
                // hiding under one spelling hides the other
                let l = local_places(builtin.clone(), &[], None, None, std::slice::from_ref(&flipped));
                assert_eq!(l.places.len(), 2, "hidden {flipped}");
            }
        }
        // a different folder is still added
        let l = local_places(vec![("Pictures".into(), "/home/example/Pictures".into())], &[], Some("/home/example/Pictures2"), None, &[]);
        assert_eq!(names(&l.places), ["Pictures", "Pictures2"]);
    }

    /// A folder inside a kept root is shown inside that root's tree: the root stays (its other
    /// subfolders stay reachable) and no second root appears; other kept roots stay too.
    #[test]
    fn browsing_below_a_kept_root_keeps_the_root() {
        let photos = "/data/Photos".to_string();
        let other = "/data/Scans".to_string();
        let saved = [photos.clone(), other.clone()];
        let builtin = || vec![("Home".to_string(), "/home/example".to_string())];
        for browsing in ["/data/Photos/2026/20260101", "/data/Photos/2026", "/data/Photos", "D:\\x"] {
            let l = local_places(builtin(), &saved, Some(browsing), None, &[]);
            let roots: Vec<&str> = l.places.iter().map(|(_, p)| p.as_str()).collect();
            if browsing.starts_with("/data") {
                assert_eq!(roots, ["/home/example", "/data/Photos", "/data/Scans"], "{browsing}");
                assert_eq!(l.owner, Some(1), "the Photos tree opens down to {browsing}");
                assert_eq!(l.browse_root, None);
            } else {
                assert_eq!(roots, ["/home/example", "/data/Photos", "/data/Scans", "D:\\x"], "a folder outside them gets a row");
            }
        }
        // the innermost containing root is the one that opens
        let l = local_places(builtin(), &["/home/example/Pictures".into()], Some("/home/example/Pictures/Trip"), None, &[]);
        assert_eq!(l.owner, Some(1));
    }

    /// A browsed folder outside every kept root gets a session row, which stays while browsing
    /// below it and gives way when browsing moves elsewhere.
    #[test]
    fn session_root_stays_while_browsing_below_it() {
        let l = local_places(Vec::new(), &[], Some("/t/base"), None, &[]);
        assert_eq!(l.browse_root.as_deref(), Some("/t/base"));
        let l = local_places(Vec::new(), &[], Some("/t/base/Trip/Day 1"), l.browse_root.as_deref(), &[]);
        assert_eq!((l.places.len(), l.browse_root.as_deref(), l.owner), (1, Some("/t/base"), Some(0)), "{l:?}");
        let l = local_places(Vec::new(), &[], Some("/elsewhere"), l.browse_root.as_deref(), &[]);
        assert_eq!(l.browse_root.as_deref(), Some("/elsewhere"));
        // hidden: no row at all
        let l = local_places(Vec::new(), &[], Some("/t/base"), None, &["/t/base/".into()]);
        assert!(l.places.is_empty() && l.browse_root.is_none());
    }

    /// A hidden folder inside a listed one (a kept folder beneath Home) is not revealed in that
    /// one's tree while it is browsed, nor are the folders below it; hiding Home itself still
    /// lets Pictures open down to a folder browsed inside it.
    #[test]
    fn hidden_folder_is_not_revealed_in_an_outer_tree() {
        let builtin = || vec![("Pictures".to_string(), "/home/example/Pictures".to_string()), ("Home".to_string(), "/home/example".to_string())];
        let kept = ["/home/example/AppData/Temp/lc".to_string()];
        let l = local_places(builtin(), &kept, Some("/home/example/AppData/Temp/lc"), None, &[]);
        assert_eq!(l.owner, Some(2), "shown as its own kept row");
        for browsing in ["/home/example/AppData/Temp/lc", "/home/example/AppData/Temp/lc/Day 1"] {
            let l = local_places(builtin(), &kept, Some(browsing), None, &["/home/example/AppData/Temp/lc/".into()]);
            assert_eq!(names(&l.places), ["Pictures", "Home"], "{browsing}");
            assert_eq!((l.owner, l.browse_root.as_deref()), (None, None), "Home does not open down to {browsing}");
        }
        let l = local_places(builtin(), &[], Some("/home/example/Pictures/Trip"), None, &["/home/example".into()]);
        assert_eq!((names(&l.places), l.owner), (vec!["Pictures"], Some(0)));
    }
}
