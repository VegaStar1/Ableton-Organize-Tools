//! On-disk cache of parsed projects so repeat scans only touch changed files.
use crate::ProjectScan;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Bump whenever parsing changes so old results are re-read.
const VERSION: u32 = 2;

#[derive(Serialize, Deserialize, Clone)]
pub struct Entry {
    pub mtime: u64,
    pub size: u64,
    /// Fingerprint of the installed-plugin name list (heuristic readers depend on it).
    pub names_fp: u64,
    pub scan: ProjectScan,
}

#[derive(Serialize, Deserialize, Default)]
pub struct Cache {
    version: u32,
    pub entries: HashMap<String, Entry>,
}

impl Cache {
    pub fn load(path: &Path) -> Self {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice::<Cache>(&b).ok())
            .filter(|c| c.version == VERSION).unwrap_or(Cache { version: VERSION, entries: HashMap::new() })
    }
    pub fn save(&self, path: &Path) {
        if let Some(p) = path.parent() { let _ = std::fs::create_dir_all(p); }
        if let Ok(b) = serde_json::to_vec(self) {
            let tmp = path.with_extension("tmp");
            if std::fs::write(&tmp, b).is_ok() { let _ = std::fs::rename(tmp, path); }
        }
    }
    pub fn get(&self, key: &str, mtime: u64, size: u64, fp: u64) -> Option<&ProjectScan> {
        self.entries.get(key).filter(|e| e.mtime == mtime && e.size == size && e.names_fp == fp).map(|e| &e.scan)
    }
}

pub fn fingerprint(names: &[String]) -> u64 {
    let mut v: Vec<&String> = names.iter().collect();
    v.sort();
    v.iter().flat_map(|s| s.bytes().chain(std::iter::once(0))).fold(1469598103934665603u64, |h, b| (h ^ b as u64).wrapping_mul(1099511628211))
}
