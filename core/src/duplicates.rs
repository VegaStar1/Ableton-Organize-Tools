//! Exact duplicate finder: group by size, then by a cheap head/tail hash,
//! then confirm with a full BLAKE3 hash. Only candidates are read.
//! Reports progress and can be cancelled.
use crate::analysis::SampleIndex;
use rayon::prelude::*;
use serde::Serialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[derive(Debug, Clone, Serialize)]
pub struct DupGroup { pub size: u64, pub files: Vec<u32> }

#[derive(Debug, Clone, Serialize)]
pub struct DupProgress {
    /// "quick" = reading file edges, "full" = reading whole candidates
    pub stage: &'static str,
    pub done_files: u64,
    pub total_files: u64,
    pub done_bytes: u64,
    pub total_bytes: u64,
}

const EDGE: u64 = 64 * 1024;

fn quick_hash(path: &std::path::Path, size: u64) -> Option<[u8; 32]> {
    let mut f = File::open(path).ok()?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; EDGE.min(size) as usize];
    f.read_exact(&mut buf).ok()?;
    h.update(&buf);
    if size > EDGE {
        f.seek(SeekFrom::Start(size.saturating_sub(EDGE))).ok()?;
        f.read_exact(&mut buf).ok()?;
        h.update(&buf);
    }
    Some(*h.finalize().as_bytes())
}

fn full_hash(path: &std::path::Path, bytes: &AtomicU64, cancel: &AtomicBool) -> Option<[u8; 32]> {
    let mut f = File::open(path).ok()?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        if cancel.load(Ordering::Relaxed) { return None; }
        let n = f.read(&mut buf).ok()?;
        if n == 0 { break; }
        h.update(&buf[..n]);
        bytes.fetch_add(n as u64, Ordering::Relaxed);
    }
    Some(*h.finalize().as_bytes())
}

/// Returns `None` when cancelled.
pub fn find(index: &SampleIndex, min_size: u64, progress: &(dyn Fn(DupProgress) + Sync), cancel: &AtomicBool) -> Option<Vec<DupGroup>> {
    let mut by_size: HashMap<u64, Vec<u32>> = HashMap::new();
    for (i, e) in index.entries.iter().enumerate() {
        if !e.gone && e.size >= min_size { by_size.entry(e.size).or_default().push(i as u32); }
    }
    let cands: Vec<u32> = by_size.into_values().filter(|v| v.len() > 1).flatten().collect();
    let total_files = cands.len() as u64;
    let done = AtomicU64::new(0);
    let tick = |stage, d: u64, tf, db, tb| progress(DupProgress { stage, done_files: d, total_files: tf, done_bytes: db, total_bytes: tb });
    tick("quick", 0, total_files, 0, 0);

    // Phase 1: head + tail of every candidate.
    let quick: Vec<(u32, Option<[u8; 32]>)> = cands.par_iter().map(|&i| {
        if cancel.load(Ordering::Relaxed) { return (i, None); }
        let h = quick_hash(&index.fs_path(i), index.entries[i as usize].size);
        let d = done.fetch_add(1, Ordering::Relaxed) + 1;
        if d % 64 == 0 || d == total_files { tick("quick", d, total_files, 0, 0); }
        (i, h)
    }).collect();
    if cancel.load(Ordering::Relaxed) { return None; }
    let mut groups: HashMap<(u64, [u8; 32]), Vec<u32>> = HashMap::new();
    for (i, h) in quick { if let Some(h) = h { groups.entry((index.entries[i as usize].size, h)).or_default().push(i); } }
    let second: Vec<u32> = groups.into_values().filter(|g| g.len() > 1).flatten().collect();

    // Phase 2: full content of files that still look identical.
    let total_bytes: u64 = second.iter().map(|&i| index.entries[i as usize].size).sum();
    let bytes = AtomicU64::new(0);
    let done2 = AtomicU64::new(0);
    let n2 = second.len() as u64;
    let full: Vec<(u32, Option<[u8; 32]>)> = std::thread::scope(|sc| {
        let worker = sc.spawn(|| second.par_iter().map(|&i| {
            let h = full_hash(&index.fs_path(i), &bytes, cancel);
            done2.fetch_add(1, Ordering::Relaxed);
            (i, h)
        }).collect::<Vec<_>>());
        while !worker.is_finished() {
            tick("full", done2.load(Ordering::Relaxed), n2, bytes.load(Ordering::Relaxed), total_bytes);
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
        worker.join().unwrap_or_default()
    });
    if cancel.load(Ordering::Relaxed) { return None; }
    tick("full", n2, n2, total_bytes, total_bytes);
    let mut fg: HashMap<(u64, [u8; 32]), Vec<u32>> = HashMap::new();
    for (i, h) in full { if let Some(h) = h { fg.entry((index.entries[i as usize].size, h)).or_default().push(i); } }
    let mut out: Vec<DupGroup> = fg.into_iter().filter(|(_, v)| v.len() > 1).map(|((size, _), files)| DupGroup { size, files }).collect();
    out.sort_by(|a, b| (b.size * b.files.len() as u64).cmp(&(a.size * a.files.len() as u64)));
    Some(out)
}
