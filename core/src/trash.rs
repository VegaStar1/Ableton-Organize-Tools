//! In-app Recycle Bin. Files are *moved*, never deleted, until the user empties
//! the bin. Each volume gets its own bin folder so the move is an instant rename.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const TRASH_DIR_NAME: &str = ".ableton-organizer-trash";
const MANIFEST: &str = "manifest.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrashItem {
    pub id: String,
    pub original_path: String,
    pub stored_path: String,
    pub size: u64,
    pub trashed_at: u64,
    /// "plugin" or "sample", only for display.
    pub kind: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Manifest { items: Vec<TrashItem> }

/// Where the bin for a given file lives: next to the volume root on Windows,
/// otherwise inside the user's home (same volume in the common case; a copy
/// fallback covers other volumes).
pub fn bin_dir_for(path: &Path, home_fallback: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        use std::path::Component;
        if let Some(Component::Prefix(p)) = path.components().next() {
            let mut root = PathBuf::from(p.as_os_str());
            root.push("\\");
            return root.join(TRASH_DIR_NAME);
        }
    }
    let _ = path;
    home_fallback.join(TRASH_DIR_NAME)
}

fn manifest_path(bin: &Path) -> PathBuf { bin.join(MANIFEST) }
fn load(bin: &Path) -> Manifest {
    fs::read(manifest_path(bin)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}
fn save(bin: &Path, m: &Manifest) -> Result<()> {
    fs::create_dir_all(bin)?;
    let tmp = bin.join("manifest.json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(m)?)?;
    fs::rename(tmp, manifest_path(bin))?;
    Ok(())
}
fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }

pub struct Trash { bins: Vec<PathBuf>, home: PathBuf }

impl Trash {
    /// `known_bins` are bin folders seen earlier (persisted in settings).
    pub fn new(home: PathBuf, known_bins: Vec<PathBuf>) -> Self { Self { bins: known_bins, home } }
    pub fn bins(&self) -> &[PathBuf] { &self.bins }

    pub fn trash(&mut self, path: &Path, kind: &str, label: &str, size: u64) -> Result<TrashItem> {
        if !path.exists() { return Err(Error::Msg(format!("не найден: {}", path.display()))); }
        let mut bin = bin_dir_for(path, &self.home);
        if fs::create_dir_all(&bin).is_err() {
            bin = self.home.join(TRASH_DIR_NAME);
            fs::create_dir_all(&bin)?;
        }
        let id = format!("{:x}-{:x}", now(), fnv(path.to_string_lossy().as_bytes()));
        let dest_dir = bin.join(&id);
        fs::create_dir_all(&dest_dir)?;
        let dest = dest_dir.join(path.file_name().ok_or_else(|| Error::Msg("bad path".into()))?);
        move_path(path, &dest)?;
        let item = TrashItem { id, original_path: path.to_string_lossy().into_owned(), stored_path: dest.to_string_lossy().into_owned(), size, trashed_at: now(), kind: kind.into(), label: label.into() };
        let mut m = load(&bin);
        m.items.push(item.clone());
        save(&bin, &m)?;
        if !self.bins.contains(&bin) { self.bins.push(bin); }
        Ok(item)
    }

    pub fn list(&self) -> Vec<TrashItem> {
        let mut v: Vec<_> = self.bins.iter().flat_map(|b| load(b).items).collect();
        v.sort_by(|a, b| b.trashed_at.cmp(&a.trashed_at));
        v
    }

    /// Put an item back where it came from. Refuses to overwrite.
    pub fn restore(&self, id: &str) -> Result<()> {
        for bin in &self.bins {
            let mut m = load(bin);
            if let Some(pos) = m.items.iter().position(|i| i.id == id) {
                let it = m.items[pos].clone();
                let orig = PathBuf::from(&it.original_path);
                if orig.exists() { return Err(Error::Msg(format!("на месте уже есть файл: {}", orig.display()))); }
                if let Some(parent) = orig.parent() { fs::create_dir_all(parent)?; }
                move_path(Path::new(&it.stored_path), &orig)?;
                let _ = fs::remove_dir(Path::new(&it.stored_path).parent().unwrap_or(Path::new("")));
                m.items.remove(pos);
                save(bin, &m)?;
                return Ok(());
            }
        }
        Err(Error::Msg("элемент не найден в корзине".into()))
    }

    /// Permanently delete the given items. This is the only irreversible call.
    pub fn purge(&self, ids: &[String]) -> Result<u64> {
        let mut freed = 0;
        for bin in &self.bins {
            let mut m = load(bin);
            let mut changed = false;
            m.items.retain(|it| {
                if !ids.contains(&it.id) { return true; }
                let stored = Path::new(&it.stored_path);
                let dir = stored.parent().map(|p| p.to_path_buf());
                let _ = if stored.is_dir() { fs::remove_dir_all(stored) } else { fs::remove_file(stored) };
                if let Some(d) = dir { let _ = fs::remove_dir_all(d); }
                freed += it.size;
                changed = true;
                false
            });
            if changed { save(bin, &m)?; }
        }
        Ok(freed)
    }

    pub fn empty(&self) -> Result<u64> {
        let ids: Vec<String> = self.list().into_iter().map(|i| i.id).collect();
        self.purge(&ids)
    }
}

fn fnv(b: &[u8]) -> u32 { b.iter().fold(2166136261u32, |h, &c| (h ^ c as u32).wrapping_mul(16777619)) }

fn move_path(from: &Path, to: &Path) -> Result<()> {
    if fs::rename(from, to).is_ok() { return Ok(()); }
    // Different volume: copy, verify, then remove the source.
    if from.is_dir() { copy_dir(from, to)?; fs::remove_dir_all(from)?; }
    else { fs::copy(from, to)?; fs::remove_file(from)?; }
    Ok(())
}
fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)? {
        let e = e?;
        let t = to.join(e.file_name());
        if e.file_type()?.is_dir() { copy_dir(&e.path(), &t)?; } else { fs::copy(e.path(), t)?; }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trash_restore_purge() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a.wav");
        fs::write(&f, b"123").unwrap();
        let mut t = Trash::new(d.path().join("home"), vec![]);
        let it = t.trash(&f, "sample", "a.wav", 3).unwrap();
        assert!(!f.exists());
        assert_eq!(t.list().len(), 1);
        t.restore(&it.id).unwrap();
        assert!(f.exists() && t.list().is_empty());
        let it2 = t.trash(&f, "sample", "a.wav", 3).unwrap();
        assert_eq!(t.purge(&[it2.id]).unwrap(), 3);
        assert!(!f.exists() && t.list().is_empty());
    }
    #[test]
    fn restore_refuses_overwrite() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("b.wav");
        fs::write(&f, b"x").unwrap();
        let mut t = Trash::new(d.path().join("home"), vec![]);
        let it = t.trash(&f, "sample", "b", 1).unwrap();
        fs::write(&f, b"new").unwrap();
        assert!(t.restore(&it.id).is_err());
        assert_eq!(fs::read(&f).unwrap(), b"new");
    }
}
