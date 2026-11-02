//! Extra facts about one plugin file, read on demand (not during the scan):
//! vendor, version, effect type, install date. Everything comes from the plugin
//! itself; nothing is downloaded.
use serde::Serialize;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, Serialize, Default)]
pub struct PluginDetails {
    pub path: String,
    pub vendor: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    /// Raw VST3 categories, e.g. "Fx|Reverb".
    pub categories: Vec<String>,
    /// Human-readable type in Russian, e.g. "Реверб".
    pub kind: Option<String>,
    /// "plugin" = taken from the plugin's own data, "name" = guessed from its name.
    pub kind_source: Option<String>,
    pub created: Option<u64>,
    pub modified: Option<u64>,
}

fn secs(t: std::io::Result<std::time::SystemTime>) -> Option<u64> {
    t.ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs())
}

pub fn read(path: &Path, display_name: &str) -> PluginDetails {
    let mut d = PluginDetails { path: path.to_string_lossy().into_owned(), ..Default::default() };
    if let Ok(md) = std::fs::metadata(path) { d.created = secs(md.created()); d.modified = secs(md.modified()); }

    // 1. VST3 moduleinfo.json (SDK 3.7.5+)
    let mi = path.join("Contents").join("moduleinfo.json");
    if let Ok(text) = std::fs::read_to_string(&mi) { apply_moduleinfo(&text, &mut d); }

    // 2. the binary itself: Windows version resource + VST3 category strings
    let bin = binary_of(path);
    if let Some(bin) = &bin {
        if d.vendor.is_none() || d.version.is_none() || d.description.is_none() {
            let v = pe_version_strings(bin);
            if d.vendor.is_none() { d.vendor = v.iter().find(|(k, _)| k == "CompanyName").map(|(_, v)| v.clone()); }
            if d.version.is_none() { d.version = v.iter().find(|(k, _)| k == "ProductVersion" || k == "FileVersion").map(|(_, v)| v.clone()); }
            if d.description.is_none() { d.description = v.iter().find(|(k, _)| k == "FileDescription").map(|(_, v)| v.clone()).filter(|x| !x.eq_ignore_ascii_case(display_name)); }
        }
        // Category strings inside the binary are NOT used: the VST3 SDK compiles its whole
        // table of categories ("Fx|Reverb", "Instrument|Synth"...) into every plugin.
    }
    // 3. macOS bundle Info.plist
    let plist = path.join("Contents").join("Info.plist");
    if let Ok(text) = std::fs::read_to_string(&plist) {
        if d.vendor.is_none() { d.vendor = plist_string(&text, "NSHumanReadableCopyright").map(|c| c.trim_start_matches(['©', ' ']).to_string()); }
        if d.version.is_none() { d.version = plist_string(&text, "CFBundleShortVersionString"); }
    }
    d.vendor = d.vendor.map(|v| v.trim().to_string()).filter(|v| !v.is_empty());

    // Only what the plugin declares about itself; no guessing.
    if let Some(k) = kind_from_categories(&d.categories) { d.kind = Some(k); d.kind_source = Some("plugin".into()); }
    let _ = display_name;
    d
}

fn binary_of(path: &Path) -> Option<PathBuf> {
    if path.is_file() { return Some(path.to_path_buf()); }
    for sub in ["x86_64-win", "x86-win", "arm64x-win", "MacOS"] {
        let dir = path.join("Contents").join(sub);
        if let Ok(rd) = std::fs::read_dir(&dir) {
            if let Some(f) = rd.flatten().map(|e| e.path()).find(|p| p.is_file()) { return Some(f); }
        }
    }
    None
}

fn apply_moduleinfo(text: &str, d: &mut PluginDetails) {
    // moduleinfo.json may contain JSON5 trailing commas; fall back to plain string search.
    let cleaned = strip_trailing_commas(text);
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&cleaned) {
        d.vendor = v["Factory Info"]["Vendor"].as_str().map(String::from).or(d.vendor.take());
        d.version = v["Version"].as_str().map(String::from).or(d.version.take());
        if let Some(classes) = v["Classes"].as_array() {
            for c in classes {
                if c["Category"].as_str() != Some("Audio Module Class") { continue; }
                if let Some(sc) = c["Sub Categories"].as_array() {
                    let joined: Vec<&str> = sc.iter().filter_map(|x| x.as_str()).collect();
                    if !joined.is_empty() { d.categories.push(joined.join("|")); }
                }
                if d.vendor.is_none() { d.vendor = c["Vendor"].as_str().map(String::from); }
            }
        }
    } else if let Some(i) = text.find("\"Vendor\"") {
        let rest = &text[i + 8..];
        if let Some(a) = rest.find('"') { if let Some(b) = rest[a + 1..].find('"') { d.vendor = Some(rest[a + 1..a + 1 + b].to_string()); } }
    }
}

/// JSON5 allows `,` before `}` / `]`; plain JSON does not. Drop those commas
/// (outside of strings) so serde can read the file.
fn strip_trailing_commas(t: &str) -> String {
    let c: Vec<char> = t.chars().collect();
    let mut out = String::with_capacity(t.len());
    let (mut in_str, mut esc) = (false, false);
    for i in 0..c.len() {
        let ch = c[i];
        if in_str {
            out.push(ch);
            if esc { esc = false } else if ch == '\\' { esc = true } else if ch == '"' { in_str = false }
            continue;
        }
        if ch == '"' { in_str = true; out.push(ch); continue; }
        if ch == ',' {
            let next = c[i + 1..].iter().find(|x| !x.is_whitespace());
            if matches!(next, Some('}') | Some(']')) { continue; }
        }
        out.push(ch);
    }
    out
}

fn plist_string(text: &str, key: &str) -> Option<String> {
    let i = text.find(&format!("<key>{key}</key>"))?;
    let rest = &text[i..];
    let a = rest.find("<string>")? + 8;
    let b = rest[a..].find("</string>")?;
    Some(rest[a..a + b].trim().to_string())
}

/// Read the Windows VERSIONINFO strings (CompanyName, ProductVersion, ...) from
/// the .rsrc section only, so large plugins are not read whole.
pub fn pe_version_strings(path: &Path) -> Vec<(String, String)> {
    let Ok(mut f) = File::open(path) else { return vec![] };
    let mut hdr = [0u8; 4096];
    let Ok(n) = f.read(&mut hdr) else { return vec![] };
    if n < 0x40 || &hdr[..2] != b"MZ" { return vec![]; }
    let pe = u32::from_le_bytes([hdr[0x3c], hdr[0x3d], hdr[0x3e], hdr[0x3f]]) as usize;
    if pe + 24 > n || &hdr[pe..pe + 4] != b"PE\0\0" { return vec![]; }
    let nsec = u16::from_le_bytes([hdr[pe + 6], hdr[pe + 7]]) as usize;
    let opt = u16::from_le_bytes([hdr[pe + 20], hdr[pe + 21]]) as usize;
    let sec0 = pe + 24 + opt;
    let mut rsrc: Option<(u64, u64)> = None;
    for i in 0..nsec {
        let o = sec0 + i * 40;
        if o + 40 > n { break; }
        if &hdr[o..o + 5] == b".rsrc" {
            let size = u32::from_le_bytes(hdr[o + 16..o + 20].try_into().unwrap()) as u64;
            let ptr = u32::from_le_bytes(hdr[o + 20..o + 24].try_into().unwrap()) as u64;
            rsrc = Some((ptr, size));
        }
    }
    let Some((ptr, size)) = rsrc else { return vec![] };
    // version info sits at the end of the resource data in practice; cap the read
    let cap = 48u64 << 20;
    let start = if size > cap { ptr + size - cap } else { ptr };
    let len = size.min(cap) as usize;
    if f.seek(SeekFrom::Start(start)).is_err() { return vec![]; }
    let mut buf = vec![0u8; len];
    if f.read_exact(&mut buf).is_err() { return vec![]; }
    let mut out = Vec::new();
    for key in ["CompanyName", "ProductVersion", "FileVersion", "FileDescription", "ProductName"] {
        if let Some(v) = find_version_value(&buf, key) { out.push((key.to_string(), v)); }
    }
    out
}

fn utf16(s: &str) -> Vec<u8> { s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect() }

/// In a VS_VERSIONINFO String entry the key is UTF-16, NUL-terminated, padded
/// to 4 bytes, then the UTF-16 value follows.
fn find_version_value(buf: &[u8], key: &str) -> Option<String> {
    let mut needle = utf16(key);
    needle.extend_from_slice(&[0, 0]);
    let pos = buf.windows(needle.len()).position(|w| w == needle.as_slice())?;
    let mut i = pos + needle.len();
    while i + 1 < buf.len() && buf[i] == 0 && buf[i + 1] == 0 { i += 2; if i - pos > needle.len() + 8 { break; } }
    let mut units = Vec::new();
    while i + 1 < buf.len() {
        let u = u16::from_le_bytes([buf[i], buf[i + 1]]);
        if u == 0 { break; }
        units.push(u);
        i += 2;
        if units.len() > 200 { break; }
    }
    let s = String::from_utf16_lossy(&units).trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn kind_from_categories(cats: &[String]) -> Option<String> {
    let map: &[(&str, &str)] = &[
        ("Reverb", "Реверб"), ("Delay", "Дилей"), ("Dynamics", "Динамика: компрессор, лимитер"), ("EQ", "Эквалайзер"),
        ("Filter", "Фильтр"), ("Distortion", "Дисторшн, сатурация"), ("Modulation", "Модуляция: хорус, фленджер"),
        ("Pitch Shift", "Питч, тюнинг"), ("Analyzer", "Анализатор"), ("Mastering", "Мастеринг"), ("Restoration", "Реставрация звука"),
        ("Spatial", "Пространство, стерео"), ("Surround", "Объёмный звук"), ("Tools", "Утилита"), ("Generator", "Генератор"),
        ("Synth", "Синтезатор"), ("Sampler", "Сэмплер"), ("Drum", "Драм-машина"), ("Piano", "Пианино"), ("Vocals", "Вокал"),
        ("Up-Downmix", "Утилита: мультиканал"), ("Network", "Сеть"),
    ];
    let mut labels: Vec<&str> = Vec::new();
    for c in cats {
        for part in c.split('|') {
            if let Some((_, l)) = map.iter().find(|(k, _)| part.eq_ignore_ascii_case(k)) { if !labels.contains(l) { labels.push(l); } }
        }
    }
    if labels.is_empty() {
        if cats.iter().any(|c| c.starts_with("Instrument")) { return Some("Инструмент".into()); }
        if cats.iter().any(|c| c.starts_with("Fx")) { return Some("Эффект".into()); }
        return None;
    }
    Some(labels.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn moduleinfo_and_categories() {
        let d = tempfile::tempdir().unwrap();
        let b = d.path().join("Verb.vst3");
        std::fs::create_dir_all(b.join("Contents")).unwrap();
        std::fs::write(b.join("Contents/moduleinfo.json"), r#"{ "Name": "Verb", "Version": "1.2.3", "Factory Info": { "Vendor": "Acme Audio", },
          "Classes": [ { "Category": "Audio Module Class", "Sub Categories": [ "Fx", "Reverb" ], }, ], }"#).unwrap();
        let r = read(&b, "Verb");
        assert_eq!(r.vendor.as_deref(), Some("Acme Audio"));
        assert_eq!(r.version.as_deref(), Some("1.2.3"));
        assert_eq!(r.kind.as_deref(), Some("Реверб"));
        assert_eq!(r.kind_source.as_deref(), Some("plugin"));
    }
    #[test]
    fn sdk_category_table_in_binary_is_ignored() {
        // a binary that contains the SDK's full category list must not get a type
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("Guitar.vst3");
        std::fs::write(&f, b"MZ\0\0\0Fx|Analyzer\0Fx|Delay\0Fx|Reverb\0Instrument|Synth\0").unwrap();
        let r = read(&f, "Guitar");
        assert!(r.kind.is_none(), "{:?}", r.kind);
        let g = d.path().join("SuperVerb.dll");
        std::fs::write(&g, b"nothing").unwrap();
        assert!(read(&g, "SuperVerb").kind.is_none(), "no guessing from the name");
    }
    #[test]
    fn version_value_parsing() {
        let mut buf = vec![0u8; 10];
        buf.extend(utf16("CompanyName")); buf.extend([0, 0]); buf.extend([0, 0]);
        buf.extend(utf16("FabFilter")); buf.extend([0, 0]);
        assert_eq!(find_version_value(&buf, "CompanyName").as_deref(), Some("FabFilter"));
    }
}
