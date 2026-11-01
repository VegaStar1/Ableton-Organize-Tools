//! Combines installed plugins, parsed projects and the sample index into the
//! "what is used, what is safe to remove" report.
use crate::cache::{self, Cache};
use crate::daw::{Daw, SupportLevel};
use crate::plugins::{self, family_key};
use crate::projects::{self, generic::NameMatcher, ProjectFile};
use crate::samples::{self, SampleFile};
use crate::{PluginFormat, ProjectScan, UsageState};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UnknownPolicy {
    /// Usage found by a heuristic reader protects every format that DAW can load.
    Safe,
    /// It protects only the best format (VST3 first).
    Balanced,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub daws: Vec<Daw>,
    pub project_dirs: Vec<PathBuf>,
    pub sample_dirs: Vec<PathBuf>,
    pub extra_plugin_dirs: Vec<PathBuf>,
    pub library_dirs: Vec<PathBuf>,
    pub old_days: u32,
    pub include_backups: bool,
    pub preferred_format: PluginFormat,
    pub unknown_policy: UnknownPolicy,
    pub cache_path: Option<PathBuf>,
    /// Keep every library file whose *name* appears in any project, even when
    /// the project points to a different folder. Safer, on by default.
    pub strict_names: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self { daws: vec![Daw::Ableton], project_dirs: vec![], sample_dirs: vec![], extra_plugin_dirs: vec![], library_dirs: vec![],
            old_days: 365, include_backups: false, preferred_format: PluginFormat::Vst3, unknown_policy: UnknownPolicy::Safe, cache_path: None, strict_names: true }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Progress { pub stage: &'static str, pub done: u64, pub total: u64 }

#[derive(Debug, Clone, Serialize)]
pub struct UsedBy { pub project: String, pub daw: Daw, pub exact: bool }

#[derive(Debug, Clone, Serialize)]
pub struct FormatReport {
    pub format: PluginFormat,
    pub path: String,
    pub size: u64,
    pub used_by: Vec<UsedBy>,
    /// DAWs whose projects could not be read precisely but can load this format.
    pub protected_by: Vec<Daw>,
    pub removable: bool,
    /// Why it cannot (or can) be removed, in plain words for the UI.
    pub reason: String,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PluginStatus { Used, Duplicate, Unused }

#[derive(Debug, Clone, Serialize)]
pub struct PluginReport {
    pub name: String,
    pub formats: Vec<FormatReport>,
    pub used_projects: usize,
    pub status: PluginStatus,
    /// Format paths recommended for removal.
    pub recommended: Vec<String>,
    pub total_size: u64,
    pub recommended_size: u64,
    /// Local picture of the plugin, when one ships with it.
    pub image: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectPlugin { pub name: String, pub format: Option<PluginFormat>, pub state: &'static str }

#[derive(Debug, Clone, Serialize)]
pub struct ProjectReport {
    pub id: usize,
    pub name: String,
    pub daw: Daw,
    pub path: String,
    pub mtime: u64,
    pub size: u64,
    pub plugins: Vec<ProjectPlugin>,
    pub file_refs: usize,
    pub missing: Vec<String>,
    pub old: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct SampleSummary {
    pub files: u64, pub bytes: u64,
    pub used_bytes: u64, pub old_bytes: u64, pub unused_bytes: u64, pub owned_bytes: u64,
    pub unused_files: u64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Timings { pub plugins_ms: u128, pub projects_ms: u128, pub samples_ms: u128, pub analysis_ms: u128, pub cached_projects: usize, pub parsed_projects: usize }

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub plugins: Vec<PluginReport>,
    pub projects: Vec<ProjectReport>,
    pub samples: SampleSummary,
    pub timings: Timings,
    pub warnings: Vec<String>,
    /// Project or preset files that could not be read at all. Every sample in
    /// their folders is kept.
    pub unreadable: Vec<String>,
}

pub struct SampleEntry {
    pub slash: String,
    pub size: u64,
    pub mtime: u64,
    pub state: UsageState,
    pub by_name: bool,
    /// Moved to the in-app trash after the scan.
    pub gone: bool,
    /// Folder node this file sits in.
    pub node: u32,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Agg { pub bytes: u64, pub files: u64, pub used_bytes: u64, pub old_bytes: u64, pub unused_bytes: u64, pub owned_bytes: u64 }

impl Agg {
    fn apply(&mut self, e: &SampleEntry, sign: i64) {
        let add = |v: &mut u64, x: u64| { *v = if sign > 0 { *v + x } else { v.saturating_sub(x) } };
        add(&mut self.bytes, e.size);
        add(&mut self.files, 1);
        match e.state {
            UsageState::Used => add(&mut self.used_bytes, e.size),
            UsageState::OldOnly => add(&mut self.old_bytes, e.size),
            UsageState::ProjectOwned => add(&mut self.owned_bytes, e.size),
            UsageState::Unused => add(&mut self.unused_bytes, e.size),
        }
    }
}

/// A folder inside a sample library, with totals for everything below it.
pub struct DirNode {
    pub path: String,
    pub name: String,
    pub parent: Option<u32>,
    pub dirs: Vec<u32>,
    pub files: Vec<u32>,
    pub agg: Agg,
}

pub struct SampleIndex {
    pub entries: Vec<SampleEntry>,
    pub used_by: HashMap<u32, Vec<u32>>,
    pub project_names: Vec<String>,
    pub roots: Vec<u32>,
    pub nodes: Vec<DirNode>,
    pub node_by_key: HashMap<String, u32>,
}

pub struct Analysis {
    pub report: Report,
    pub samples: SampleIndex,
    /// Project files and their modification times at scan time.
    pub stamps: Vec<(PathBuf, u64)>,
}

fn slash(p: &str) -> String { p.strip_prefix("\\\\?\\").unwrap_or(p).replace('\\', "/") }

pub fn run(s: &Settings, progress: &(dyn Fn(Progress) + Sync)) -> Analysis {
    let mut warnings = Vec::new();
    let mut unreadable: Vec<String> = Vec::new();
    let mut unreadable_dirs: Vec<String> = Vec::new();
    let mut tm = Timings::default();

    // 1. installed plugins
    let t = Instant::now();
    progress(Progress { stage: "plugins", done: 0, total: 0 });
    let mut dirs: Vec<PathBuf> = plugins::default_dirs().into_iter().map(|(_, p)| p).collect();
    dirs.extend(s.extra_plugin_dirs.iter().cloned());
    let installed = plugins::scan_dirs(&dirs);
    let families = plugins::group_families(installed);
    tm.plugins_ms = t.elapsed().as_millis();
    let images = plugins::find_images(&families);
    let names: Vec<String> = families.iter().map(|f| f.display.clone()).collect();
    let fp = cache::fingerprint(&names);
    let matcher = NameMatcher::new(&names);

    // 2. samples (metadata only). Ableton presets found here are read like projects.
    let t = Instant::now();
    let counter = AtomicU64::new(0);
    let scan_out: samples::ScanOut = std::thread::scope(|sc| {
        let h = sc.spawn(|| samples::scan(&s.sample_dirs, Some(&counter)));
        while !h.is_finished() {
            progress(Progress { stage: "samples", done: counter.load(Ordering::Relaxed), total: 0 });
            std::thread::sleep(std::time::Duration::from_millis(120));
        }
        h.join().unwrap_or_default()
    });
    let sample_files: Vec<SampleFile> = scan_out.files;
    tm.samples_ms = t.elapsed().as_millis();

    // 3. projects (parallel, cached)
    let t = Instant::now();
    progress(Progress { stage: "projects", done: 0, total: 0 });
    let mut files = projects::discover(&s.project_dirs, &s.daws, s.include_backups, &s.library_dirs);
    if s.daws.contains(&Daw::Ableton) {
        let known: HashSet<PathBuf> = files.iter().map(|f| f.path.clone()).collect();
        for p in scan_out.presets {
            let path = PathBuf::from(&p.path);
            if known.contains(&path) { continue; }
            let mut pf = projects::make(Daw::Ableton, path, None, true);
            pf.size = p.size; pf.mtime = p.mtime;
            files.push(pf);
        }
    }
    let stamps: Vec<(PathBuf, u64)> = files.iter().filter(|f| !f.is_library).map(|f| (f.path.clone(), f.mtime)).collect();
    let mut cache = s.cache_path.as_deref().map(Cache::load).unwrap_or_default();
    let done = AtomicU64::new(0);
    let total = files.len() as u64;
    let results: Vec<(ProjectScan, bool, Option<String>)> = files.par_iter().map(|pf| {
        let key = pf.path.to_string_lossy().into_owned();
        let use_fp = if pf.daw.support() == SupportLevel::Heuristic { fp } else { 0 };
        let r = if let Some(c) = cache.get(&key, pf.mtime, pf.size, use_fp) { (c.clone(), true, None) } else {
            // A broken or unusual project must never stop the whole scan.
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| projects::parse(pf, &matcher))) {
                Ok(Ok(sc)) => (sc, false, None),
                Ok(Err(e)) => (ProjectScan::default(), false, Some(format!("{}: {e}", pf.path.display()))),
                Err(_) => (ProjectScan::default(), false, Some(format!("{}: внутренняя ошибка разбора", pf.path.display()))),
            }
        };
        let d = done.fetch_add(1, Ordering::Relaxed) + 1;
        if d % 8 == 0 || d == total { progress(Progress { stage: "projects", done: d, total }); }
        r
    }).collect();
    let mut scans: Vec<(&ProjectFile, ProjectScan)> = Vec::new();
    for (pf, (sc, cached, err)) in files.iter().zip(results) {
        if cached { tm.cached_projects += 1 } else { tm.parsed_projects += 1 }
        if let Some(e) = err { unreadable.push(e); unreadable_dirs.push(pf.path.parent().map(|d| slash(&d.to_string_lossy()).to_lowercase()).unwrap_or_default()); }
        else if !cached {
            let use_fp = if pf.daw.support() == SupportLevel::Heuristic { fp } else { 0 };
            cache.entries.insert(pf.path.to_string_lossy().into_owned(), cache::Entry { mtime: pf.mtime, size: pf.size, names_fp: use_fp, scan: sc.clone() });
        }
        scans.push((pf, sc));
    }
    if let Some(cp) = &s.cache_path { cache.save(cp); }
    tm.projects_ms = t.elapsed().as_millis();


    // 4. analysis
    let t = Instant::now();
    progress(Progress { stage: "analysis", done: 0, total: 0 });
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let old_cut = now.saturating_sub(s.old_days as u64 * 86400);
    let fam_by_key: HashMap<String, usize> = families.iter().enumerate().map(|(i, f)| (family_key(&f.display), i)).collect();

    // per family, per format: exact users and heuristic users
    #[derive(Default, Clone)]
    struct Use { exact: Vec<UsedBy>, unknown: Vec<UsedBy> }
    let mut uses: Vec<HashMap<PluginFormat, Use>> = vec![HashMap::new(); families.len()];
    let mut fam_users: Vec<BTreeSet<String>> = vec![BTreeSet::new(); families.len()];
    let mut project_reports: Vec<ProjectReport> = Vec::new();
    let mut proj_names: Vec<String> = Vec::new();
    let mut proj_is_old: Vec<bool> = Vec::new();
    let mut proj_dirs_lower: HashSet<String> = HashSet::new();
    // Folders of files we could not read at all: keep everything in them.
    for d in &unreadable_dirs { proj_dirs_lower.insert(d.clone()); }

    // Index samples by lowercase path first (used for missing-file checks and reference matching).
    let mut entries: Vec<SampleEntry> = sample_files.into_iter().map(|f| SampleEntry { slash: slash(&f.path), size: f.size, mtime: f.mtime, state: UsageState::Unused, by_name: false, gone: false, node: 0 }).collect();
    let mut lookup: HashMap<String, u32> = entries.iter().enumerate().map(|(i, e)| (e.slash.to_lowercase(), i as u32)).collect();
    let mut refs: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut unresolved: HashMap<String, Vec<u32>> = HashMap::new();
    let mut all_names: HashMap<String, Vec<u32>> = HashMap::new();

    for (pf, sc) in &scans {
        let old = !pf.is_library && pf.mtime < old_cut;
        let pid = proj_names.len() as u32;
        let is_shown = !pf.is_library;
        // library presets are folded into one pseudo project name
        let pname = if pf.is_library { "Ableton User Library".to_string() } else { pf.name.clone() };
        let dir = pf.path.parent().map(|d| slash(&d.to_string_lossy())).unwrap_or_default();
        if !pf.is_library { proj_dirs_lower.insert(dir.to_lowercase()); }

        let mut plist = Vec::new();
        for pr in &sc.plugins {
            let k = family_key(&pr.name);
            let mut state = "not_installed";
            if let Some(&fi) = fam_by_key.get(&k) {
                fam_users[fi].insert(pname.clone());
                match pr.format {
                    Some(fmt) => {
                        uses[fi].entry(fmt).or_default().exact.push(UsedBy { project: pname.clone(), daw: pf.daw, exact: true });
                        state = if families[fi].plugins.iter().any(|p| p.format == fmt) { "ok" } else { "format_missing" };
                    }
                    None => {
                        for &fmt in pf.daw.formats() { uses[fi].entry(fmt).or_default().unknown.push(UsedBy { project: pname.clone(), daw: pf.daw, exact: false }); }
                        state = "ok";
                    }
                }
            }
            plist.push(ProjectPlugin { name: pr.name.clone(), format: pr.format, state });
        }

        // file references
        let mut missing = Vec::new();
        for fr in &sc.files {
            let mut hit = false;
            let abs = (!fr.path.is_empty()).then(|| slash(&fr.path));
            let rel = fr.relative.as_ref().map(|r| format!("{}/{}", dir, slash(r)));
            for cand in abs.iter().chain(rel.iter()) {
                if let Some(&idx) = lookup.get(&cand.to_lowercase()) { refs.entry(idx).or_default().push(pid); hit = true; }
            }
            if s.strict_names {
                for cand in abs.iter().chain(rel.iter()) {
                    if let Some(name) = cand.rsplit('/').next() { if !name.is_empty() { all_names.entry(name.to_lowercase()).or_default().push(pid); } }
                }
            }
            if !hit {
                let exists = abs.iter().chain(rel.iter()).any(|c| Path::new(c).exists());
                if !exists {
                    let shown = abs.clone().or(rel.clone()).unwrap_or_default();
                    if let Some(name) = shown.rsplit('/').next() { unresolved.entry(name.to_lowercase()).or_default().push(pid); }
                    missing.push(shown);
                }
            }
        }
        proj_names.push(pname.clone());
        proj_is_old.push(old);
        if is_shown {
            plist.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            project_reports.push(ProjectReport { id: project_reports.len(), name: pf.name.clone(), daw: pf.daw, path: pf.path.to_string_lossy().into_owned(),
                mtime: pf.mtime, size: pf.size, plugins: plist, file_refs: sc.files.len(), missing, old });
        }
    }

    // sample states
    let mut by_name_refs: HashMap<u32, Vec<u32>> = HashMap::new();
    if s.strict_names { for (k, v) in all_names { unresolved.entry(k).or_default().extend(v); } }
    if !unresolved.is_empty() {
        for (i, e) in entries.iter().enumerate() {
            let i = i as u32;
            if refs.contains_key(&i) { continue; }
            if let Some(name) = e.slash.rsplit('/').next() {
                if let Some(ps) = unresolved.get(&name.to_lowercase()) { by_name_refs.insert(i, ps.clone()); }
            }
        }
    }
    let mut summary = SampleSummary::default();
    for (i, e) in entries.iter_mut().enumerate() {
        let i = i as u32;
        let direct = refs.get(&i);
        let named = by_name_refs.get(&i);
        let all: Vec<u32> = direct.into_iter().chain(named).flatten().copied().collect();
        e.by_name = direct.is_none() && named.is_some();
        let any_recent = all.iter().any(|&p| !proj_is_old[p as usize]);
        let lower = e.slash.to_lowercase();
        let owned = ancestors(&lower).any(|a| proj_dirs_lower.contains(a));
        e.state = if any_recent { UsageState::Used } else if owned { UsageState::ProjectOwned } else if !all.is_empty() { UsageState::OldOnly } else { UsageState::Unused };
        summary.files += 1; summary.bytes += e.size;
        match e.state {
            UsageState::Used => summary.used_bytes += e.size,
            UsageState::OldOnly => summary.old_bytes += e.size,
            UsageState::ProjectOwned => summary.owned_bytes += e.size,
            UsageState::Unused => { summary.unused_bytes += e.size; summary.unused_files += 1 }
        }
    }
    let mut used_by = refs;
    for (k, v) in by_name_refs { used_by.entry(k).or_default().extend(v); }
    for v in used_by.values_mut() { v.sort_unstable(); v.dedup(); }
    drop(std::mem::take(&mut lookup));

    // plugin verdicts
    let unsupported: Vec<Daw> = s.daws.iter().copied().filter(|d| d.support() == SupportLevel::None).collect();
    let mut plugin_reports = Vec::new();
    for (fi, fam) in families.iter().enumerate() {
        let mut fmts = Vec::new();
        let installed_formats: Vec<PluginFormat> = fam.plugins.iter().map(|p| p.format).collect();
        let any_use = !fam_users[fi].is_empty();
        for p in &fam.plugins {
            let u = uses[fi].get(&p.format).cloned().unwrap_or_default();
            let mut protected_by: Vec<Daw> = unsupported.iter().copied().filter(|d| d.formats().contains(&p.format)).collect();
            // heuristic users protect depending on policy
            let mut used_by_list = u.exact.clone();
            let best = [PluginFormat::Vst3, PluginFormat::Vst2, PluginFormat::Au, PluginFormat::Clap, PluginFormat::Aax].into_iter().find(|f| installed_formats.contains(f));
            for h in &u.unknown {
                let applies = match s.unknown_policy {
                    UnknownPolicy::Safe => true,
                    UnknownPolicy::Balanced => h.daw.formats().iter().find(|f| installed_formats.contains(f)).copied() == Some(p.format) || best == Some(p.format) && h.daw.formats().contains(&p.format),
                };
                if applies { used_by_list.push(h.clone()); }
            }
            protected_by.sort_by_key(|d| d.name()); protected_by.dedup();
            used_by_list.sort_by(|a, b| a.project.cmp(&b.project)); used_by_list.dedup_by(|a, b| a.project == b.project && a.daw == b.daw);
            let can_load = s.daws.iter().any(|d| d.formats().contains(&p.format));
            let removable = used_by_list.is_empty() && protected_by.is_empty();
            let reason = if !protected_by.is_empty() { format!("{} не читается, формат может использоваться", protected_by.iter().map(|d| d.name()).collect::<Vec<_>>().join(", ")) }
                else if !used_by_list.is_empty() { format!("Используется в проектах: {}", used_by_list.len()) }
                else if !can_load { "Ни одна из выбранных DAW не загружает этот формат".into() }
                else { "Ни один проект не использует этот формат".into() };
            fmts.push(FormatReport { format: p.format, path: p.path.clone(), size: p.size, used_by: used_by_list, protected_by, removable, reason });
        }
        fmts.sort_by_key(|f| f.format);
        // recommendation
        let mut rec = Vec::new();
        let keep_pref = any_use && fmts.iter().any(|f| f.format == s.preferred_format);
        for f in &fmts {
            if !f.removable { continue; }
            if keep_pref && f.format == s.preferred_format { continue; }
            rec.push(f.path.clone());
        }
        let status = if !any_use && fmts.iter().all(|f| f.used_by.is_empty() && f.protected_by.is_empty()) { PluginStatus::Unused }
            else if !rec.is_empty() { PluginStatus::Duplicate } else { PluginStatus::Used };
        let rsize = fmts.iter().filter(|f| rec.contains(&f.path)).map(|f| f.size).sum();
        plugin_reports.push(PluginReport { name: fam.display.clone(), used_projects: fam_users[fi].len(), status, total_size: fmts.iter().map(|f| f.size).sum(), recommended_size: rsize, recommended: rec, image: images.get(&family_key(&fam.display)).cloned(), formats: fmts });
    }
    plugin_reports.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    for d in &unsupported { warnings.push(format!("{}: проекты не читаются. Плагины форматов этой DAW не предлагаются к удалению.", d.name())); }
    for d in &s.daws {
        if d.support() == SupportLevel::Heuristic { warnings.push(format!("{}: плагины определяются по названиям внутри проекта, формат неизвестен. Защищаются все форматы, которые эта DAW умеет загружать.", d.name())); }
    }
    tm.analysis_ms = t.elapsed().as_millis();
    let root_paths: Vec<String> = s.sample_dirs.iter().map(|p| slash(&p.to_string_lossy()).trim_end_matches('/').to_string()).collect();
    let samples = SampleIndex::build(entries, used_by, proj_names, &root_paths);
    Analysis { report: Report { plugins: plugin_reports, projects: project_reports, samples: summary, timings: tm, warnings, unreadable }, samples, stamps }
}

/// Yield every parent directory of a lowercase slash path: "a/b/c.wav" -> "a/b", "a".
fn ancestors(p: &str) -> impl Iterator<Item = &str> {
    let mut end = p.len();
    std::iter::from_fn(move || {
        let idx = p[..end].rfind('/')?;
        end = idx;
        Some(&p[..idx])
    })
}

// ---------- sample queries for the UI ----------

#[derive(Debug, Clone, Serialize)]
pub struct Child {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub bytes: u64,
    pub files: u64,
    pub unused_bytes: u64,
    pub used_bytes: u64,
    pub old_bytes: u64,
    pub owned_bytes: u64,
    pub state: Option<UsageState>,
    pub idx: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Listing { pub items: Vec<Child>, pub hidden_files: u64, pub parent: Option<String> }

#[derive(Debug, Clone, Serialize)]
pub struct FileInfo { pub idx: u32, pub path: String, pub name: String, pub size: u64, pub mtime: u64, pub state: UsageState, pub by_name: bool, pub used_by: Vec<String> }

/// Max file rows sent for one folder; the rest are summarised.
pub const MAX_FILE_ROWS: usize = 3000;

impl SampleIndex {
    /// Build the folder tree once so that browsing is instant on huge libraries.
    pub fn build(mut entries: Vec<SampleEntry>, used_by: HashMap<u32, Vec<u32>>, project_names: Vec<String>, roots: &[String]) -> Self {
        let mut nodes: Vec<DirNode> = Vec::new();
        let mut node_by_key: HashMap<String, u32> = HashMap::new();
        let mut root_ids = Vec::new();
        for r in roots {
            let key = r.to_lowercase();
            if node_by_key.contains_key(&key) { continue; }
            let id = nodes.len() as u32;
            nodes.push(DirNode { path: r.clone(), name: r.rsplit('/').next().unwrap_or(r).to_string(), parent: None, dirs: vec![], files: vec![], agg: Agg::default() });
            node_by_key.insert(key, id);
            root_ids.push(id);
        }
        // longest roots first so nested roots win
        let mut roots_sorted: Vec<(String, u32)> = roots.iter().map(|r| (r.to_lowercase(), node_by_key[&r.to_lowercase()])).collect();
        roots_sorted.sort_by(|a, b| b.0.len().cmp(&a.0.len()));

        fn get_or_create(dir: &str, root_len: usize, root_id: u32, nodes: &mut Vec<DirNode>, by_key: &mut HashMap<String, u32>) -> u32 {
            if dir.len() <= root_len { return root_id; }
            let key = dir.to_lowercase();
            if let Some(&id) = by_key.get(&key) { return id; }
            let (parent_dir, name) = match dir.rfind('/') { Some(i) => (&dir[..i], &dir[i + 1..]), None => ("", dir) };
            let parent = get_or_create(parent_dir, root_len, root_id, nodes, by_key);
            let id = nodes.len() as u32;
            nodes.push(DirNode { path: dir.to_string(), name: name.to_string(), parent: Some(parent), dirs: vec![], files: vec![], agg: Agg::default() });
            nodes[parent as usize].dirs.push(id);
            by_key.insert(key, id);
            id
        }

        let mut last: Option<(String, u32)> = None;
        let mut keep = vec![true; entries.len()];
        for (i, e) in entries.iter_mut().enumerate() {
            let Some(cut) = e.slash.rfind('/') else { keep[i] = false; continue };
            let dir = &e.slash[..cut];
            let node = match &last {
                Some((d, n)) if d == dir => *n,
                _ => {
                    let lower = dir.to_lowercase();
                    let Some((rl, rid)) = roots_sorted.iter().find(|(r, _)| lower == *r || (lower.len() > r.len() && lower.starts_with(r.as_str()) && lower.as_bytes()[r.len()] == b'/')).map(|(r, id)| (r.len(), *id))
                        else { keep[i] = false; continue };
                    let n = get_or_create(dir, rl, rid, &mut nodes, &mut node_by_key);
                    last = Some((dir.to_string(), n));
                    n
                }
            };
            e.node = node;
            nodes[node as usize].files.push(i as u32);
        }
        for (i, e) in entries.iter().enumerate() {
            if !keep[i] { continue; }
            let mut n = Some(e.node);
            while let Some(id) = n { nodes[id as usize].agg.apply(e, 1); n = nodes[id as usize].parent; }
        }
        Self { entries, used_by, project_names, roots: root_ids, nodes, node_by_key }
    }

    pub fn fs_path(&self, idx: u32) -> PathBuf {
        let s = &self.entries[idx as usize].slash;
        if cfg!(windows) { PathBuf::from(s.replace('/', "\\")) } else { PathBuf::from(s) }
    }

    fn dir_child(&self, id: u32) -> Child {
        let n = &self.nodes[id as usize];
        Child { name: n.name.clone(), path: n.path.clone(), is_dir: true, bytes: n.agg.bytes, files: n.agg.files, unused_bytes: n.agg.unused_bytes,
            used_bytes: n.agg.used_bytes, old_bytes: n.agg.old_bytes, owned_bytes: n.agg.owned_bytes, state: None, idx: None }
    }

    /// Children of a folder. `None` lists the library roots.
    pub fn children(&self, dir: Option<&str>) -> Listing {
        let mut items: Vec<Child>;
        let mut hidden = 0;
        let mut parent = None;
        match dir {
            None => items = self.roots.iter().map(|&r| { let mut c = self.dir_child(r); c.name = c.path.clone(); c }).filter(|c| c.files > 0).collect(),
            Some(d) => {
                let key = slash(d).trim_end_matches('/').to_lowercase();
                let Some(&id) = self.node_by_key.get(&key) else { return Listing { items: vec![], hidden_files: 0, parent: None } };
                let n = &self.nodes[id as usize];
                parent = n.parent.map(|p| self.nodes[p as usize].path.clone());
                items = n.dirs.iter().map(|&c| self.dir_child(c)).filter(|c| c.files > 0).collect();
                let mut files: Vec<u32> = n.files.iter().copied().filter(|&i| !self.entries[i as usize].gone).collect();
                files.sort_by(|&a, &b| self.entries[b as usize].size.cmp(&self.entries[a as usize].size));
                if files.len() > MAX_FILE_ROWS { hidden = (files.len() - MAX_FILE_ROWS) as u64; files.truncate(MAX_FILE_ROWS); }
                items.sort_by(|a, b| b.bytes.cmp(&a.bytes));
                for i in files {
                    let e = &self.entries[i as usize];
                    let mut c = Child { name: e.slash.rsplit('/').next().unwrap_or("").to_string(), path: e.slash.clone(), is_dir: false, bytes: 0, files: 0,
                        unused_bytes: 0, used_bytes: 0, old_bytes: 0, owned_bytes: 0, state: Some(e.state), idx: Some(i) };
                    let mut a = Agg::default(); a.apply(e, 1);
                    c.bytes = a.bytes; c.files = 1; c.unused_bytes = a.unused_bytes; c.used_bytes = a.used_bytes; c.old_bytes = a.old_bytes; c.owned_bytes = a.owned_bytes;
                    items.push(c);
                }
                return Listing { items, hidden_files: hidden, parent };
            }
        }
        items.sort_by(|a, b| b.bytes.cmp(&a.bytes));
        Listing { items, hidden_files: hidden, parent }
    }

    /// Mark a file as moved to / restored from the trash; folder totals follow.
    pub fn set_gone_by_path(&mut self, fs_path: &str, gone: bool) {
        let want = slash(fs_path);
        let Some(i) = self.entries.iter().position(|e| e.slash.eq_ignore_ascii_case(&want)) else { return };
        if self.entries[i].gone == gone { return; }
        self.entries[i].gone = gone;
        let sign = if gone { -1 } else { 1 };
        let mut n = Some(self.entries[i].node);
        while let Some(id) = n {
            let e = &self.entries[i];
            self.nodes[id as usize].agg.apply(e, sign);
            n = self.nodes[id as usize].parent;
        }
    }

    pub fn summary(&self) -> SampleSummary {
        let mut s = SampleSummary::default();
        for e in self.entries.iter().filter(|e| !e.gone) {
            s.files += 1; s.bytes += e.size;
            match e.state {
                UsageState::Used => s.used_bytes += e.size,
                UsageState::OldOnly => s.old_bytes += e.size,
                UsageState::ProjectOwned => s.owned_bytes += e.size,
                UsageState::Unused => { s.unused_bytes += e.size; s.unused_files += 1 }
            }
        }
        s
    }

    pub fn file_info(&self, idx: u32) -> FileInfo {
        let e = &self.entries[idx as usize];
        let used_by = self.used_by.get(&idx).map(|v| v.iter().map(|&p| self.project_names[p as usize].clone()).collect()).unwrap_or_default();
        FileInfo { idx, path: e.slash.clone(), name: e.slash.rsplit('/').next().unwrap_or("").to_string(), size: e.size, mtime: e.mtime, state: e.state, by_name: e.by_name, used_by }
    }

    /// Largest files with a given state, optionally under a directory.
    pub fn top_files(&self, state: UsageState, under: Option<&str>, limit: usize) -> Vec<FileInfo> {
        let pre = under.map(|d| format!("{}/", slash(d).trim_end_matches('/')));
        let mut idx: Vec<u32> = self.entries.iter().enumerate().filter(|(_, e)| !e.gone && e.state == state
            && pre.as_ref().map_or(true, |p| e.slash.len() > p.len() && e.slash.as_bytes()[..p.len()].eq_ignore_ascii_case(p.as_bytes()))).map(|(i, _)| i as u32).collect();
        idx.sort_by(|&a, &b| self.entries[b as usize].size.cmp(&self.entries[a as usize].size));
        idx.truncate(limit);
        idx.into_iter().map(|i| self.file_info(i)).collect()
    }
}

/// Project files that appeared, disappeared or changed since the scan.
/// Removing files is refused while this returns something.
pub fn projects_changed(s: &Settings, stamps: &[(PathBuf, u64)]) -> Option<String> {
    let now = projects::discover(&s.project_dirs, &s.daws, s.include_backups, &[]);
    let before: HashMap<&Path, u64> = stamps.iter().map(|(p, m)| (p.as_path(), *m)).collect();
    for f in &now {
        match before.get(f.path.as_path()) {
            None => return Some(format!("Появился новый проект: {}", f.path.display())),
            Some(&m) if m != f.mtime => return Some(format!("Проект изменён после скана: {}", f.path.display())),
            _ => {}
        }
    }
    if now.len() < stamps.len() { return Some("Часть проектов удалена или перемещена после скана".into()); }
    None
}
