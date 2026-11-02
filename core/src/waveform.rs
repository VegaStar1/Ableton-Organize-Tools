//! Waveform overview for the player: streams the file once and keeps only
//! min/max per block, so even very large files use little memory.
use crate::{Error, Result};
use serde::Serialize;
use std::fs::File;
use std::path::Path;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

#[derive(Debug, Clone, Serialize)]
pub struct Wave {
    /// Interleaved min,max pairs in -1..1, `buckets` pairs long.
    pub peaks: Vec<f32>,
    pub duration: f64,
    pub sample_rate: u32,
    pub channels: u32,
}

const BLOCK: usize = 128;

pub fn compute(path: &Path, buckets: usize) -> Result<Wave> {
    let buckets = buckets.clamp(16, 4000);
    let file = File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) { hint.with_extension(ext); }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| Error::Msg(format!("формат не поддерживается: {e}")))?;
    let mut format = probed.format;
    let track = format.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| Error::Msg("в файле нет звука".into()))?;
    let track_id = track.id;
    let sample_rate = track.codec_params.sample_rate.unwrap_or(44100);
    let channels = track.codec_params.channels.map(|c| c.count() as u32).unwrap_or(1).max(1);
    let mut decoder = symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| Error::Msg(format!("кодек не поддерживается: {e}")))?;

    // min/max for every BLOCK frames
    let mut blocks: Vec<(f32, f32)> = Vec::new();
    let (mut lo, mut hi, mut n) = (f32::MAX, f32::MIN, 0usize);
    let mut frames: u64 = 0;
    let mut buf: Option<SampleBuffer<f32>> = None;
    loop {
        let packet = match format.next_packet() { Ok(p) => p, Err(_) => break };
        if packet.track_id() != track_id { continue; }
        let decoded = match decoder.decode(&packet) { Ok(d) => d, Err(symphonia::core::errors::Error::DecodeError(_)) => continue, Err(_) => break };
        let spec = *decoded.spec();
        let ch = spec.channels.count().max(1);
        if decoded.capacity() > 1 << 22 { return Err(Error::Msg("слишком большой блок в файле".into())); }
        if buf.as_ref().map_or(true, |b| b.capacity() < decoded.capacity() * ch) {
            buf = Some(SampleBuffer::<f32>::new(decoded.capacity() as u64, spec));
        }
        let b = buf.as_mut().unwrap();
        b.copy_interleaved_ref(decoded);
        for frame in b.samples().chunks(ch) {
            for &v in frame { if v < lo { lo = v } if v > hi { hi = v } }
            n += 1;
            if n == BLOCK { blocks.push((lo, hi)); lo = f32::MAX; hi = f32::MIN; n = 0; }
        }
        frames += (b.samples().len() / ch) as u64;
        // Overview of the first 3 hours is enough; stops runaway work on odd files.
        if frames > sample_rate as u64 * 3 * 3600 { break; }
    }
    if n > 0 { blocks.push((lo, hi)); }
    if blocks.is_empty() { return Err(Error::Msg("не удалось прочитать звук".into())); }

    let mut peaks = Vec::with_capacity(buckets * 2);
    for i in 0..buckets {
        let a = i * blocks.len() / buckets;
        let z = (((i + 1) * blocks.len()) / buckets).max(a + 1).min(blocks.len());
        let (mut l, mut h) = (0f32, 0f32);
        for &(bl, bh) in &blocks[a..z] { l = l.min(bl); h = h.max(bh); }
        peaks.push(l.clamp(-1.0, 1.0));
        peaks.push(h.clamp(-1.0, 1.0));
    }
    Ok(Wave { peaks, duration: frames as f64 / sample_rate as f64, sample_rate, channels })
}

/// Decode any supported file (AIFF, CAF, ALAC, ADPCM WAV...) into a plain
/// 16-bit PCM WAV that the built-in player can play. Keeps up to two channels.
pub fn transcode_to_wav(src: &Path, dest: &Path) -> Result<()> {
    use std::io::{BufWriter, Seek, SeekFrom, Write};
    let file = File::open(src)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = src.extension().and_then(|e| e.to_str()) { hint.with_extension(ext); }
    let probed = symphonia::default::get_probe().format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| Error::Msg(format!("формат не поддерживается: {e}")))?;
    let mut format = probed.format;
    let track = format.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL).ok_or_else(|| Error::Msg("в файле нет звука".into()))?;
    let track_id = track.id;
    let rate = track.codec_params.sample_rate.unwrap_or(44100);
    let mut decoder = symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| Error::Msg(format!("кодек не поддерживается: {e}")))?;
    if let Some(dir) = dest.parent() { std::fs::create_dir_all(dir)?; }
    let tmp = dest.with_extension("part");
    let mut w = BufWriter::new(File::create(&tmp)?);
    w.write_all(&[0u8; 44])?; // header written at the end
    let mut out_ch: u16 = 0;
    let mut data_bytes: u64 = 0;
    let mut buf: Option<SampleBuffer<f32>> = None;
    loop {
        let packet = match format.next_packet() { Ok(p) => p, Err(_) => break };
        if packet.track_id() != track_id { continue; }
        let decoded = match decoder.decode(&packet) { Ok(d) => d, Err(symphonia::core::errors::Error::DecodeError(_)) => continue, Err(_) => break };
        let spec = *decoded.spec();
        let ch = spec.channels.count().max(1);
        if out_ch == 0 { out_ch = ch.min(2) as u16; }
        if decoded.capacity() > 1 << 22 { return Err(Error::Msg("слишком большой блок в файле".into())); }
        if buf.as_ref().map_or(true, |b| b.capacity() < decoded.capacity() * ch) { buf = Some(SampleBuffer::<f32>::new(decoded.capacity() as u64, spec)); }
        let b = buf.as_mut().unwrap();
        b.copy_interleaved_ref(decoded);
        for frame in b.samples().chunks(ch) {
            for c in 0..out_ch as usize {
                let v = frame.get(c).or(frame.first()).copied().unwrap_or(0.0);
                w.write_all(&((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())?;
                data_bytes += 2;
            }
        }
        if data_bytes > 3_500_000_000 { break; } // WAV size limit
    }
    if out_ch == 0 { return Err(Error::Msg("не удалось прочитать звук".into())); }
    let mut f = w.into_inner().map_err(|e| Error::Msg(e.to_string()))?;
    f.seek(SeekFrom::Start(0))?;
    let block = out_ch as u32 * 2;
    let mut h = Vec::with_capacity(44);
    h.extend_from_slice(b"RIFF"); h.extend_from_slice(&((36 + data_bytes) as u32).to_le_bytes()); h.extend_from_slice(b"WAVE");
    h.extend_from_slice(b"fmt "); h.extend_from_slice(&16u32.to_le_bytes()); h.extend_from_slice(&1u16.to_le_bytes()); h.extend_from_slice(&out_ch.to_le_bytes());
    h.extend_from_slice(&rate.to_le_bytes()); h.extend_from_slice(&(rate * block).to_le_bytes()); h.extend_from_slice(&(block as u16).to_le_bytes()); h.extend_from_slice(&16u16.to_le_bytes());
    h.extend_from_slice(b"data"); h.extend_from_slice(&(data_bytes as u32).to_le_bytes());
    f.write_all(&h)?;
    drop(f);
    std::fs::rename(&tmp, dest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn write_wav(path: &Path, samples: &[i16], rate: u32) {
        let mut v = Vec::new();
        let data_len = (samples.len() * 2) as u32;
        v.extend_from_slice(b"RIFF"); v.extend_from_slice(&(36 + data_len).to_le_bytes()); v.extend_from_slice(b"WAVE");
        v.extend_from_slice(b"fmt "); v.extend_from_slice(&16u32.to_le_bytes()); v.extend_from_slice(&1u16.to_le_bytes()); v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&rate.to_le_bytes()); v.extend_from_slice(&(rate * 2).to_le_bytes()); v.extend_from_slice(&2u16.to_le_bytes()); v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data"); v.extend_from_slice(&data_len.to_le_bytes());
        for s in samples { v.extend_from_slice(&s.to_le_bytes()); }
        std::fs::write(path, v).unwrap();
    }
    fn write_aiff(path: &Path, samples: &[i16], rate: u32) {
        // minimal AIFF: COMM + SSND, big-endian 16-bit mono
        let mut comm = Vec::new();
        comm.extend_from_slice(&1u16.to_be_bytes()); comm.extend_from_slice(&(samples.len() as u32).to_be_bytes()); comm.extend_from_slice(&16u16.to_be_bytes());
        // 80-bit extended sample rate
        let e = 16383 + 31 - (rate.leading_zeros() as u16) ; let m = (rate as u64) << (rate.leading_zeros() + 32);
        comm.extend_from_slice(&e.to_be_bytes()); comm.extend_from_slice(&m.to_be_bytes());
        let mut ssnd = vec![0u8; 8];
        for s in samples { ssnd.extend_from_slice(&s.to_be_bytes()); }
        let mut v = Vec::new();
        v.extend_from_slice(b"FORM"); v.extend_from_slice(&((4 + 8 + comm.len() + 8 + ssnd.len()) as u32).to_be_bytes()); v.extend_from_slice(b"AIFF");
        v.extend_from_slice(b"COMM"); v.extend_from_slice(&(comm.len() as u32).to_be_bytes()); v.extend_from_slice(&comm);
        v.extend_from_slice(b"SSND"); v.extend_from_slice(&(ssnd.len() as u32).to_be_bytes()); v.extend_from_slice(&ssnd);
        std::fs::write(path, v).unwrap();
    }
    #[test]
    fn aiff_transcodes_to_playable_wav() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("kick.aif");
        let s: Vec<i16> = (0..44100).map(|i| ((i as f32 / 20.0).sin() * 20000.0) as i16).collect();
        write_aiff(&src, &s, 44100);
        let dest = d.path().join("out/kick.wav");
        transcode_to_wav(&src, &dest).unwrap();
        let w = compute(&dest, 50).unwrap();
        assert!((w.duration - 1.0).abs() < 0.01, "{}", w.duration);
        assert!(w.peaks.iter().cloned().fold(0f32, f32::max) > 0.5);
        assert_eq!(&std::fs::read(&dest).unwrap()[..4], b"RIFF");
    }
    #[test]
    fn waveform_of_wav() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("t.wav");
        // 1 s of silence then 1 s of a loud square wave
        let mut s = vec![0i16; 44100];
        s.extend((0..44100).map(|i| if (i / 50) % 2 == 0 { 30000 } else { -30000 }));
        write_wav(&p, &s, 44100);
        let w = compute(&p, 100).unwrap();
        assert_eq!(w.peaks.len(), 200);
        assert!((w.duration - 2.0).abs() < 0.01, "{}", w.duration);
        assert!(w.peaks[1].abs() < 0.01, "first half silent");
        assert!(w.peaks[199] > 0.8 && w.peaks[198] < -0.8, "second half loud");
    }
}
