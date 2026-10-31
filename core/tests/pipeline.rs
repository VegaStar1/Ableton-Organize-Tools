use flate2::{write::GzEncoder, Compression};
use organizer_core::analysis::{self, PluginStatus, Settings};
use organizer_core::daw::Daw;
use organizer_core::UsageState;
use std::fs;
use std::io::Write;
use std::path::Path;

fn write_als(path: &Path, xml: &str) {
    let mut e = GzEncoder::new(fs::File::create(path).unwrap(), Compression::fast());
    e.write_all(xml.as_bytes()).unwrap();
    e.finish().unwrap();
}

#[test]
fn end_to_end() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    // plugins
    let pl = root.join("plugins");
    fs::create_dir_all(&pl).unwrap();
    fs::write(pl.join("Serum.vst3"), vec![0u8; 1000]).unwrap();
    fs::write(pl.join("Serum_x64.dll"), b"MZ....VSTPluginMain....").unwrap();
    fs::write(pl.join("Helper.dll"), b"MZ plain library").unwrap();
    fs::write(pl.join("Reverb.vst3"), b"x").unwrap();
    fs::write(pl.join("Reverb.aaxplugin"), b"x").unwrap();
    fs::write(pl.join("Ghost.vst3"), b"x").unwrap();
    // samples
    let smp = root.join("samples");
    fs::create_dir_all(smp.join("Pack")).unwrap();
    for n in ["Kick.wav", "Snare.wav", "Unused.wav"] { fs::write(smp.join("Pack").join(n), vec![1u8; 2000]).unwrap(); }
    let proj = root.join("proj/Song Project");
    fs::create_dir_all(proj.join("Samples/Recorded")).unwrap();
    fs::write(proj.join("Samples/Recorded/take.wav"), b"t").unwrap();
    fs::write(proj.join("Samples/Recorded/leftover.wav"), b"t").unwrap();
    let kick = smp.join("Pack/Kick.wav").to_string_lossy().replace('\\', "/");
    let xml = format!(r#"<Ableton><Vst3PluginInfo><Name Value="Serum"/></Vst3PluginInfo>
      <SampleRef><FileRef><Path Value="{kick}"/></FileRef></SampleRef>
      <SampleRef><FileRef><RelativePath Value="Samples/Recorded/take.wav"/></FileRef></SampleRef>
      <SampleRef><FileRef><Path Value="Z:/gone/Snare.wav"/></FileRef></SampleRef></Ableton>"#);
    write_als(&proj.join("Song.als"), &xml);

    let s = Settings { daws: vec![Daw::Ableton], project_dirs: vec![root.join("proj")], sample_dirs: vec![smp.clone(), proj.join("Samples")],
        extra_plugin_dirs: vec![pl], cache_path: Some(root.join("cache.json")), ..Settings::default() };
    let a = analysis::run(&s, &|_| {});
    let r = &a.report;
    assert_eq!(r.projects.len(), 1);
    assert!(r.projects[0].missing.iter().any(|m| m.ends_with("Snare.wav")));

    let serum = r.plugins.iter().find(|p| p.name.starts_with("Serum")).unwrap();
    assert_eq!(serum.formats.len(), 2);
    assert_eq!(serum.status, PluginStatus::Duplicate);
    assert_eq!(serum.recommended.len(), 1);
    assert!(serum.recommended[0].to_lowercase().contains(".dll"));
    assert!(r.plugins.iter().all(|p| p.name != "Helper"));
    let ghost = r.plugins.iter().find(|p| p.name == "Ghost").unwrap();
    assert_eq!(ghost.status, PluginStatus::Unused);
    let reverb = r.plugins.iter().find(|p| p.name == "Reverb").unwrap();
    assert_eq!(reverb.recommended.len(), 2);

    let state = |name: &str| a.samples.entries.iter().find(|e| e.slash.ends_with(name)).unwrap().state;
    assert_eq!(state("Pack/Kick.wav"), UsageState::Used);
    assert_eq!(state("Pack/Snare.wav"), UsageState::Used, "relocated sample kept by name");
    assert_eq!(state("Pack/Unused.wav"), UsageState::Unused);
    assert_eq!(state("Recorded/take.wav"), UsageState::Used);
    assert_eq!(state("Recorded/leftover.wav"), UsageState::ProjectOwned);

    // second run is served from the cache
    let b = analysis::run(&s, &|_| {});
    assert_eq!(b.report.timings.cached_projects, 1);
    assert_eq!(b.report.timings.parsed_projects, 0);

    let kids = a.samples.children(None);
    assert!(!kids.is_empty());
}

#[test]
fn studio_one_usage_protects_all_loadable_formats() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let pl = root.join("plugins");
    fs::create_dir_all(&pl).unwrap();
    fs::write(pl.join("Massive.vst3"), b"x").unwrap();
    fs::write(pl.join("Massive.aaxplugin"), b"x").unwrap();
    let songs = root.join("songs");
    fs::create_dir_all(&songs).unwrap();
    let mut z = zip::ZipWriter::new(fs::File::create(songs.join("a.song")).unwrap());
    z.start_file("Devices/audiomixer.xml", zip::write::SimpleFileOptions::default()).unwrap();
    z.write_all(b"<Attributes name=\"Massive\" />").unwrap();
    z.finish().unwrap();
    let s = Settings { daws: vec![Daw::StudioOne], project_dirs: vec![songs], extra_plugin_dirs: vec![pl], ..Settings::default() };
    let a = analysis::run(&s, &|_| {});
    let m = a.report.plugins.iter().find(|p| p.name == "Massive").unwrap();
    let vst3 = m.formats.iter().find(|f| f.format.label() == "VST3").unwrap();
    let aax = m.formats.iter().find(|f| f.format.label() == "AAX").unwrap();
    assert!(!vst3.removable, "Studio One usage protects VST3");
    assert!(aax.removable, "AAX is not loadable by Studio One");
}

#[test]
fn cyrillic_folders_full_scan() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("Музыка");
    let pl = root.join("Плагины");
    fs::create_dir_all(&pl).unwrap();
    fs::write(pl.join("Массив.vst3"), b"x").unwrap();
    let smp = root.join("Сэмплы/Барабаны");
    fs::create_dir_all(&smp).unwrap();
    fs::write(smp.join("Бочка.wav"), b"k").unwrap();
    fs::write(smp.join("Лишний.wav"), b"k").unwrap();
    let songs = root.join("Песни/Трек");
    fs::create_dir_all(&songs).unwrap();
    let kick = smp.join("Бочка.wav").to_string_lossy().replace('\\', "/");
    let mut z = zip::ZipWriter::new(fs::File::create(songs.join("Трек.song")).unwrap());
    z.start_file("Song/mediapool.xml", zip::write::SimpleFileOptions::default()).unwrap();
    z.write_all(format!("<Url url=\"file:///{}\"/> плагин Массив тут", kick.trim_start_matches('/')).as_bytes()).unwrap();
    z.finish().unwrap();
    write_als(&songs.join("Проект.als"), &format!(r#"<Ableton><FileRef><Path Value="{kick}"/></FileRef></Ableton>"#));
    let s = Settings { daws: vec![Daw::Ableton, Daw::StudioOne], project_dirs: vec![root.join("Песни")], sample_dirs: vec![root.join("Сэмплы")], extra_plugin_dirs: vec![pl], ..Settings::default() };
    let a = analysis::run(&s, &|_| {});
    assert!(a.report.warnings.iter().all(|w| !w.contains("Не удалось")), "{:?}", a.report.warnings);
    assert_eq!(a.report.projects.len(), 2);
    let st = |n: &str| a.samples.entries.iter().find(|e| e.slash.ends_with(n)).unwrap().state;
    assert_eq!(st("Бочка.wav"), UsageState::Used);
    assert_eq!(st("Лишний.wav"), UsageState::Unused);
    assert!(!a.samples.children(None).is_empty());
}
