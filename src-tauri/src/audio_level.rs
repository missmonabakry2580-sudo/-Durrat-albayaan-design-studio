//! Turns the MP3 bytes ElevenLabs returns into a real-time loudness signal
//! (`voice://audio-level`) for whichever visual renderer the frontend is
//! showing — see src/components/presence/ThreeDAvatar.tsx and
//! docs/ARCHITECTURE.md's "Visual modes" section. This is honestly
//! amplitude-driven mouth movement, not phoneme-accurate lip sync: there is
//! no real-time phoneme alignment anywhere in this pipeline, and this
//! module doesn't add one. It only measures how loud the actual audio Mona
//! hears is, in short windows, so a talking mouth can move roughly in
//! proportion to her real voice output instead of on a random or invented
//! timer.
//!
//! Only wired up for the ElevenLabs path (commands::speak_text) — the
//! on-device AVSpeechSynthesizer path never hands Rust any audio bytes at
//! all (see voice.rs's doc comments), so there is nothing here to decode
//! for it; the 3D avatar simply gets no amplitude signal on that path and
//! its mouth stays at rest during speech, which is more honest than
//! inventing motion with no signal behind it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use tauri::{AppHandle, Emitter};

const WINDOW_MS: u32 = 40;
/// Reference RMS amplitude (out of i16::MAX = 32767) treated as "fully
/// open mouth" loudness — a fraction of full scale chosen to sit in normal
/// speech's typical range, not derived from any specific recording. Louder
/// audio simply clips to 1.0 rather than distorting the mapping.
const REFERENCE_RMS: f32 = 4500.0;

/// Decodes MP3 bytes to mono i16 PCM plus the track's sample rate. Real
/// decoding via symphonia (pure Rust, no system codec to bundle) — not a
/// stub or an estimate.
pub fn decode_mp3_mono(bytes: &[u8]) -> Result<(Vec<i16>, u32), String> {
    let cursor = std::io::Cursor::new(bytes.to_vec());
    let mss = MediaSourceStream::new(Box::new(cursor), Default::default());

    let mut hint = Hint::new();
    hint.with_extension("mp3");

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| format!("couldn't probe MP3 audio: {e}"))?;
    let mut format = probed.format;

    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .ok_or("no decodable audio track in this MP3")?
        .clone();
    let sample_rate = track.codec_params.sample_rate.ok_or("MP3 track has no sample rate")?;
    let channels = track.codec_params.channels.map(|c| c.count()).unwrap_or(1);

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("couldn't create MP3 decoder: {e}"))?;

    let mut mono = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(e) => return Err(format!("MP3 demux error: {e}")),
        };
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            // A single malformed frame shouldn't throw out an otherwise
            // decodable file — skip it and keep going, same tolerance
            // symphonia's own examples use.
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(e) => return Err(format!("MP3 decode error: {e}")),
        };
        let spec = *decoded.spec();
        let mut sample_buf = SampleBuffer::<i16>::new(decoded.capacity() as u64, spec);
        sample_buf.copy_interleaved_ref(decoded);
        for frame in sample_buf.samples().chunks(channels.max(1)) {
            let sum: i32 = frame.iter().map(|&s| s as i32).sum();
            mono.push((sum / frame.len().max(1) as i32) as i16);
        }
    }

    if mono.is_empty() {
        return Err("MP3 decoded to zero samples".to_string());
    }
    Ok((mono, sample_rate))
}

/// Short-window RMS loudness, normalized to 0.0-1.0 against REFERENCE_RMS.
/// A pure function so it's directly unit-testable without any real MP3 —
/// see the tests below.
pub fn rms_envelope(samples: &[i16], sample_rate: u32, window_ms: u32) -> Vec<f32> {
    if samples.is_empty() || sample_rate == 0 {
        return Vec::new();
    }
    let window_len = ((sample_rate as u64 * window_ms as u64) / 1000).max(1) as usize;
    samples
        .chunks(window_len)
        .map(|chunk| {
            let sum_sq: f64 = chunk.iter().map(|&s| (s as f64) * (s as f64)).sum();
            let rms = (sum_sq / chunk.len() as f64).sqrt();
            ((rms as f32) / REFERENCE_RMS).clamp(0.0, 1.0)
        })
        .collect()
}

/// Decodes and computes the envelope, then spawns a background thread that
/// emits `voice://audio-level` at `WINDOW_MS` cadence, matching the
/// envelope's own window size. This is a best-effort approximation of the
/// real wall-clock playback `elevenlabs::play` starts in its own thread at
/// roughly the same moment — this thread's own scheduling jitter is the
/// only drift source, not a different clock or a guess at timing. Silently
/// does nothing if decoding fails: Amin's actual voice (afplay) is
/// entirely unaffected either way, since this is cosmetic data for the 3D
/// avatar's mouth, never load-bearing for speech itself.
pub fn spawn_level_emitter(app: AppHandle, mp3: Vec<u8>) {
    let (samples, sample_rate) = match decode_mp3_mono(&mp3) {
        Ok(v) => v,
        Err(_) => return,
    };
    let envelope = rms_envelope(&samples, sample_rate, WINDOW_MS);
    if envelope.is_empty() {
        return;
    }

    let cancel = Arc::new(AtomicBool::new(true));
    crate::voice::set_audio_level_cancel(Some(cancel.clone()));

    std::thread::spawn(move || {
        for level in envelope {
            if !cancel.load(Ordering::SeqCst) {
                break;
            }
            let _ = app.emit("voice://audio-level", level);
            std::thread::sleep(Duration::from_millis(WINDOW_MS as u64));
        }
        crate::voice::set_audio_level_cancel(None);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine_samples(freq: f32, sample_rate: u32, duration_ms: u32, amplitude: f32) -> Vec<i16> {
        let n = (sample_rate as f32 * duration_ms as f32 / 1000.0) as usize;
        (0..n)
            .map(|i| {
                let t = i as f32 / sample_rate as f32;
                (amplitude * (2.0 * std::f32::consts::PI * freq * t).sin()) as i16
            })
            .collect()
    }

    #[test]
    fn silence_produces_zero_level() {
        let samples = vec![0i16; 1600]; // 100ms at 16kHz
        let env = rms_envelope(&samples, 16000, 40);
        assert!(!env.is_empty());
        assert!(env.iter().all(|&l| l == 0.0));
    }

    #[test]
    fn loud_tone_produces_high_level() {
        let samples = sine_samples(440.0, 16000, 200, 20000.0);
        let env = rms_envelope(&samples, 16000, 40);
        assert!(!env.is_empty());
        assert!(env.iter().all(|&l| l > 0.8), "expected near-max level, got {env:?}");
    }

    #[test]
    fn quiet_tone_produces_low_but_nonzero_level() {
        let loud = sine_samples(440.0, 16000, 200, 20000.0);
        let quiet = sine_samples(440.0, 16000, 200, 500.0);
        let loud_env = rms_envelope(&loud, 16000, 40);
        let quiet_env = rms_envelope(&quiet, 16000, 40);
        let loud_avg: f32 = loud_env.iter().sum::<f32>() / loud_env.len() as f32;
        let quiet_avg: f32 = quiet_env.iter().sum::<f32>() / quiet_env.len() as f32;
        assert!(quiet_avg > 0.0);
        assert!(quiet_avg < loud_avg);
    }

    #[test]
    fn every_level_is_clamped_to_0_1() {
        let samples = sine_samples(220.0, 44100, 500, i16::MAX as f32);
        let env = rms_envelope(&samples, 44100, 40);
        assert!(env.iter().all(|&l| (0.0..=1.0).contains(&l)));
    }

    #[test]
    fn empty_input_produces_empty_envelope() {
        assert!(rms_envelope(&[], 16000, 40).is_empty());
        assert!(rms_envelope(&[1, 2, 3], 0, 40).is_empty());
    }
}
