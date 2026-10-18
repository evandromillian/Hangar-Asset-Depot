//! Audio metadata (WAV fully decoded for a waveform envelope).

use hangar_core::{paths, AudioInfo};
use std::path::Path;

const PEAK_BUCKETS: usize = 200;

pub fn analyze(root: &Path, rel: &str) -> AudioInfo {
    let format = paths::extension(rel);
    let mut info = AudioInfo { path: rel.to_string(), format: format.clone(), ..Default::default() };
    if format == "wav"
        && let Ok(reader) = hound::WavReader::open(root.join(rel))
    {
        analyze_wav(reader, &mut info);
    }
    info
}

fn analyze_wav<R: std::io::Read>(mut reader: hound::WavReader<R>, info: &mut AudioInfo) {
    let spec = reader.spec();
    info.sample_rate = Some(spec.sample_rate);
    info.channels = Some(spec.channels);
    let frames = reader.duration() as usize;
    if spec.sample_rate > 0 {
        info.duration_secs = Some(frames as f32 / spec.sample_rate as f32);
    }
    if frames == 0 {
        return;
    }
    let total_samples = frames * spec.channels as usize;
    let per_bucket = total_samples.div_ceil(PEAK_BUCKETS).max(1);
    let mut peaks = vec![0f32; total_samples.div_ceil(per_bucket)];
    let scale = match spec.sample_format {
        hound::SampleFormat::Float => 1.0,
        hound::SampleFormat::Int => 1.0 / (1u64 << (spec.bits_per_sample.saturating_sub(1))) as f32,
    };
    match spec.sample_format {
        hound::SampleFormat::Float => {
            for (i, s) in reader.samples::<f32>().enumerate() {
                let Ok(s) = s else { break };
                let b = &mut peaks[i / per_bucket];
                *b = b.max((s * scale).abs());
            }
        }
        hound::SampleFormat::Int => {
            for (i, s) in reader.samples::<i32>().enumerate() {
                let Ok(s) = s else { break };
                let b = &mut peaks[i / per_bucket];
                *b = b.max((s as f32 * scale).abs());
            }
        }
    }
    let max = peaks.iter().cloned().fold(0f32, f32::max);
    if max > 0.0 {
        for p in &mut peaks {
            *p = (*p / max).min(1.0);
        }
    }
    info.peaks = peaks;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_peaks() {
        let dir = tempfile::tempdir().unwrap();
        let spec = hound::WavSpec { channels: 1, sample_rate: 8000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(dir.path().join("a.wav"), spec).unwrap();
        for i in 0..8000 {
            let amp = if i < 4000 { 1000.0 } else { 16000.0 };
            w.write_sample(((i as f32 * 0.1).sin() * amp) as i16).unwrap();
        }
        w.finalize().unwrap();
        let info = analyze(dir.path(), "a.wav");
        assert_eq!(info.sample_rate, Some(8000));
        assert!((info.duration_secs.unwrap() - 1.0).abs() < 1e-3);
        assert_eq!(info.peaks.len(), 200);
        assert!(info.peaks[10] < 0.2 && info.peaks[150] > 0.9);
    }
}
