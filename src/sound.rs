//! Short taps for a click. The sounds are synthesized, so nav ships no audio files.

use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat};

const OPEN_SECONDS: f32 = 0.07;
const UP_SECONDS: f32 = 0.10;
const MISS_SECONDS: f32 = 0.04;
const HOVER_SECONDS: f32 = 0.032;

/// Which click just happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tap {
    /// A folder opened.
    Open,
    /// The view moved to the parent.
    Up,
    /// The click landed on empty ground.
    Miss,
    /// The pointer moved onto a folder.
    Hover,
}

/// Plays taps on the default output. If that output cannot be opened, clicks stay silent.
pub struct Clicks {
    mixer: Option<Arc<Mutex<Mixer>>>,
    _stream: Option<cpal::Stream>,
}

impl Clicks {
    pub fn start() -> Self {
        match open_output() {
            Ok((mixer, stream)) => Self {
                mixer: Some(mixer),
                _stream: Some(stream),
            },
            Err(err) => {
                eprintln!("nav: click sounds are off ({err})");
                Self {
                    mixer: None,
                    _stream: None,
                }
            }
        }
    }

    pub fn is_live(&self) -> bool {
        self.mixer.is_some()
    }

    /// `pitch` is 0 to 1 and shifts the tone a little, so two stones do not sound identical.
    pub fn play(&self, tap: Tap, pitch: f32) {
        let Some(mixer) = &self.mixer else {
            return;
        };
        if let Ok(mut mixer) = mixer.lock() {
            mixer.trigger(tap, pitch);
        }
    }
}

struct Voice {
    tap: Tap,
    pitch: f32,
    age: f32,
}

struct Mixer {
    voices: Vec<Voice>,
    sample_rate: f32,
}

impl Mixer {
    fn trigger(&mut self, tap: Tap, pitch: f32) {
        if self.voices.len() >= 8 {
            self.voices.remove(0);
        }
        self.voices.push(Voice {
            tap,
            pitch: pitch.clamp(0.0, 1.0),
            age: 0.0,
        });
    }

    fn next_sample(&mut self) -> f32 {
        let step = 1.0 / self.sample_rate.max(1.0);
        let mut sum = 0.0;
        self.voices.retain_mut(|voice| {
            sum += sample_at(voice.tap, voice.age, voice.pitch);
            voice.age += step;
            voice.age < seconds(voice.tap)
        });
        sum.clamp(-0.8, 0.8)
    }
}

fn seconds(tap: Tap) -> f32 {
    match tap {
        Tap::Open => OPEN_SECONDS,
        Tap::Up => UP_SECONDS,
        Tap::Miss => MISS_SECONDS,
        Tap::Hover => HOVER_SECONDS,
    }
}

/// One sample of a tap at `t` seconds. The sound starts at full attack and fades to silence.
pub fn sample_at(tap: Tap, t: f32, pitch: f32) -> f32 {
    let pitch = pitch.clamp(0.0, 1.0);
    let dur = seconds(tap);
    if t < 0.0 || t >= dur {
        return 0.0;
    }
    let (freq, decay, noise_mix, gain) = match tap {
        Tap::Open => (250.0 + pitch * 130.0, 52.0, 0.42, 0.22),
        Tap::Up => (118.0 + pitch * 36.0, 26.0, 0.18, 0.20),
        Tap::Miss => (480.0 + pitch * 90.0, 64.0, 0.38, 0.16),
        Tap::Hover => (520.0 + pitch * 180.0, 70.0, 0.28, 0.18),
    };
    // Linear drop in pitch. Phase is the integral, so the tone stays continuous.
    let phase = std::f32::consts::TAU * freq * (t - 0.35 * t * t / (2.0 * dur));
    let body = phase.sin() + (phase * 2.35).sin() * 0.28;
    let noise = hash_noise((t * 48_000.0) as u32) * (-t * 260.0).exp();
    let env = (-t * decay).exp() * (1.0 - t / dur);
    (body * (1.0 - noise_mix) + noise * noise_mix) * env * gain
}

fn hash_noise(n: u32) -> f32 {
    let mut h = n.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    h = (h >> 16) ^ h;
    let x = (h.wrapping_mul(h.wrapping_mul(73_244_475).wrapping_add(1)) >> 16) & 0xffff;
    (x as f32 / 32_768.0) - 1.0
}

fn open_output() -> Result<(Arc<Mutex<Mixer>>, cpal::Stream), String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "no audio output".to_string())?;
    let supported = device.default_output_config().map_err(|err| err.to_string())?;
    let mixer = Arc::new(Mutex::new(Mixer {
        voices: Vec::new(),
        sample_rate: supported.sample_rate().0 as f32,
    }));
    let mut config: cpal::StreamConfig = supported.clone().into();
    config.buffer_size = cpal::BufferSize::Fixed(512);
    let stream = match build_stream(&device, supported.sample_format(), &config, mixer.clone()) {
        Ok(stream) => stream,
        Err(_) => {
            config.buffer_size = cpal::BufferSize::Default;
            build_stream(&device, supported.sample_format(), &config, mixer.clone())?
        }
    };
    stream.play().map_err(|err| err.to_string())?;
    Ok((mixer, stream))
}

fn build_stream(
    device: &cpal::Device,
    format: SampleFormat,
    config: &cpal::StreamConfig,
    mixer: Arc<Mutex<Mixer>>,
) -> Result<cpal::Stream, String> {
    let channels = config.channels as usize;
    let result = match format {
        SampleFormat::F32 => {
            let mixer = mixer.clone();
            device.build_output_stream(
                config,
                move |data: &mut [f32], _| mix_into(data, channels, &mixer),
                report_audio_error,
                None,
            )
        }
        SampleFormat::I16 => {
            let mixer = mixer.clone();
            device.build_output_stream(
                config,
                move |data: &mut [i16], _| mix_into(data, channels, &mixer),
                report_audio_error,
                None,
            )
        }
        SampleFormat::U16 => {
            let mixer = mixer.clone();
            device.build_output_stream(
                config,
                move |data: &mut [u16], _| mix_into(data, channels, &mixer),
                report_audio_error,
                None,
            )
        }
        other => return Err(format!("audio format {other} is not supported")),
    };
    result.map_err(|err| err.to_string())
}

fn report_audio_error(err: cpal::StreamError) {
    eprintln!("nav: audio: {err}");
}

fn mix_into<T>(data: &mut [T], channels: usize, mixer: &Mutex<Mixer>)
where
    T: Sample + FromSample<f32>,
{
    let Ok(mut mixer) = mixer.lock() else {
        return;
    };
    let channels = channels.max(1);
    for frame in data.chunks_mut(channels) {
        let sample = T::from_sample(mixer.next_sample());
        for channel in frame {
            *channel = sample;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(tap: Tap, pitch: f32) -> Vec<f32> {
        let rate = 48_000u32;
        let count = (seconds(tap) * rate as f32).ceil() as usize;
        (0..count)
            .map(|i| sample_at(tap, i as f32 / rate as f32, pitch))
            .collect()
    }

    fn energy(samples: &[f32]) -> f32 {
        samples.iter().map(|sample| sample * sample).sum()
    }

    #[test]
    fn each_tap_attacks_and_then_falls_silent() {
        for tap in [Tap::Open, Tap::Up, Tap::Miss, Tap::Hover] {
            let samples = render(tap, 0.4);
            let peak = samples.iter().fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
            assert!(peak > 0.04 && peak < 0.55, "{tap:?} peak {peak}");
            let split = samples.len() / 4;
            assert!(energy(&samples[..split]) > energy(&samples[samples.len() - split..]));
            assert!(samples[samples.len() - 1].abs() < 0.001, "{tap:?} should end in silence");
        }
    }

    #[test]
    fn the_three_taps_are_different_sounds() {
        let open = render(Tap::Open, 0.4);
        let up = render(Tap::Up, 0.4);
        let miss = render(Tap::Miss, 0.4);
        assert_ne!(open, up);
        assert_ne!(open, miss);
        assert!(open.len() < up.len(), "going up rings a little longer");
        assert!(miss.len() < open.len(), "an empty click is the short one");
        let hover = render(Tap::Hover, 0.4);
        assert!(hover.len() < miss.len(), "a mouse-over is the shortest tap");
        assert_ne!(hover, open);
        assert!(energy(&open) > energy(&hover));
        assert!(energy(&open) > energy(&miss));
        assert_ne!(render(Tap::Open, 0.1), render(Tap::Open, 0.9));
    }

    #[test]
    #[ignore = "plays the default audio device"]
    fn taps_reach_the_default_device() {
        let clicks = Clicks::start();
        assert!(clicks.is_live(), "the default output should open");
        clicks.play(Tap::Open, 0.25);
        std::thread::sleep(std::time::Duration::from_millis(180));
        clicks.play(Tap::Up, 0.5);
        std::thread::sleep(std::time::Duration::from_millis(180));
        clicks.play(Tap::Miss, 0.5);
        std::thread::sleep(std::time::Duration::from_millis(180));
        clicks.play(Tap::Hover, 0.7);
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}
