use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum PluginFormat {
    Vst3,
    Vst2,
    Aax,
    Au,
    Clap,
}

impl PluginFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Vst3 => "VST3",
            Self::Vst2 => "VST2",
            Self::Aax => "AAX",
            Self::Au => "AU",
            Self::Clap => "CLAP",
        }
    }
}

/// One plugin file/bundle on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledPlugin {
    pub name: String,
    pub format: PluginFormat,
    pub path: String,
    pub size: u64,
}

/// A reference to a plugin found inside a project file.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PluginRef {
    pub name: String,
    /// `None` when the project format does not tell us which format was used.
    pub format: Option<PluginFormat>,
}

/// A reference to a media file found inside a project file.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileRefEntry {
    pub path: String,
    /// Path relative to the project folder, when the DAW stores one.
    pub relative: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectScan {
    pub plugins: Vec<PluginRef>,
    pub files: Vec<FileRefEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UsageState {
    /// Referenced by a recent project.
    Used,
    /// Referenced only by projects older than the "old" threshold.
    OldOnly,
    /// Not referenced by anything we scanned.
    Unused,
    /// Lives inside a project folder: part of that project.
    ProjectOwned,
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Msg(String),
}
pub type Result<T> = std::result::Result<T, Error>;
