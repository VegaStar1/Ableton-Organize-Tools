use crate::PluginFormat;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Daw {
    Ableton,
    StudioOne,
    FlStudio,
    ProTools,
    Bitwig,
    Cubase,
    Reaper,
    Logic,
}

pub const ALL_DAWS: [Daw; 8] = [
    Daw::Ableton, Daw::StudioOne, Daw::FlStudio, Daw::ProTools,
    Daw::Bitwig, Daw::Cubase, Daw::Reaper, Daw::Logic,
];

/// How deeply we understand a DAW's project files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SupportLevel {
    /// Project structure is parsed; plugin format and file paths are exact.
    Exact,
    /// Project is searched for plugin names and file paths; format is unknown.
    Heuristic,
    /// Projects cannot be read. Everything this DAW can load stays protected.
    None,
}

impl Daw {
    pub fn name(self) -> &'static str {
        match self {
            Daw::Ableton => "Ableton Live",
            Daw::StudioOne => "Studio One",
            Daw::FlStudio => "FL Studio",
            Daw::ProTools => "Pro Tools",
            Daw::Bitwig => "Bitwig Studio",
            Daw::Cubase => "Cubase",
            Daw::Reaper => "REAPER",
            Daw::Logic => "Logic Pro",
        }
    }
    pub fn project_exts(self) -> &'static [&'static str] {
        match self {
            Daw::Ableton => &["als"],
            Daw::StudioOne => &["song"],
            Daw::FlStudio => &["flp"],
            Daw::ProTools => &["ptx", "ptf"],
            Daw::Bitwig => &["bwproject"],
            Daw::Cubase => &["cpr"],
            Daw::Reaper => &["rpp"],
            Daw::Logic => &["logicx"],
        }
    }
    pub fn support(self) -> SupportLevel {
        match self {
            Daw::Ableton => SupportLevel::Exact,
            Daw::Logic => SupportLevel::None,
            _ => SupportLevel::Heuristic,
        }
    }
    /// Plugin formats this DAW can load.
    pub fn formats(self) -> &'static [PluginFormat] {
        use PluginFormat::*;
        match self {
            Daw::Ableton => &[Vst3, Vst2, Au],
            Daw::StudioOne => &[Vst3, Vst2, Au],
            Daw::FlStudio => &[Vst3, Vst2, Au],
            Daw::ProTools => &[Aax],
            Daw::Bitwig => &[Vst3, Vst2, Clap],
            Daw::Cubase => &[Vst3, Vst2],
            Daw::Reaper => &[Vst3, Vst2, Au, Clap],
            Daw::Logic => &[Au],
        }
    }
    /// Folders where the DAW usually keeps projects, relative to the home dir.
    pub fn default_project_dirs(self) -> Vec<PathBuf> {
        let docs = dirs::document_dir().unwrap_or_default();
        let music = dirs::audio_dir().unwrap_or_default();
        match self {
            Daw::Ableton => vec![docs.join("Ableton"), music.join("Ableton")],
            Daw::StudioOne => vec![docs.join("Studio One").join("Songs")],
            Daw::FlStudio => vec![docs.join("Image-Line").join("FL Studio").join("Projects")],
            Daw::ProTools => vec![docs.join("Pro Tools")],
            Daw::Bitwig => vec![docs.join("Bitwig Studio").join("Projects")],
            Daw::Cubase => vec![docs.join("Steinberg")],
            Daw::Reaper => vec![docs.join("REAPER Media")],
            Daw::Logic => vec![music.join("Logic")],
        }
    }
    /// Where the app itself is usually installed. Used only for detection.
    fn install_markers(self) -> Vec<PathBuf> {
        let mut v = Vec::new();
        if cfg!(windows) {
            let pf = std::env::var_os("ProgramFiles").map(PathBuf::from).unwrap_or_else(|| "C:\\Program Files".into());
            let pfx = std::env::var_os("ProgramFiles(x86)").map(PathBuf::from).unwrap_or_else(|| "C:\\Program Files (x86)".into());
            let local = dirs::data_local_dir().unwrap_or_default();
            for base in [pf, pfx] {
                match self {
                    Daw::Ableton => v.push(base.join("Ableton")),
                    Daw::StudioOne => { v.push(base.join("PreSonus")); v.push(base.join("Studio One")) }
                    Daw::FlStudio => v.push(base.join("Image-Line")),
                    Daw::ProTools => { v.push(base.join("Avid").join("Pro Tools")); v.push(base.join("Avid")) }
                    Daw::Bitwig => v.push(base.join("Bitwig Studio")),
                    Daw::Cubase => v.push(base.join("Steinberg")),
                    Daw::Reaper => v.push(base.join("REAPER (x64)")),
                    Daw::Logic => {}
                }
            }
            if self == Daw::StudioOne { v.push(local.join("PreSonus")); }
        } else {
            let app = PathBuf::from("/Applications");
            match self {
                Daw::Ableton => v.push(app.clone()),
                Daw::StudioOne => v.push(app.join("Studio One.app")),
                Daw::FlStudio => v.push(app.join("FL Studio.app")),
                Daw::ProTools => v.push(app.join("Pro Tools.app")),
                Daw::Bitwig => v.push(app.join("Bitwig Studio.app")),
                Daw::Cubase => v.push(app.clone()),
                Daw::Reaper => v.push(app.join("REAPER.app")),
                Daw::Logic => v.push(app.join("Logic Pro.app")),
            }
        }
        v
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DetectedDaw {
    pub daw: Daw,
    pub name: &'static str,
    pub installed: bool,
    pub support: SupportLevel,
    pub project_dirs: Vec<String>,
}

/// Look for installed DAWs. Detection is a convenience: the user can always
/// tick DAWs manually.
pub fn detect() -> Vec<DetectedDaw> {
    ALL_DAWS.iter().map(|&d| {
        let installed = d.install_markers().iter().any(|p| marker_present(d, p));
        let project_dirs = d.default_project_dirs().into_iter()
            .filter(|p| p.is_dir()).map(|p| p.to_string_lossy().into_owned()).collect();
        DetectedDaw { daw: d, name: d.name(), installed, support: d.support(), project_dirs }
    }).collect()
}

fn marker_present(d: Daw, p: &Path) -> bool {
    if !p.exists() { return false; }
    // Shared parents (/Applications, Program Files\Avid) need a name check.
    match d {
        Daw::Ableton if !cfg!(windows) => has_child_starting(p, "Ableton Live"),
        Daw::Cubase if !cfg!(windows) => has_child_starting(p, "Cubase"),
        Daw::Cubase => has_child_starting(p, "Cubase"),
        Daw::ProTools if cfg!(windows) && p.ends_with("Avid") => has_child_starting(p, "Pro Tools"),
        _ => true,
    }
}

fn has_child_starting(dir: &Path, prefix: &str) -> bool {
    std::fs::read_dir(dir).map(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().starts_with(prefix))).unwrap_or(false)
}
