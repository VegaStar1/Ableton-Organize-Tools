//! Fast parallel metadata walk of sample libraries (no file contents are read).
use jwalk::WalkDir;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const SAMPLE_EXTS: &[&str] = &["wav", "aif", "aiff", "flac", "mp3", "ogg", "m4a", "w64", "rex", "rx2", "caf", "wv", "mid", "fxp", "nksn"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SampleFile {
    pub path: String,
    pub size: u64,
    pub mtime: u64,
}

/// Lowercased, forward-slash path used to compare references to files on disk.
pub fn norm(p: &str) -> String {
    let p = p.strip_prefix("\\\\?\\").unwrap_or(p);
    p.replace('\\', "/").trim_end_matches('/').to_lowercase()
}

pub fn scan(roots: &[PathBuf], counter: Option<&AtomicU64>) -> Vec<SampleFile> {
    let mut out = Vec::new();
    for root in roots {
        for e in WalkDir::new(root).skip_hidden(false).follow_links(false).into_iter().flatten() {
            if !e.file_type().is_file() { continue; }
            // Skip our own trash folders.
            let name = e.file_name().to_string_lossy();
            if let Some(ext) = Path::new(&*name).extension().and_then(|s| s.to_str()) {
                if !SAMPLE_EXTS.iter().any(|x| x.eq_ignore_ascii_case(ext)) { continue; }
            } else { continue; }
            let p = e.path();
            if p.components().any(|c| c.as_os_str() == crate::trash::TRASH_DIR_NAME) { continue; }
            let Ok(md) = e.metadata() else { continue };
            let mtime = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
            out.push(SampleFile { path: p.to_string_lossy().into_owned(), size: md.len(), mtime });
            if let Some(c) = counter { c.fetch_add(1, Ordering::Relaxed); }
        }
    }
    out
}
