pub mod ableton;
pub mod generic;

use crate::daw::{Daw, SupportLevel};
use crate::ProjectScan;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectFile {
    pub daw: Daw,
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    pub mtime: u64,
    /// True for Ableton User Library presets (racks, clips): they count as usage
    /// for plugins but are not shown as projects.
    pub is_library: bool,
}

const LIB_EXTS: &[&str] = &["adg", "adv", "alc"];

/// Walk the given folders and collect project files for the chosen DAWs.
pub fn discover(roots: &[PathBuf], daws: &[Daw], include_backups: bool, library_dirs: &[PathBuf]) -> Vec<ProjectFile> {
    let mut out = Vec::new();
    let want = |ext: &str| daws.iter().copied().find(|d| d.project_exts().contains(&ext));
    for root in roots {
        let walker = jwalk::WalkDir::new(root).skip_hidden(false).process_read_dir(move |_, _, _, children| {
            if !include_backups {
                children.retain(|c| c.as_ref().map(|e| !(e.file_type().is_dir() && e.file_name().to_string_lossy().eq_ignore_ascii_case("Backup"))).unwrap_or(true));
            }
        });
        for e in walker.into_iter().flatten() {
            if !e.file_type().is_file() { continue; }
            let p = e.path();
            let Some(ext) = p.extension().and_then(|s| s.to_str()).map(|s| s.to_ascii_lowercase()) else { continue };
            if let Some(daw) = want(&ext) {
                out.push(make(daw, p, e.metadata().ok(), false));
            }
        }
    }
    if daws.contains(&Daw::Ableton) {
        for root in library_dirs {
            for e in jwalk::WalkDir::new(root).skip_hidden(false).into_iter().flatten() {
                if !e.file_type().is_file() { continue; }
                let p = e.path();
                let ext = p.extension().and_then(|s| s.to_str()).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
                if LIB_EXTS.contains(&ext.as_str()) { out.push(make(Daw::Ableton, p, e.metadata().ok(), true)); }
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out.dedup_by(|a, b| a.path == b.path);
    out
}

pub fn make(daw: Daw, path: PathBuf, md: Option<std::fs::Metadata>, is_library: bool) -> ProjectFile {
    let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let (size, mtime) = md.map(|m| (m.len(), m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0))).unwrap_or((0, 0));
    ProjectFile { daw, path, name, size, mtime, is_library }
}

/// Parse one project. `known_names` are lowercase family names of installed
/// plugins, used by the heuristic readers.
pub fn parse(pf: &ProjectFile, matcher: &generic::NameMatcher) -> std::io::Result<ProjectScan> {
    match pf.daw.support() {
        // Ableton files that are not well-formed XML are read as plain text instead,
        // so their sample paths still protect the samples.
        SupportLevel::Exact => {
            let data = ableton::read_decompressed(&pf.path, 1 << 30)?;
            // Structured read first; a file that is not valid XML is read as plain text.
            let mut sc = match ableton::parse_reader(&data[..]) { Ok(sc) => sc, Err(_) => generic::scan_bytes(&data, matcher) };
            // Belt and braces: any audio path written anywhere in the file also counts,
            // even if the XML reader skipped it.
            let mut known: std::collections::HashSet<String> = sc.files.iter().map(|f| f.path.to_lowercase()).collect();
            for p in generic::extract_audio_paths(&data) {
                let p = xml_unescape(&p);
                if known.insert(p.to_lowercase()) { sc.files.push(crate::FileRefEntry { path: p, relative: None }); }
            }
            Ok(sc)
        }
        SupportLevel::Heuristic => generic::parse(&pf.path, pf.daw, matcher),
        SupportLevel::None => Ok(ProjectScan::default()),
    }
}

fn xml_unescape(s: &str) -> String {
    if !s.contains('&') { return s.to_string(); }
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

pub fn project_dir(p: &Path) -> PathBuf { p.parent().map(|x| x.to_path_buf()).unwrap_or_default() }
