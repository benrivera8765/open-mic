// SPDX-License-Identifier: GPL-3.0-or-later
//! Song-rate to device-rate conversion, used **only** when the two differ
//! (WORK_ORDER section 5: resample at most once, with a top-quality resampler).
//!
//! Engine: `rubato` asynchronous **windowed-sinc** resampler, 256-tap sinc,
//! Blackman-Harris^2 window, 256x oversampled table, cubic interpolation between table
//! points, automatic cutoff (rubato's `calculate_cutoff`). Fixed ratio
//! (`out_rate / in_rate`); the ratio is never adjusted.
//!
//! Measured quality and cost: docs/benchmarks/dsp.md ("output resampler").

use crate::graph::DspError;
use crate::params::CHANNELS;
use rubato::audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{
    Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

pub const SINC_LEN: usize = 256;
pub const OVERSAMPLING: usize = 256;

/// Fixed-input-chunk resampler (or a pass-through when the rates are equal).
pub struct OutputResampler {
    in_rate: u32,
    out_rate: u32,
    chunk: usize,
    inner: Option<Async<f32>>,
    in_buf: Vec<Vec<f32>>,
    out_buf: Vec<Vec<f32>>,
}

impl OutputResampler {
    /// `chunk`: input frames per call to [`process`](Self::process).
    pub fn new(in_rate: u32, out_rate: u32, chunk: usize) -> Result<Self, DspError> {
        if in_rate == 0 || out_rate == 0 || chunk == 0 {
            return Err(DspError::Invalid("resampler rates and chunk must be > 0"));
        }
        if in_rate == out_rate {
            return Ok(OutputResampler {
                in_rate,
                out_rate,
                chunk,
                inner: None,
                in_buf: Vec::new(),
                out_buf: Vec::new(),
            });
        }
        let params = SincInterpolationParameters::new(SINC_LEN, WindowFunction::BlackmanHarris2)
            .oversampling_factor(OVERSAMPLING)
            .interpolation(SincInterpolationType::Cubic);
        let ratio = out_rate as f64 / in_rate as f64;
        let rs = Async::<f32>::new_sinc(ratio, 1.0, &params, chunk, CHANNELS, FixedAsync::Input)
            .map_err(|e| DspError::Resampler(e.to_string()))?;
        let out_max = rs.output_frames_max();
        Ok(OutputResampler {
            in_rate,
            out_rate,
            chunk,
            inner: Some(rs),
            in_buf: vec![vec![0.0; chunk]; CHANNELS],
            out_buf: vec![vec![0.0; out_max]; CHANNELS],
        })
    }

    pub fn is_passthrough(&self) -> bool {
        self.inner.is_none()
    }

    pub fn in_rate(&self) -> u32 {
        self.in_rate
    }

    pub fn out_rate(&self) -> u32 {
        self.out_rate
    }

    pub fn chunk(&self) -> usize {
        self.chunk
    }

    /// Delay added by the resampler, in **output** frames (0 for pass-through).
    pub fn output_delay(&self) -> usize {
        self.inner.as_ref().map_or(0, |r| r.output_delay())
    }

    /// Most output frames one call can produce.
    pub fn max_output_frames(&self) -> usize {
        self.inner.as_ref().map_or(self.chunk, |r| r.output_frames_max())
    }

    pub fn reset(&mut self) {
        if let Some(r) = self.inner.as_mut() {
            r.reset();
        }
    }

    /// Convert exactly `chunk` input frames. Returns the produced output as planar slices
    /// (valid until the next call). For pass-through the input is copied.
    pub fn process(&mut self, input: [&[f32]; CHANNELS]) -> Result<[&[f32]; CHANNELS], DspError> {
        let chunk = self.chunk;
        match self.inner.as_mut() {
            None => {
                if self.in_buf.is_empty() {
                    self.in_buf = vec![vec![0.0; chunk]; CHANNELS];
                }
                for c in 0..CHANNELS {
                    self.in_buf[c][..chunk].copy_from_slice(&input[c][..chunk]);
                }
                Ok([&self.in_buf[0][..chunk], &self.in_buf[1][..chunk]])
            }
            Some(rs) => {
                for c in 0..CHANNELS {
                    self.in_buf[c][..chunk].copy_from_slice(&input[c][..chunk]);
                }
                let out_cap = self.out_buf[0].len();
                let inp = SequentialSliceOfVecs::new(&self.in_buf, CHANNELS, chunk)
                    .map_err(|e| DspError::Resampler(e.to_string()))?;
                let mut outp = SequentialSliceOfVecs::new_mut(&mut self.out_buf, CHANNELS, out_cap)
                    .map_err(|e| DspError::Resampler(e.to_string()))?;
                let (_read, written) = rs
                    .process_into_buffer(&inp, &mut outp, None)
                    .map_err(|e| DspError::Resampler(e.to_string()))?;
                Ok([&self.out_buf[0][..written], &self.out_buf[1][..written]])
            }
        }
    }
}
