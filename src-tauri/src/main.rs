#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Desktop shell. Every command is async and heavy work runs on worker threads,
//! so the window never freezes. The scan result sits behind a RwLock: browsing
//! reads it in parallel; only moving files to the trash writes.

use organizer_core::analysis::{self, Analysis, FileInfo, Listing, Settings};
use organizer_core::daw::{self, DetectedDaw};
use organizer_core::duplicates::{self, DupGroup};
use organizer_core::system::{self, Uninstaller};
use organizer_core::trash::{Trash, TrashItem};
use organizer_core::waveform::{self, Wave};
use organizer_core::UsageState;
use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, RwLock};
use tauri::{AppHandle, Emitter, Manager, State};

struct AppState {
    data_dir: PathBuf,
    settings: Mutex<Settings>,
    analysis: RwLock<Option<Analysis>>,
    trash: Mutex<Trash>,
    dup_cancel: AtomicBool,
    uninstallers: Mutex<Option<Vec<Uninstaller>>>,
    waves: Mutex<HashMap<String, Wave>>,
}
type St<'a> = State<'a, AppState>;
type R<T> = Result<T, String>;

static LAST_PANIC: Mutex<String> = Mutex::new(String::new());

fn install_panic_hook(log: PathBuf) {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = format!("{info}");
        if let Ok(mut m) = LAST_PANIC.lock() { *m = msg.clone(); }
        let _ = std::fs::create_dir_all(log.parent().unwrap_or(Path::new(".")));
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log) {
            let _ = writeln!(f, "[{}] {msg}", now());
        }
        prev(info);
    }));
}

fn now() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }
fn err<E: std::fmt::Display>(e: E) -> String { e.to_string() }
fn settings_path(d: &Path) -> PathBuf { d.join("settings.json") }
fn bins_path(d: &Path) -> PathBuf { d.join("trash-bins.json") }
fn cache_path(d: &Path) -> PathBuf { d.join("project-cache.json") }
fn log_path(d: &Path) -> PathBuf { d.join("crash.log") }

/// Run blocking work off the UI thread and turn panics into readable errors.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> R<T> + Send + 'static) -> R<T> {
    tauri::async_runtime::spawn_blocking(move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|_| {
            let msg = LAST_PANIC.lock().map(|m| m.clone()).unwrap_or_default();
            Err(format!("Внутренняя ошибка. Подробности в crash.log в папке настроек.\n{msg}"))
        })
    }).await.map_err(err)?
}

fn with_analysis<T>(app: &AppHandle, f: impl FnOnce(&Analysis) -> T) -> R<T> {
    let st = app.state::<AppState>();
    let g = st.analysis.read().map_err(err)?;
    g.as_ref().map(f).ok_or_else(|| "Сначала выполните скан".to_string())
}

// ---------- setup ----------

#[derive(Serialize)]
struct Boot { settings: Settings, daws: Vec<DetectedDaw>, first_run: bool, platform: &'static str }

#[tauri::command]
async fn boot(app: AppHandle) -> R<Boot> {
    blocking(move || {
        let st = app.state::<AppState>();
        let settings = st.settings.lock().map_err(err)?.clone();
        Ok(Boot { settings, daws: daw::detect(), first_run: !settings_path(&st.data_dir).exists(), platform: if cfg!(windows) { "windows" } else { "macos" } })
    }).await
}

#[tauri::command]
async fn save_settings(st: St<'_>, settings: Settings) -> R<()> {
    std::fs::create_dir_all(&st.data_dir).map_err(err)?;
    std::fs::write(settings_path(&st.data_dir), serde_json::to_vec_pretty(&settings).map_err(err)?).map_err(err)?;
    *st.settings.lock().map_err(err)? = settings;
    Ok(())
}

#[tauri::command]
async fn run_scan(app: AppHandle, settings: Settings) -> R<analysis::Report> {
    blocking(move || {
        let st = app.state::<AppState>();
        let mut s = settings;
        s.cache_path = Some(cache_path(&st.data_dir));
        let h = app.clone();
        let a = analysis::run(&s, &|p| { let _ = h.emit("scan-progress", p); });
        let report = a.report.clone();
        *st.analysis.write().map_err(err)? = Some(a);
        Ok(report)
    }).await
}

// ---------- browsing ----------

#[tauri::command]
async fn sample_children(app: AppHandle, dir: Option<String>) -> R<Listing> {
    blocking(move || with_analysis(&app, |a| a.samples.children(dir.as_deref()))).await
}

#[tauri::command]
async fn sample_top(app: AppHandle, state: UsageState, under: Option<String>, limit: usize) -> R<Vec<FileInfo>> {
    blocking(move || with_analysis(&app, |a| a.samples.top_files(state, under.as_deref(), limit))).await
}

#[tauri::command]
async fn sample_info(app: AppHandle, idx: u32) -> R<FileInfo> {
    blocking(move || with_analysis(&app, |a| a.samples.file_info(idx))).await
}

#[tauri::command]
async fn sample_summary(app: AppHandle) -> R<analysis::SampleSummary> {
    blocking(move || with_analysis(&app, |a| a.samples.summary())).await
}

// ---------- duplicates ----------

#[tauri::command]
async fn find_duplicates(app: AppHandle, min_size: u64) -> R<Option<Vec<(DupGroup, Vec<FileInfo>)>>> {
    app.state::<AppState>().dup_cancel.store(false, Ordering::Relaxed);
    blocking(move || {
        let st = app.state::<AppState>();
        let g = st.analysis.read().map_err(err)?;
        let an = g.as_ref().ok_or("Сначала выполните скан")?;
        let h = app.clone();
        let res = duplicates::find(&an.samples, min_size, &|p| { let _ = h.emit("dup-progress", p); }, &st.dup_cancel);
        Ok(res.map(|groups| groups.into_iter().take(1000).map(|g| { let f = g.files.iter().map(|&i| an.samples.file_info(i)).collect(); (g, f) }).collect()))
    }).await
}

#[tauri::command]
async fn cancel_duplicates(st: St<'_>) -> R<()> { st.dup_cancel.store(true, Ordering::Relaxed); Ok(()) }

// ---------- trash ----------

#[derive(Deserialize)]
struct TrashRequest {
    kind: String,
    /// plugin file path, or sample index as string
    key: String,
    label: String,
    #[serde(default)]
    allow_old: bool,
    /// The user confirmed removing something projects still use.
    #[serde(default)]
    force: bool,
}

#[derive(Serialize)]
struct TrashResult { moved: Vec<TrashItem>, errors: Vec<String> }

fn save_bins(st: &AppState, t: &Trash) {
    let _ = std::fs::write(bins_path(&st.data_dir), serde_json::to_vec(&t.bins()).unwrap_or_default());
}

/// The UI is never trusted: every request is re-checked against the scan and
/// against the disk right before anything is moved.
#[tauri::command]
async fn trash_move(app: AppHandle, items: Vec<TrashRequest>) -> R<TrashResult> {
    blocking(move || {
        let st = app.state::<AppState>();
        let settings = st.settings.lock().map_err(err)?.clone();
        // 1. projects must be exactly as they were when the scan ran
        {
            let g = st.analysis.read().map_err(err)?;
            let an = g.as_ref().ok_or("Сначала выполните скан")?;
            if let Some(why) = analysis::projects_changed(&settings, &an.stamps) {
                return Err(format!("{why}.\nНажмите «Обновить скан», затем повторите. Ничего не перемещено."));
            }
        }
        let mut g = st.analysis.try_write().map_err(|_| "Подождите окончания поиска дубликатов".to_string())?;
        let an = g.as_mut().ok_or("Сначала выполните скан")?;
        let mut trash = st.trash.lock().map_err(err)?;
        let mut moved = Vec::new();
        let mut errors = Vec::new();
        for r in items {
            let target: Result<(PathBuf, u64), String> = if r.kind == "plugin" {
                an.report.plugins.iter().flat_map(|p| p.formats.iter()).find(|f| f.path == r.key)
                    .ok_or_else(|| format!("{}: плагин не найден в результатах скана", r.label))
                    .and_then(|f| if f.removable || r.force { Ok((PathBuf::from(&f.path), f.size)) } else { Err(format!("{}: нужен проектам. Подтвердите удаление нужного в окне подтверждения", r.label)) })
            } else {
                r.key.parse::<u32>().ok().and_then(|i| an.samples.entries.get(i as usize).map(|e| (i, e)))
                    .ok_or_else(|| format!("{}: файл не найден в результатах скана", r.label))
                    .and_then(|(i, e)| {
                        let allowed = r.force || matches!(e.state, UsageState::Unused) || (r.allow_old && matches!(e.state, UsageState::OldOnly));
                        if !allowed { return Err(format!("{}: используется или принадлежит проекту", r.label)); }
                        // 2. the file itself must not have changed since the scan
                        let p = an.samples.fs_path(i);
                        let md = std::fs::metadata(&p).map_err(|_| format!("{}: файла больше нет на месте", r.label))?;
                        let mtime = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
                        if md.len() != e.size || mtime != e.mtime { return Err(format!("{}: файл изменился после скана, пропущен", r.label)); }
                        Ok((p, e.size))
                    })
            };
            match target {
                Err(e) => errors.push(e),
                Ok((path, size)) => match trash.trash(&path, &r.kind, &r.label, size) {
                    Ok(it) => {
                        if r.kind == "sample" { an.samples.set_gone_by_path(&it.original_path, true); }
                        moved.push(it)
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        let denied = msg.contains("os error 5") || msg.to_lowercase().contains("denied") || msg.contains("Отказано");
                        errors.push(format!("{}: {msg}{}", r.label, if denied { "\nНет прав на папку. Запустите программу от имени администратора." } else { "" }))
                    }
                },
            }
        }
        save_bins(&st, &trash);
        Ok(TrashResult { moved, errors })
    }).await
}

#[tauri::command]
async fn trash_list(app: AppHandle) -> R<Vec<TrashItem>> {
    blocking(move || Ok(app.state::<AppState>().trash.lock().map_err(err)?.list())).await
}

#[tauri::command]
async fn trash_restore(app: AppHandle, ids: Vec<String>) -> R<Vec<String>> {
    blocking(move || {
        let st = app.state::<AppState>();
        let trash = st.trash.lock().map_err(err)?;
        let items = trash.list();
        let mut errors = Vec::new();
        for id in ids {
            match trash.restore(&id) {
                Ok(()) => if let Some(it) = items.iter().find(|i| i.id == id) {
                    if let Ok(mut g) = st.analysis.write() { if let Some(an) = g.as_mut() { an.samples.set_gone_by_path(&it.original_path, false); } }
                },
                Err(e) => errors.push(e.to_string()),
            }
        }
        Ok(errors)
    }).await
}

#[tauri::command]
async fn trash_purge(app: AppHandle, ids: Vec<String>) -> R<u64> {
    blocking(move || app.state::<AppState>().trash.lock().map_err(err)?.purge(&ids).map_err(err)).await
}

#[tauri::command]
async fn trash_empty(app: AppHandle) -> R<u64> {
    blocking(move || app.state::<AppState>().trash.lock().map_err(err)?.empty().map_err(err)).await
}

// ---------- system integration ----------

#[tauri::command]
async fn reveal_path(path: String) -> R<()> {
    blocking(move || system::reveal(Path::new(&path)).map_err(err)).await
}

#[tauri::command]
async fn web_search(query: String, images: Option<bool>) -> R<()> {
    blocking(move || system::web_search(&query, images.unwrap_or(true)).map_err(err)).await
}

#[tauri::command]
async fn show_properties(path: String) -> R<()> {
    blocking(move || system::show_properties(Path::new(&path)).map_err(err)).await
}

#[derive(Serialize)]
struct PluginInfo { formats: Vec<organizer_core::details::PluginDetails>, vendor: Option<String>, publisher_from: Option<String> }

/// Vendor, version, effect type and dates for each format of a plugin.
#[tauri::command]
async fn plugin_details(app: AppHandle, name: String, paths: Vec<String>) -> R<PluginInfo> {
    blocking(move || {
        let formats: Vec<_> = paths.iter().map(|p| organizer_core::details::read(Path::new(p), &name)).collect();
        let mut vendor = formats.iter().find_map(|f| f.vendor.clone());
        let mut publisher_from = None;
        if vendor.is_none() {
            // fall back to the publisher of the matching Windows uninstaller
            let st = app.state::<AppState>();
            let mut cache = st.uninstallers.lock().map_err(err)?;
            if cache.is_none() { *cache = Some(system::list_uninstallers()); }
            if let Some(u) = system::match_uninstallers(&name, &paths, cache.as_ref().unwrap()).into_iter().find(|u| !u.publisher.is_empty()) {
                vendor = Some(u.publisher.clone());
                publisher_from = Some(u.name.clone());
            }
        }
        Ok(PluginInfo { formats, vendor, publisher_from })
    }).await
}

#[tauri::command]
async fn plugin_uninstallers(app: AppHandle, name: String, paths: Vec<String>) -> R<Vec<Uninstaller>> {
    blocking(move || {
        let st = app.state::<AppState>();
        let mut cache = st.uninstallers.lock().map_err(err)?;
        if cache.is_none() { *cache = Some(system::list_uninstallers()); }
        Ok(system::match_uninstallers(&name, &paths, cache.as_ref().unwrap()))
    }).await
}

/// Only uninstallers found in the registry can be launched, by their key.
#[tauri::command]
async fn run_uninstaller(app: AppHandle, id: String) -> R<()> {
    blocking(move || {
        let st = app.state::<AppState>();
        let fresh = system::list_uninstallers();
        let u = fresh.iter().find(|u| u.id == id).ok_or("Деинсталлятор не найден в системе")?;
        system::launch_uninstaller(u).map_err(err)?;
        *st.uninstallers.lock().map_err(err)? = None; // list will change after uninstall
        Ok(())
    }).await
}

// ---------- waveform ----------

/// Waveform overview of an audio file. Results are kept in memory for the session.
#[tauri::command]
async fn wave(app: AppHandle, path: String, buckets: usize) -> R<Wave> {
    blocking(move || {
        let st = app.state::<AppState>();
        let key = format!("{buckets}|{path}");
        if let Some(w) = st.waves.lock().map_err(err)?.get(&key) { return Ok(w.clone()); }
        let fs_path = if cfg!(windows) { PathBuf::from(path.replace('/', "\\")) } else { PathBuf::from(&path) };
        let w = waveform::compute(&fs_path, buckets).map_err(err)?;
        let mut cache = st.waves.lock().map_err(err)?;
        if cache.len() > 400 { cache.clear(); }
        cache.insert(key, w.clone());
        Ok(w)
    }).await
}

// ---------- program cache ----------

#[derive(Serialize)]
struct CacheInfo { bytes: u64, files: Vec<String>, folder: String }

fn cache_files(d: &Path) -> Vec<PathBuf> {
    [cache_path(d), log_path(d)].into_iter().filter(|p| p.exists()).collect()
}

#[tauri::command]
async fn cache_info(app: AppHandle) -> R<CacheInfo> {
    blocking(move || {
        let d = app.state::<AppState>().data_dir.clone();
        let files = cache_files(&d);
        let bytes = files.iter().filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len()).sum();
        Ok(CacheInfo { bytes, files: files.iter().map(|p| p.to_string_lossy().into_owned()).collect(), folder: d.to_string_lossy().into_owned() })
    }).await
}

/// Deletes the project cache and logs. Settings and the trash are kept.
#[tauri::command]
async fn clear_cache(app: AppHandle) -> R<u64> {
    blocking(move || {
        let d = app.state::<AppState>().data_dir.clone();
        let mut freed = 0;
        for p in cache_files(&d) {
            freed += std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            std::fs::remove_file(&p).map_err(err)?;
        }
        Ok(freed)
    }).await
}

/// The app used to be called Ableton Organizer. Carry settings, trash locations
/// and the project cache over once, so nothing is lost after the rename.
fn migrate_from_old_name(new_dir: &Path) {
    if settings_path(new_dir).exists() { return; }
    let Some(parent) = new_dir.parent() else { return };
    let old = parent.join("app.abletonorganizer.desktop");
    if !settings_path(&old).exists() { return; }
    let _ = std::fs::create_dir_all(new_dir);
    for name in ["settings.json", "trash-bins.json", "project-cache.json"] {
        let _ = std::fs::copy(old.join(name), new_dir.join(name));
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir().unwrap_or_else(|_| dirs::data_dir().unwrap_or_default().join("Debridr"));
            migrate_from_old_name(&data_dir);
            install_panic_hook(log_path(&data_dir));
            let settings: Settings = std::fs::read(settings_path(&data_dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
            let bins: Vec<PathBuf> = std::fs::read(bins_path(&data_dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
            let home = dirs::home_dir().unwrap_or_default();
            app.manage(AppState { data_dir, settings: Mutex::new(settings), analysis: RwLock::new(None), trash: Mutex::new(Trash::new(home, bins)),
                dup_cancel: AtomicBool::new(false), uninstallers: Mutex::new(None), waves: Mutex::new(HashMap::new()) });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            boot, save_settings, run_scan, sample_children, sample_top, sample_info, sample_summary,
            find_duplicates, cancel_duplicates, trash_move, trash_list, trash_restore, trash_purge, trash_empty,
            reveal_path, web_search, show_properties, plugin_details, plugin_uninstallers, run_uninstaller, cache_info, clear_cache, wave
        ])
        .run(tauri::generate_context!())
        .expect("error while running Debridr");
}
