#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use organizer_core::analysis::{self, Analysis, Child, FileInfo, Settings};
use organizer_core::daw::{self, DetectedDaw};
use organizer_core::duplicates::{self, DupGroup};
use organizer_core::trash::{Trash, TrashItem};
use organizer_core::UsageState;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};

struct App {
    settings: Settings,
    analysis: Option<Analysis>,
    trash: Trash,
    data_dir: PathBuf,
}
type Shared<'a> = State<'a, Mutex<App>>;

static LAST_PANIC: Mutex<String> = Mutex::new(String::new());

/// Record every panic in memory (shown in the UI) and in crash.log next to the settings.
fn install_panic_hook(log: PathBuf) {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = format!("{info}");
        if let Ok(mut m) = LAST_PANIC.lock() { *m = msg.clone(); }
        let _ = std::fs::create_dir_all(log.parent().unwrap_or(std::path::Path::new(".")));
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log) {
            let _ = writeln!(f, "[{}] {msg}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0));
        }
        prev(info);
    }));
}

fn err<E: std::fmt::Display>(e: E) -> String { e.to_string() }

fn settings_path(d: &PathBuf) -> PathBuf { d.join("settings.json") }
fn bins_path(d: &PathBuf) -> PathBuf { d.join("trash-bins.json") }

fn save_bins(app: &App) {
    let bins: Vec<&PathBuf> = app.trash.bins().iter().collect();
    let _ = std::fs::write(bins_path(&app.data_dir), serde_json::to_vec(&bins).unwrap_or_default());
}

#[derive(Serialize)]
struct Boot { settings: Settings, daws: Vec<DetectedDaw>, first_run: bool, platform: &'static str }

#[tauri::command]
fn boot(app: Shared) -> Boot {
    let a = app.lock().unwrap();
    Boot { settings: a.settings.clone(), daws: daw::detect(), first_run: !settings_path(&a.data_dir).exists(), platform: if cfg!(windows) { "windows" } else { "macos" } }
}

#[tauri::command]
fn save_settings(app: Shared, settings: Settings) -> Result<(), String> {
    let mut a = app.lock().unwrap();
    std::fs::create_dir_all(&a.data_dir).map_err(err)?;
    std::fs::write(settings_path(&a.data_dir), serde_json::to_vec_pretty(&settings).map_err(err)?).map_err(err)?;
    a.settings = settings;
    Ok(())
}

#[tauri::command]
async fn run_scan(handle: AppHandle, settings: Settings) -> Result<analysis::Report, String> {
    let cache = handle.state::<Mutex<App>>().lock().unwrap().data_dir.join("project-cache.json");
    let mut s = settings;
    s.cache_path = Some(cache);
    let h = handle.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| analysis::run(&s, &|p| { let _ = h.emit("scan-progress", p); })))
    }).await.map_err(err)?.map_err(|_| {
        let msg = LAST_PANIC.lock().map(|m| m.clone()).unwrap_or_default();
        format!("Скан прервался из-за внутренней ошибки. Подробности записаны в crash.log в папке настроек.\n{msg}")
    })?;
    let report = result.report.clone();
    handle.state::<Mutex<App>>().lock().unwrap().analysis = Some(result);
    Ok(report)
}

#[tauri::command]
fn sample_children(app: Shared, dir: Option<String>) -> Vec<Child> {
    app.lock().unwrap().analysis.as_ref().map(|a| a.samples.children(dir.as_deref())).unwrap_or_default()
}

#[tauri::command]
fn sample_top(app: Shared, state: UsageState, under: Option<String>, limit: usize) -> Vec<FileInfo> {
    app.lock().unwrap().analysis.as_ref().map(|a| a.samples.top_files(state, under.as_deref(), limit)).unwrap_or_default()
}

#[tauri::command]
fn sample_info(app: Shared, idx: u32) -> Option<FileInfo> {
    app.lock().unwrap().analysis.as_ref().map(|a| a.samples.file_info(idx))
}

#[tauri::command]
async fn find_duplicates(handle: AppHandle, min_size: u64) -> Result<Vec<(DupGroup, Vec<FileInfo>)>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let st = handle.state::<Mutex<App>>();
        let a = st.lock().unwrap();
        let Some(an) = a.analysis.as_ref() else { return Vec::new() };
        duplicates::find(&an.samples, min_size).into_iter().take(500).map(|g| { let f = g.files.iter().map(|&i| an.samples.file_info(i)).collect(); (g, f) }).collect()
    }).await.map_err(err)
}

#[derive(Deserialize)]
struct TrashRequest { kind: String, /// plugin file path, or sample index as string
    key: String, label: String, #[serde(default)] allow_old: bool }

#[derive(Serialize)]
struct TrashResult { moved: Vec<TrashItem>, errors: Vec<String> }

/// The UI is never trusted: every request is re-checked against the last scan.
#[tauri::command]
fn trash_move(app: Shared, items: Vec<TrashRequest>) -> TrashResult {
    let mut guard = app.lock().unwrap();
    let a = &mut *guard;
    let mut moved = Vec::new();
    let mut errors = Vec::new();
    for r in items {
        let Some(an) = a.analysis.as_ref() else { errors.push("Нет результатов скана".into()); break };
        let target: Result<(PathBuf, u64), String> = if r.kind == "plugin" {
            an.report.plugins.iter().flat_map(|p| p.formats.iter()).find(|f| f.path == r.key)
                .ok_or_else(|| format!("{}: плагин не найден в результатах скана", r.label))
                .and_then(|f| if f.removable { Ok((PathBuf::from(&f.path), f.size)) } else { Err(format!("{}: файл нужен проектам, удалять нельзя", r.label)) })
        } else {
            r.key.parse::<u32>().ok().and_then(|i| an.samples.entries.get(i as usize).map(|e| (i, e)))
                .ok_or_else(|| format!("{}: файл не найден в результатах скана", r.label))
                .and_then(|(i, e)| match e.state {
                    UsageState::Unused => Ok((an.samples.fs_path(i), e.size)),
                    UsageState::OldOnly if r.allow_old => Ok((an.samples.fs_path(i), e.size)),
                    _ => Err(format!("{}: файл используется или принадлежит проекту", r.label)),
                })
        };
        match target {
            Err(e) => errors.push(e),
            Ok((path, size)) => match a.trash.trash(&path, &r.kind, &r.label, size) {
                Ok(it) => {
                    if r.kind == "sample" { if let Some(an) = a.analysis.as_mut() { an.samples.set_gone_by_path(&it.original_path, true); } }
                    moved.push(it)
                }
                Err(e) => {
                    let msg = e.to_string();
                    let hint = if msg.to_lowercase().contains("denied") || msg.contains("os error 5") || msg.contains("Отказано") { " Нет прав на папку. Запустите приложение от имени администратора." } else { "" };
                    errors.push(format!("{}: {msg}.{hint}", r.label))
                }
            },
        }
    }
    save_bins(a);
    TrashResult { moved, errors }
}

#[tauri::command]
fn trash_list(app: Shared) -> Vec<TrashItem> { app.lock().unwrap().trash.list() }

#[tauri::command]
fn trash_restore(app: Shared, ids: Vec<String>) -> Vec<String> {
    let mut guard = app.lock().unwrap();
    let a = &mut *guard;
    let items = a.trash.list();
    let mut errors = Vec::new();
    for id in ids {
        match a.trash.restore(&id) {
            Ok(()) => if let (Some(it), Some(an)) = (items.iter().find(|i| i.id == id), a.analysis.as_mut()) { an.samples.set_gone_by_path(&it.original_path, false); },
            Err(e) => errors.push(e.to_string()),
        }
    }
    errors
}

#[tauri::command]
fn sample_summary(app: Shared) -> Option<analysis::SampleSummary> {
    app.lock().unwrap().analysis.as_ref().map(|a| a.samples.summary())
}

#[tauri::command]
fn trash_purge(app: Shared, ids: Vec<String>) -> Result<u64, String> { app.lock().unwrap().trash.purge(&ids).map_err(err) }

#[tauri::command]
fn trash_empty(app: Shared) -> Result<u64, String> { app.lock().unwrap().trash.empty().map_err(err) }

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir().unwrap_or_else(|_| dirs::data_dir().unwrap_or_default().join("AbletonOrganizer"));
            let settings: Settings = std::fs::read(settings_path(&data_dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
            let bins: Vec<PathBuf> = std::fs::read(bins_path(&data_dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
            install_panic_hook(data_dir.join("crash.log"));
            let home = dirs::home_dir().unwrap_or_default();
            app.manage(Mutex::new(App { settings, analysis: None, trash: Trash::new(home, bins), data_dir }));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![boot, save_settings, run_scan, sample_children, sample_top, sample_info, sample_summary, find_duplicates, trash_move, trash_list, trash_restore, trash_purge, trash_empty])
        .run(tauri::generate_context!())
        .expect("error while running Ableton Organizer");
}
