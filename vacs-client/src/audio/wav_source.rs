use anyhow::{Context, Result, anyhow};
use std::time::Duration;
use vacs_audio::sources::AudioSource;

pub struct WavLoopSource {
    samples: Vec<f32>,
    input_sample_rate: f32,
    output_sample_rate: f32,
    output_channels: usize,
    position: f32,
    playing: bool,
    volume: f32,
    looping: bool,
    gap_samples: usize,
    silence_remaining: usize,
}

impl WavLoopSource {
    pub fn from_file_looping(
        path: &std::path::Path,
        output_sample_rate: f32,
        output_channels: usize,
        volume: f32,
    ) -> Result<Self> {
        Self::from_file(path, output_sample_rate, output_channels, volume, true, Some(Duration::from_secs(2)))
    }

    pub fn from_file_oneshot(
        path: &std::path::Path,
        output_sample_rate: f32,
        output_channels: usize,
        volume: f32,
    ) -> Result<Self> {
        Self::from_file(path, output_sample_rate, output_channels, volume, false, None)
    }

    fn from_file(
        path: &std::path::Path,
        output_sample_rate: f32,
        output_channels: usize,
        volume: f32,
        looping: bool,
        gap: Option<Duration>,
    ) -> Result<Self> {
        let mut reader = hound::WavReader::open(path)
            .with_context(|| format!("Failed to open WAV file at {}", path.display()))?;

        let spec = reader.spec();
        if spec.channels == 0 {
            return Err(anyhow!("WAV file has no channels"));
        }

        let mono = match (spec.sample_format, spec.bits_per_sample) {
            (hound::SampleFormat::Float, 32) => {
                let samples: Result<Vec<f32>, _> = reader.samples::<f32>().collect();
                Self::interleaved_to_mono(samples.context("Failed to decode WAV float samples")?, spec.channels as usize)
            }
            (hound::SampleFormat::Int, 16) => {
                let samples: Result<Vec<i16>, _> = reader.samples::<i16>().collect();
                let samples = samples
                    .context("Failed to decode WAV 16-bit PCM samples")?
                    .into_iter()
                    .map(|sample| sample as f32 / i16::MAX as f32)
                    .collect();
                Self::interleaved_to_mono(samples, spec.channels as usize)
            }
            _ => {
                return Err(anyhow!(
                    "Unsupported WAV format (expected 16-bit PCM or 32-bit float)"
                ));
            }
        };

        if mono.is_empty() {
            return Err(anyhow!("WAV file contains no audio samples"));
        }

        let gap_samples = gap
            .map(|d| (d.as_secs_f32() * output_sample_rate) as usize)
            .unwrap_or(0);

        Ok(Self {
            samples: mono,
            input_sample_rate: spec.sample_rate as f32,
            output_sample_rate,
            output_channels,
            position: 0.0,
            playing: false,
            volume,
            looping,
            gap_samples,
            silence_remaining: 0,
        })
    }

    fn interleaved_to_mono(interleaved: Vec<f32>, channels: usize) -> Vec<f32> {
        if channels == 1 {
            return interleaved;
        }

        interleaved
            .chunks(channels)
            .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
            .collect()
    }

    fn next_sample(&mut self) -> f32 {
        let len = self.samples.len();
        if len == 0 {
            return 0.0;
        }

        // Currently in the silence gap between loops
        if self.silence_remaining > 0 {
            self.silence_remaining -= 1;
            return 0.0;
        }

        if self.position >= len as f32 {
            if !self.looping {
                self.playing = false;
                self.position = 0.0;
                return 0.0;
            }

            while self.position >= len as f32 {
                self.position -= len as f32;
            }

            // Start the silence gap before the next loop iteration
            if self.gap_samples > 0 {
                self.silence_remaining = self.gap_samples;
                return 0.0;
            }
        }

        let index = self.position as usize;
        let next_index = if index + 1 < len { index + 1 } else { 0 };
        let frac = self.position - index as f32;

        let current = self.samples[index];
        let next = self.samples[next_index];
        let sample = current + (next - current) * frac;

        self.position += self.input_sample_rate / self.output_sample_rate;

        sample * self.volume
    }
}

impl AudioSource for WavLoopSource {
    fn mix_into(&mut self, output: &mut [f32]) {
        if !self.playing || self.output_channels == 0 {
            return;
        }

        for frame in output.chunks_exact_mut(self.output_channels) {
            let sample = self.next_sample();
            for channel in frame {
                *channel += sample;
            }
        }
    }

    fn start(&mut self) {
        self.playing = true;
    }

    fn stop(&mut self) {
        self.playing = false;
        self.position = 0.0;
        self.silence_remaining = 0;
    }

    fn restart(&mut self) {
        self.position = 0.0;
        self.playing = true;
        self.silence_remaining = 0;
    }

    fn set_volume(&mut self, volume: f32) {
        self.volume = volume;
    }
}