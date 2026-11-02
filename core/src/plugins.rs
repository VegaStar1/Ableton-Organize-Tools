use crate::{InstalledPlugin, PluginFormat};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Standard plugin folders for the current OS, per format.
pub fn default_dirs() -> Vec<(PluginFormat, PathBuf)> {
    use PluginFormat::*;
    let mut v = Vec::new();
    if cfg!(windows) {
        let env = |k: &str, d: &str| std::env::var_os(k).map(PathBuf::from).unwrap_or_else(|| d.into());
        let pf = env("ProgramFiles", "C:\\Program Files");
        let pfx = env("ProgramFiles(x86)", "C:\\Program Files (x86)");
        let common = env("CommonProgramFiles", "C:\\Program Files\\Common Files");
        let local = dirs::data_local_dir().unwrap_or_default().join("Programs").join("Common");
        v.push((Vst3, common.join("VST3")));
        v.push((Vst3, local.join("VST3")));
        for base in [&pf, &pfx] {
            v.push((Vst2, base.join("VSTPlugins")));
            v.push((Vst2, base.join("Steinberg").join("VSTPlugins")));
            v.push((Vst2, base.join("Common Files").join("VST2")));
            v.push((Vst2, base.join("Common Files").join("Steinberg").join("VST2")));
        }
        v.push((Aax, common.join("Avid").join("Audio").join("Plug-Ins")));
        v.push((Clap, common.join("CLAP")));
        v.push((Clap, local.join("CLAP")));
    } else {
        let home = dirs::home_dir().unwrap_or_default();
        for base in [PathBuf::from("/Library/Audio/Plug-Ins"), home.join("Library/Audio/Plug-Ins")] {
            v.push((Vst3, base.join("VST3")));
            v.push((Vst2, base.join("VST")));
            v.push((Au, base.join("Components")));
            v.push((Clap, base.join("CLAP")));
        }
        v.push((Aax, PathBuf::from("/Library/Application Support/Avid/Audio/Plug-Ins")));
    }
    v.into_iter().filter(|(_, p)| p.is_dir()).collect()
}

fn format_for_ext(ext: &str) -> Option<PluginFormat> {
    match ext {
        "vst3" => Some(PluginFormat::Vst3),
        "aaxplugin" => Some(PluginFormat::Aax),
        "clap" => Some(PluginFormat::Clap),
        "component" => Some(PluginFormat::Au),
        "vst" => Some(PluginFormat::Vst2), // macOS bundle
        "dll" => Some(PluginFormat::Vst2), // verified below
        _ => None,
    }
}

/// Scan the given directories (format hint is only used for the folder's
/// default; the extension decides the real format).
pub fn scan_dirs(dirs: &[PathBuf]) -> Vec<InstalledPlugin> {
    let mut out = Vec::new();
    for d in dirs {
        walk(d, 0, &mut out);
    }
    // The same folder can be reached twice (default list + user folder, different
    // case or slashes): keep one entry per real file.
    let norm = |p: &str| p.replace('\\', "/").trim_end_matches('/').to_lowercase();
    out.sort_by(|a, b| norm(&a.path).cmp(&norm(&b.path)));
    out.dedup_by(|a, b| norm(&a.path) == norm(&b.path));
    out
}

fn walk(dir: &Path, depth: u32, out: &mut Vec<InstalledPlugin>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let path = e.path();
        let ext = path.extension().and_then(|s| s.to_str()).map(|s| s.to_ascii_lowercase());
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if let Some(fmt) = ext.as_deref().and_then(format_for_ext) {
            if fmt == PluginFormat::Vst2 && ext.as_deref() == Some("dll") && !(path.is_file() && is_vst2_dll(&path)) {
                continue;
            }
            let name = display_name(&path);
            out.push(InstalledPlugin { name, format: fmt, path: path.to_string_lossy().into_owned(), size: entry_size(&path) });
            continue; // never descend into bundles
        }
        if is_dir && depth < 6 {
            walk(&path, depth + 1, out);
        }
    }
}

fn display_name(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

pub fn entry_size(p: &Path) -> u64 {
    match std::fs::metadata(p) {
        Ok(m) if m.is_file() => m.len(),
        Ok(_) => jwalk::WalkDir::new(p).skip_hidden(false).into_iter().flatten()
            .filter_map(|e| if e.file_type().is_file() { e.metadata().ok().map(|m| m.len()) } else { None }).sum(),
        Err(_) => 0,
    }
}

/// A Windows VST2 plugin exports `VSTPluginMain` (or legacy `main`). Plain
/// helper DLLs do not, so they are not listed as plugins.
fn is_vst2_dll(p: &Path) -> bool {
    file_contains(p, b"VSTPluginMain").unwrap_or(false)
}

fn file_contains(p: &Path, needle: &[u8]) -> std::io::Result<bool> {
    let mut f = File::open(p)?;
    let mut buf = vec![0u8; 1 << 20];
    let mut carry: Vec<u8> = Vec::new();
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 { return Ok(false); }
        carry.extend_from_slice(&buf[..n]);
        if carry.windows(needle.len()).any(|w| w == needle) { return Ok(true); }
        let keep = needle.len().saturating_sub(1).min(carry.len());
        carry.drain(..carry.len() - keep);
    }
}

/// Key used to recognise the same product across formats and project names:
/// lowercase alphanumerics, with architecture suffixes removed.
pub fn family_key(name: &str) -> String {
    let mut s = name.trim().to_ascii_lowercase();
    for suf in ["(x64)", "(64-bit)", "(64bit)", "_x64", "-x64", " x64", "_64", " 64bit", "(vst3)", "(vst)", "(aax)", "(au)"] {
        if let Some(t) = s.strip_suffix(suf) { s = t.trim_end().to_string(); }
    }
    s.chars().filter(|c| c.is_alphanumeric()).collect()
}

pub struct Family {
    pub display: String,
    pub plugins: Vec<InstalledPlugin>,
}

/// Group formats of the same product together.
pub fn group_families(list: Vec<InstalledPlugin>) -> Vec<Family> {
    let mut map: BTreeMap<String, Family> = BTreeMap::new();
    for p in list {
        let key = family_key(&p.name);
        if key.is_empty() { continue; }
        let f = map.entry(key).or_insert_with(|| Family { display: p.name.clone(), plugins: vec![] });
        // Prefer the shortest, cleanest display name (no "_x64" tails).
        if p.name.len() < f.display.len() { f.display = p.name.clone(); }
        f.plugins.push(p);
    }
    map.into_values().collect()
}

/// Local pictures of plugins, no internet needed:
/// - VST3 bundles may ship GUI snapshots in `Contents/Resources/Snapshots`
/// - Native Instruments NKS artwork in `NI Resources/image/<product>`
/// Returns family key -> image path.
pub fn find_images(families: &[Family]) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    for f in families {
        for p in &f.plugins {
            if p.format != PluginFormat::Vst3 { continue; }
            let snaps = Path::new(&p.path).join("Contents").join("Resources").join("Snapshots");
            if let Ok(rd) = std::fs::read_dir(&snaps) {
                let mut pngs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|x| x.extension().map_or(false, |e| e.eq_ignore_ascii_case("png"))).collect();
                // prefer the 1x snapshot (smaller) when both exist
                pngs.sort_by_key(|x| x.to_string_lossy().contains("_2.0x"));
                if let Some(img) = pngs.first() { out.insert(family_key(&f.display), img.to_string_lossy().into_owned()); }
            }
        }
    }
    let ni_roots: Vec<PathBuf> = if cfg!(windows) { vec![PathBuf::from("C:\\Users\\Public\\Documents\\NI Resources\\image")] } else { vec![PathBuf::from("/Users/Shared/NI Resources/image")] };
    let want: std::collections::HashMap<String, String> = families.iter().map(|f| (family_key(&f.display), f.display.clone())).collect();
    for root in ni_roots {
        for e in jwalk::WalkDir::new(&root).max_depth(3).into_iter().flatten() {
            if !e.file_type().is_dir() { continue; }
            let key = family_key(&e.file_name().to_string_lossy());
            if key.is_empty() || out.contains_key(&key) || !want.contains_key(&key) { continue; }
            for name in ["VB_artwork.png", "MST_artwork.png", "OSO_logo.png", "VB_logo.png"] {
                let p = e.path().join(name);
                if p.is_file() { out.insert(key.clone(), p.to_string_lossy().into_owned()); break; }
            }
        }
    }
    out
}
