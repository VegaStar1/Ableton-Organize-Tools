//! Exact duplicate finder: group by size, then by a cheap head/tail hash,
//! then confirm with a full BLAKE3 hash. Only candidates are read.
use crate::analysis::SampleIndex;
use rayon::prelude::*;
use serde::Serialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

#[derive(Debug, Clone, Serialize)]
pub struct DupGroup { pub size: u64, pub files: Vec<u32> }

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

fn full_hash(path: &std::path::Path) -> Option<[u8; 32]> {
    let mut f = File::open(path).ok()?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 { break; }
        h.update(&buf[..n]);
    }
    Some(*h.finalize().as_bytes())
}

pub fn find(index: &SampleIndex, min_size: u64) -> Vec<DupGroup> {
    let mut by_size: HashMap<u64, Vec<u32>> = HashMap::new();
    for (i, e) in index.entries.iter().enumerate() {
        if e.size >= min_size { by_size.entry(e.size).or_default().push(i as u32); }
    }
    let cands: Vec<(u64, Vec<u32>)> = by_size.into_iter().filter(|(_, v)| v.len() > 1).collect();
    let mut out: Vec<DupGroup> = cands.par_iter().flat_map(|(size, ids)| {
        let mut quick: HashMap<[u8; 32], Vec<u32>> = HashMap::new();
        for &i in ids { if let Some(h) = quick_hash(&index.fs_path(i), *size) { quick.entry(h).or_default().push(i); } }
        let mut groups = Vec::new();
        for (_, g) in quick.into_iter().filter(|(_, g)| g.len() > 1) {
            let mut full: HashMap<[u8; 32], Vec<u32>> = HashMap::new();
            for i in g { if let Some(h) = full_hash(&index.fs_path(i)) { full.entry(h).or_default().push(i); } }
            for (_, f) in full.into_iter().filter(|(_, f)| f.len() > 1) { groups.push(DupGroup { size: *size, files: f }); }
        }
        groups
    }).collect();
    out.sort_by(|a, b| (b.size * b.files.len() as u64).cmp(&(a.size * a.files.len() as u64)));
    out
}
