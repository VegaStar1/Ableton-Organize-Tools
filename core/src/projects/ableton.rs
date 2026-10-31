//! Ableton Live sets (.als) and User Library presets are gzip-compressed XML.
//! Plugins appear as `Vst3PluginInfo/Name`, `VstPluginInfo/PlugName` and
//! `AuPluginInfo/Name`. Media appear as `FileRef/Path` + `FileRef/RelativePath`.
use crate::{FileRefEntry, PluginFormat, PluginRef, ProjectScan};
use flate2::read::GzDecoder;
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

pub fn parse(path: &Path) -> std::io::Result<ProjectScan> {
    let mut f = File::open(path)?;
    let mut magic = [0u8; 2];
    let n = f.read(&mut magic)?;
    f.seek(SeekFrom::Start(0))?;
    if n == 2 && magic == [0x1f, 0x8b] {
        parse_reader(BufReader::with_capacity(1 << 16, GzDecoder::new(f)))
    } else {
        parse_reader(BufReader::with_capacity(1 << 16, f))
    }
}

pub fn parse_reader<R: BufRead>(r: R) -> std::io::Result<ProjectScan> {
    let mut rd = Reader::from_reader(r);
    let mut buf = Vec::new();
    let mut stack: Vec<Vec<u8>> = Vec::new();
    let mut plugins: BTreeSet<(String, Option<PluginFormat>)> = BTreeSet::new();
    let mut files: BTreeSet<(String, Option<String>)> = BTreeSet::new();
    // current FileRef being assembled
    let mut cur_path: Option<String> = None;
    let mut cur_rel: Option<String> = None;

    loop {
        let ev = rd.read_event_into(&mut buf).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        match ev {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let name = e.name().as_ref().to_vec();
                let parent = stack.last().map(|v| v.as_slice());
                let value = || e.attributes().flatten().find(|a| a.key.as_ref() == b"Value")
                    .and_then(|a| a.unescape_value().ok().map(|c| c.into_owned()));
                match (parent, name.as_slice()) {
                    (Some(b"Vst3PluginInfo"), b"Name") => if let Some(v) = value() { plugins.insert((v, Some(PluginFormat::Vst3))); },
                    (Some(b"VstPluginInfo"), b"PlugName") => if let Some(v) = value() { plugins.insert((v, Some(PluginFormat::Vst2))); },
                    (Some(b"AuPluginInfo"), b"Name") => if let Some(v) = value() { plugins.insert((v, Some(PluginFormat::Au))); },
                    (Some(b"FileRef"), b"Path") => cur_path = value(),
                    (Some(b"FileRef"), b"RelativePath") => cur_rel = value(),
                    _ => {}
                }
                if matches!(ev, Event::Start(_)) {
                    if name == b"FileRef" { cur_path = None; cur_rel = None; }
                    stack.push(name);
                }
            }
            Event::End(_) => {
                if let Some(name) = stack.pop() {
                    if name == b"FileRef" {
                        match (cur_path.take(), cur_rel.take()) {
                            (Some(p), r) if !p.is_empty() => { files.insert((p, r.filter(|x| !x.is_empty()))); }
                            (None, Some(r)) | (Some(_), Some(r)) if !r.is_empty() => { files.insert((String::new(), Some(r))); }
                            _ => {}
                        }
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(ProjectScan {
        plugins: plugins.into_iter().map(|(name, format)| PluginRef { name, format }).collect(),
        files: files.into_iter().map(|(path, relative)| FileRefEntry { path, relative }).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const XML: &str = r#"<?xml version="1.0"?><Ableton><LiveSet><Tracks><MidiTrack>
      <PluginDevice><PluginDesc><Vst3PluginInfo Id="0"><Name Value="Serum"/><Uid><Fx Id="1"/></Uid></Vst3PluginInfo></PluginDesc></PluginDevice>
      <PluginDevice><PluginDesc><VstPluginInfo Id="0"><PlugName Value="Sylenth1"/><Path Value="C:/VST/Sylenth1.dll"/></VstPluginInfo></PluginDesc></PluginDevice>
      <SampleRef><FileRef><HasRelativePath Value="true"/><RelativePath Value="Samples/Recorded/a.wav"/><Path Value="D:/Proj/Samples/Recorded/a.wav"/></FileRef></SampleRef>
      <SampleRef><FileRef><Path Value="D:/Samples/Kick.wav"/></FileRef></SampleRef>
    </MidiTrack></Tracks></LiveSet></Ableton>"#;
    #[test]
    fn parses_plugins_and_files() {
        let s = parse_reader(XML.as_bytes()).unwrap();
        assert!(s.plugins.contains(&PluginRef { name: "Serum".into(), format: Some(PluginFormat::Vst3) }));
        assert!(s.plugins.contains(&PluginRef { name: "Sylenth1".into(), format: Some(PluginFormat::Vst2) }));
        assert_eq!(s.files.len(), 2);
        assert!(s.files.iter().any(|f| f.path == "D:/Samples/Kick.wav"));
        assert!(s.files.iter().any(|f| f.relative.as_deref() == Some("Samples/Recorded/a.wav")));
    }
}
