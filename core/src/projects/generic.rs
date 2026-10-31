//! Heuristic reader for DAWs whose project format is not parsed structurally
//! (Studio One, FL Studio, Pro Tools, Bitwig, Cubase, REAPER).
//!
//! It looks for the names of *installed* plugins and for audio file paths inside
//! the raw bytes (ASCII and UTF-16LE). False positives only make the app keep
//! more files, never fewer, which is the safe direction.
use crate::daw::Daw;
use crate::plugins::family_key;
use crate::{FileRefEntry, PluginRef, ProjectScan};
use aho_corasick::{AhoCorasick, MatchKind};
use std::collections::BTreeSet;
use std::io::Read;
use std::path::Path;

pub const AUDIO_EXTS: &[&str] = &["wav", "aif", "aiff", "flac", "mp3", "ogg", "wv", "m4a", "w64", "rex", "rx2", "caf"];

pub struct NameMatcher {
    ascii: AhoCorasick,
    utf16: AhoCorasick,
    names: Vec<String>, // display names, parallel to patterns
}

impl NameMatcher {
    /// `names` are display names of installed plugin families.
    pub fn new(names: &[String]) -> Self {
        let names: Vec<String> = names.iter().filter(|n| n.chars().count() >= 3).cloned().collect();
        let ascii: Vec<Vec<u8>> = names.iter().map(|n| n.to_ascii_lowercase().into_bytes()).collect();
        let utf16: Vec<Vec<u8>> = names.iter().map(|n| n.to_ascii_lowercase().encode_utf16().flat_map(|u| u.to_le_bytes()).collect()).collect();
        let build = |p: Vec<Vec<u8>>| AhoCorasick::builder().match_kind(MatchKind::Standard).build(p).expect("automaton");
        Self { ascii: build(ascii), utf16: build(utf16), names }
    }

    /// Names of plugins found in `data` (lowercasing is done on a copy).
    pub fn find(&self, data: &[u8], out: &mut BTreeSet<String>) {
        if self.names.is_empty() { return; }
        let lower = data.to_ascii_lowercase();
        for m in self.ascii.find_overlapping_iter(&lower) {
            let (s, e) = (m.start(), m.end());
            let before = if s > 0 { Some(lower[s - 1]) } else { None };
            let after = lower.get(e).copied();
            if boundary(before) && boundary(after) { out.insert(self.names[m.pattern().as_usize()].clone()); }
        }
        for m in self.utf16.find_overlapping_iter(&lower) {
            let (s, e) = (m.start(), m.end());
            if s % 2 != 0 { continue; }
            let before = if s >= 2 { Some(lower[s - 2]) } else { None };
            let after = lower.get(e).copied();
            if boundary(before) && boundary(after) { out.insert(self.names[m.pattern().as_usize()].clone()); }
        }
    }
}

fn boundary(b: Option<u8>) -> bool { b.map_or(true, |c| !(c.is_ascii_alphanumeric())) }

pub fn parse(path: &Path, daw: Daw, matcher: &NameMatcher) -> std::io::Result<ProjectScan> {
    let mut names = BTreeSet::new();
    let mut files = BTreeSet::new();
    let mut feed = |data: &[u8]| {
        matcher.find(data, &mut names);
        for p in extract_audio_paths(data) { files.insert(p); }
    };
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
    if ext == "song" && is_zip(path) {
        let mut z = zip::ZipArchive::new(std::fs::File::open(path)?).map_err(to_io)?;
        for i in 0..z.len() {
            let mut e = z.by_index(i).map_err(to_io)?;
            if e.is_dir() || e.size() > 512 << 20 { continue; }
            let mut buf = Vec::with_capacity(e.size() as usize);
            e.read_to_end(&mut buf)?;
            feed(&buf);
        }
    } else {
        // Stream large binaries in chunks with overlap so names on the seam are not lost.
        let mut f = std::fs::File::open(path)?;
        let chunk = 8 << 20;
        let overlap = 1024;
        let mut buf = vec![0u8; chunk + overlap];
        let mut have = 0usize;
        loop {
            let n = f.read(&mut buf[have..])?;
            if n == 0 { if have > 0 { feed(&buf[..have]); } break; }
            have += n;
            if have == buf.len() {
                feed(&buf[..have]);
                buf.copy_within(have - overlap..have, 0);
                have = overlap;
            }
        }
    }
    let _ = daw;
    Ok(ProjectScan {
        plugins: names.into_iter().map(|name| PluginRef { name, format: None }).collect(),
        files: files.into_iter().map(|path| FileRefEntry { path, relative: None }).collect(),
    })
}

fn is_zip(p: &Path) -> bool {
    let mut m = [0u8; 4];
    std::fs::File::open(p).and_then(|mut f| f.read_exact(&mut m)).is_ok() && &m[..2] == b"PK"
}
fn to_io<E: std::fmt::Display>(e: E) -> std::io::Error { std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()) }

/// Find strings that end in an audio extension and look like paths.
/// Handles ASCII, UTF-16LE and `file:///` URLs with percent-encoding.
pub fn extract_audio_paths(data: &[u8]) -> Vec<String> {
    let mut out = BTreeSet::new();
    scan_ascii(data, &mut out);
    // UTF-16LE: pull out the two byte lanes that hold ASCII-range text.
    if data.len() > 8 {
        for lane in 0..2 {
            let mut narrowed = Vec::with_capacity(data.len() / 2);
            let mut i = lane;
            while i + 1 < data.len() {
                // keep chars whose high byte is zero; replace others with NUL
                narrowed.push(if data[i + 1] == 0 { data[i] } else { 0 });
                i += 2;
            }
            // Non-ASCII file names are lost here; they are still covered by file-name matching in analysis.
            scan_ascii(&narrowed, &mut out);
        }
    }
    out.into_iter().collect()
}

fn scan_ascii(data: &[u8], out: &mut BTreeSet<String>) {
    // Find ".ext" followed by a non-alphanumeric byte, then walk back to a delimiter.
    let mut i = 0;
    while i < data.len() {
        if data[i] == b'.' {
            for ext in AUDIO_EXTS {
                let end = i + 1 + ext.len();
                if end <= data.len() && data[i + 1..end].eq_ignore_ascii_case(ext.as_bytes())
                    && data.get(end).map_or(true, |c| !c.is_ascii_alphanumeric()) {
                    let mut s = i;
                    while s > 0 && i - s < 520 && !is_delim(data[s - 1]) { s -= 1; }
                    if let Ok(txt) = std::str::from_utf8(&data[s..end]) {
                        if let Some(p) = looks_like_path(txt) { out.insert(p); }
                    }
                    break;
                }
            }
        }
        i += 1;
    }
}

fn is_delim(c: u8) -> bool { c < 0x20 || matches!(c, b'"' | b'<' | b'>' | b'|' | b'*' | b'?' | 0x7f) }

/// Cut leading prose off a candidate: keep the text from the last `X:\`, `X:/`,
/// `file://` or `\\server` marker, else from the first `/` that follows a space.
fn trim_to_path_start(s: &str) -> &str {
    let b = s.as_bytes();
    let mut best: Option<usize> = None;
    for i in 0..b.len() {
        let drive = i + 2 < b.len() && b[i].is_ascii_alphabetic() && b[i + 1] == b':' && (b[i + 2] == b'\\' || b[i + 2] == b'/')
            && (i == 0 || !b[i - 1].is_ascii_alphanumeric());
        // Byte comparisons only: `s` may hold Cyrillic, and slicing a str mid-character panics.
        let in_url = i >= 8 && &b[i - 8..i] == b"file:///";
        if (drive && !in_url) || b[i..].starts_with(b"file://") || b[i..].starts_with(b"\\\\") { best = Some(i); }
    }
    if let Some(i) = best { return &s[i..]; }
    if b.first() == Some(&b'/') { return s; }
    for i in 1..b.len() { if b[i] == b'/' && b[i - 1] == b' ' { return &s[i..]; } }
    s
}

fn looks_like_path(s: &str) -> Option<String> {
    let s = trim_to_path_start(s.trim()).trim();
    if s.len() < 5 { return None; }
    let decoded = if let Some(rest) = s.strip_prefix("file://") {
        let rest = rest.trim_start_matches('/');
        let d = percent_encoding::percent_decode_str(rest).decode_utf8_lossy().into_owned();
        if d.chars().nth(1) == Some(':') { d } else { format!("/{d}") }
    } else { s.to_string() };
    let has_sep = decoded.contains('/') || decoded.contains('\\');
    if has_sep { Some(decoded) } else { None }
}

/// Convenience for tests and the analysis step.
pub fn key(n: &str) -> String { family_key(n) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finds_names_with_boundaries() {
        let m = NameMatcher::new(&["Serum".into(), "Pro-Q 3".into()]);
        let mut out = BTreeSet::new();
        m.find(b"xx <Name value=\"Serum\"/> and FabFilter Pro-Q 3.", &mut out);
        assert!(out.contains("Serum") && out.contains("Pro-Q 3"));
        let mut out2 = BTreeSet::new();
        m.find(b"Serumize", &mut out2);
        assert!(out2.is_empty());
    }
    #[test]
    fn finds_utf16_names() {
        let m = NameMatcher::new(&["Massive".into()]);
        let data: Vec<u8> = "..Massive..".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let mut out = BTreeSet::new();
        m.find(&data, &mut out);
        assert!(out.contains("Massive"));
    }
    #[test]
    fn extracts_paths() {
        let d = b"<Url url=\"file:///D:/Samples/Kick%20A.wav\"/> and C:\\Audio\\snare one.WAV\0";
        let p = extract_audio_paths(d);
        assert!(p.iter().any(|x| x == "D:/Samples/Kick A.wav"), "{p:?}");
        assert!(p.iter().any(|x| x.eq_ignore_ascii_case("C:\\Audio\\snare one.wav")), "{p:?}");
    }
}
#[cfg(test)]
mod cyrillic_tests {
    use super::*;
    #[test]
    fn cyrillic_paths_do_not_panic() {
        let d = "текст D:\\Сэмплы\\Бочка 01.wav и file:///D:/Мои%20звуки/snare.wav и Привет/кик.wav".as_bytes();
        let p = extract_audio_paths(d);
        assert!(p.iter().any(|x| x == "D:\\Сэмплы\\Бочка 01.wav"), "{p:?}");
        assert!(p.iter().any(|x| x == "D:/Мои звуки/snare.wav"), "{p:?}");
    }
}
