//! Talking to the operating system: show a file in Explorer/Finder, open a web
//! search, find and launch a plugin's own uninstaller.
use crate::plugins::family_key;
use crate::{Error, Result};
use serde::Serialize;
use std::path::Path;
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Show a file or folder selected in Explorer / Finder.
pub fn reveal(path: &Path) -> Result<()> {
    let target = if path.exists() { path.to_path_buf() } else {
        path.ancestors().find(|p| p.exists()).map(|p| p.to_path_buf()).ok_or_else(|| Error::Msg("Путь не найден".into()))?
    };
    #[cfg(windows)]
    {
        Command::new("explorer.exe").raw_arg(format!("/select,\"{}\"", target.display())).spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg("-R").arg(&target).spawn()?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("xdg-open").arg(target.parent().unwrap_or(&target)).spawn()?;
    }
    Ok(())
}

/// Open an image search for a plugin in the default browser.
pub fn web_search(query: &str) -> Result<()> {
    let q: String = percent_encoding::utf8_percent_encode(query, percent_encoding::NON_ALPHANUMERIC).to_string();
    let url = format!("https://www.google.com/search?tbm=isch&q={q}");
    open_url(&url)
}

fn open_url(url: &str) -> Result<()> {
    if !url.starts_with("https://") { return Err(Error::Msg("bad url".into())); }
    #[cfg(windows)]
    { Command::new("rundll32.exe").args(["url.dll,FileProtocolHandler", url]).creation_flags(CREATE_NO_WINDOW).spawn()?; }
    #[cfg(target_os = "macos")]
    { Command::new("open").arg(url).spawn()?; }
    #[cfg(all(unix, not(target_os = "macos")))]
    { Command::new("xdg-open").arg(url).spawn()?; }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct Uninstaller {
    /// Registry key path; used to launch it again later without trusting the UI.
    pub id: String,
    pub name: String,
    pub publisher: String,
    pub version: String,
    pub location: String,
    #[serde(skip)]
    pub command: String,
}

/// Every program registered in Windows "Apps & features".
pub fn list_uninstallers() -> Vec<Uninstaller> {
    #[cfg(windows)]
    {
        use winreg::enums::*;
        use winreg::RegKey;
        let mut out = Vec::new();
        let sources = [
            (HKEY_LOCAL_MACHINE, "HKLM", "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall"),
            (HKEY_LOCAL_MACHINE, "HKLM", "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall"),
            (HKEY_CURRENT_USER, "HKCU", "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall"),
        ];
        for (hive, hname, path) in sources {
            let Ok(root) = RegKey::predef(hive).open_subkey(path) else { continue };
            for sub in root.enum_keys().flatten() {
                let Ok(k) = root.open_subkey(&sub) else { continue };
                let get = |n: &str| k.get_value::<String, _>(n).unwrap_or_default();
                let name = get("DisplayName");
                let command = get("UninstallString");
                if name.is_empty() || command.is_empty() { continue; }
                if k.get_value::<u32, _>("SystemComponent").unwrap_or(0) == 1 { continue; }
                out.push(Uninstaller { id: format!("{hname}\\{path}\\{sub}"), name, publisher: get("Publisher"), version: get("DisplayVersion"), location: get("InstallLocation"), command });
            }
        }
        out
    }
    #[cfg(not(windows))]
    { Vec::new() }
}

/// Uninstallers that probably belong to this plugin, best first.
pub fn match_uninstallers(plugin: &str, paths: &[String], all: &[Uninstaller]) -> Vec<Uninstaller> {
    let fk = family_key(plugin);
    if fk.len() < 3 { return vec![]; }
    let lpaths: Vec<String> = paths.iter().map(|p| p.replace('/', "\\").to_lowercase()).collect();
    let mut scored: Vec<(u32, &Uninstaller)> = all.iter().filter_map(|u| {
        let uk = family_key(&u.name);
        let mut score = 0;
        if uk == fk { score = 100; }
        else if uk.starts_with(&fk) && fk.len() >= 4 { score = 80; }
        else if uk.contains(&fk) && fk.len() >= 5 { score = 65; }
        else if fk.contains(&uk) && uk.len() >= 6 { score = 55; }
        let loc = u.location.trim().trim_end_matches('\\').to_lowercase();
        if loc.len() > 3 && lpaths.iter().any(|p| p.starts_with(&format!("{loc}\\"))) { score += 60; }
        (score >= 55).then_some((score, u))
    }).collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.name.len().cmp(&b.1.name.len())));
    scored.into_iter().take(5).map(|(_, u)| u.clone()).collect()
}

/// Split an UninstallString into program and arguments.
pub fn split_command(cmd: &str) -> (String, String) {
    let c = cmd.trim();
    if let Some(rest) = c.strip_prefix('"') {
        if let Some(end) = rest.find('"') { return (rest[..end].to_string(), rest[end + 1..].trim().to_string()); }
    }
    let lower = c.to_ascii_lowercase();
    if let Some(i) = lower.find(".exe") { return (c[..i + 4].to_string(), c[i + 4..].trim().to_string()); }
    (c.to_string(), String::new())
}

/// Launch the uninstaller through the shell so Windows can show the UAC prompt.
pub fn launch_uninstaller(u: &Uninstaller) -> Result<()> {
    let (exe, mut args) = split_command(&u.command);
    if exe.to_ascii_lowercase().ends_with("msiexec.exe") {
        // "/I{GUID}" opens the repair/modify screen; "/X{GUID}" uninstalls.
        args = args.replacen("/I{", "/X{", 1).replacen("/i{", "/X{", 1);
    }
    #[cfg(windows)]
    {
        let q = |s: &str| format!("'{}'", s.replace('\'', "''"));
        let script = if args.is_empty() { format!("Start-Process -FilePath {}", q(&exe)) } else { format!("Start-Process -FilePath {} -ArgumentList {}", q(&exe), q(&args)) };
        Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &script]).creation_flags(CREATE_NO_WINDOW).spawn()?;
        Ok(())
    }
    #[cfg(not(windows))]
    { let _ = (exe, args); Err(Error::Msg("Деинсталляторы есть только в Windows".into())) }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn u(name: &str, loc: &str) -> Uninstaller { Uninstaller { id: name.into(), name: name.into(), publisher: String::new(), version: String::new(), location: loc.into(), command: "x".into() } }
    #[test]
    fn matching() {
        let all = vec![u("FabFilter Pro-Q 3 v3.21", ""), u("Massive X", ""), u("Massive", ""), u("Microsoft Edge", ""), u("Xfer Serum", "C:\\Program Files\\Xfer")];
        let m = match_uninstallers("FabFilter Pro-Q 3", &[], &all);
        assert_eq!(m[0].name, "FabFilter Pro-Q 3 v3.21");
        let m = match_uninstallers("Massive", &[], &all);
        assert_eq!(m[0].name, "Massive");
        let m = match_uninstallers("Serum_x64", &["C:\\Program Files\\Xfer\\Serum_x64.dll".into()], &all);
        assert_eq!(m[0].name, "Xfer Serum");
        assert!(match_uninstallers("Edge", &[], &all).is_empty());
    }
    #[test]
    fn split() {
        assert_eq!(split_command("\"C:\\a b\\unins000.exe\" /S"), ("C:\\a b\\unins000.exe".into(), "/S".into()));
        assert_eq!(split_command("MsiExec.exe /I{ABC}"), ("MsiExec.exe".into(), "/I{ABC}".into()));
        assert_eq!(split_command("C:\\x\\uninst.exe"), ("C:\\x\\uninst.exe".into(), "".into()));
    }
}
