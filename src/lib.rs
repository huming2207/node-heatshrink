#![deny(clippy::all)]

use heatshrink::Config;
use napi::Task;
use napi::bindgen_prelude::*;

#[macro_use]
extern crate napi_derive;

/// Maximum number of times to double the output buffer before giving up.
const MAX_DECODE_RETRIES: u32 = 8;

/// Cap the output buffer at 64 MB to prevent runaway allocations.
const MAX_DECODE_BUFFER: usize = 64 * 1024 * 1024;

/// Calculate the initial encode output buffer size.
/// Encoding generally produces output smaller than input, but we need
/// headroom for incompressible data where heatshrink may expand slightly.
fn encode_buffer_size(input_len: usize) -> usize {
  // At minimum 1 KB, otherwise 2x input to handle worst-case expansion
  (input_len * 2).max(1024)
}

/// Calculate the initial decode output buffer size.
/// Heatshrink-compressed data can expand significantly (10x+ for highly
/// compressible inputs like log files), so we start with a generous 4x.
fn decode_buffer_size(input_len: usize) -> usize {
  // At minimum 4 KB, otherwise 4x input as a reasonable starting point
  (input_len * 4).max(4096)
}

/// Attempt to decode with progressively larger buffers.
/// Starts at `initial_size` and doubles up to MAX_DECODE_RETRIES times,
/// capped at MAX_DECODE_BUFFER.
fn decode_with_retry(input: &[u8], initial_size: usize, config: &Config) -> std::result::Result<Vec<u8>, String> {
  let mut buf_size = initial_size;
  for _ in 0..MAX_DECODE_RETRIES {
    let mut output_buf: Vec<u8> = vec![0; buf_size];
    match heatshrink::decode(input, output_buf.as_mut_slice(), config) {
      Ok(out) => return Ok(Vec::from(out)),
      Err(_) => {
        buf_size *= 2;
        if buf_size > MAX_DECODE_BUFFER {
          break;
        }
      }
    }
  }
  Err(format!(
    "Output buffer exhausted after retries (last attempt: {} bytes, input: {} bytes)",
    buf_size.min(MAX_DECODE_BUFFER),
    input.len()
  ))
}

#[napi]
pub fn encode_sync(input: Buffer, window_size: u8, lookahead_size: u8) -> Result<Buffer> {
  let input_ref: &[u8] = input.as_ref();
  let config = match Config::new(window_size, lookahead_size) {
    Ok(cfg) => cfg,
    Err(err) => {
      return Err(Error::new(Status::GenericFailure, err));
    }
  };

  let mut output_buf: Vec<u8> = vec![0; encode_buffer_size(input.len())];
  match heatshrink::encode(input_ref, output_buf.as_mut_slice(), &config) {
    Ok(out) => Ok(Vec::from(out).into()),
    Err(err) => {
      Err(Error::new(Status::GenericFailure, format!("{:?}", err)))
    }
  }
}

#[napi]
pub fn decode_sync(input: Buffer, window_size: u8, lookahead_size: u8, output_buffer_size: Option<u32>) -> Result<Buffer> {
  let input_ref: &[u8] = input.as_ref();
  let config = match Config::new(window_size, lookahead_size) {
    Ok(cfg) => cfg,
    Err(err) => {
      return Err(Error::new(Status::GenericFailure, err));
    }
  };

  let initial_size = output_buffer_size
    .map(|s| s as usize)
    .unwrap_or_else(|| decode_buffer_size(input.len()));

  match decode_with_retry(input_ref, initial_size, &config) {
    Ok(out) => Ok(out.into()),
    Err(msg) => Err(Error::new(Status::GenericFailure, msg)),
  }
}

pub struct EncodeTask {
  pub(crate) input: Buffer,
  pub(crate) window_size: u8,
  pub(crate) lookahead_size: u8,
}

impl Task for EncodeTask {
  type Output = Vec<u8>;
  type JsValue = Buffer;

  fn compute(&mut self) -> Result<Self::Output> {
    let input_ref: &[u8] = self.input.as_ref();
    let config = match Config::new(self.window_size, self.lookahead_size) {
      Ok(cfg) => cfg,
      Err(err) => {
        return Err(Error::new(Status::GenericFailure, err));
      }
    };

    let mut output_buf: Vec<u8> = vec![0; encode_buffer_size(self.input.len())];
    match heatshrink::encode(input_ref, output_buf.as_mut_slice(), &config) {
      Ok(out) => Ok(Vec::from(out)),
      Err(err) => {
        Err(Error::new(Status::GenericFailure, format!("{:?}", err)))
      }
    }
  }

  fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
    Ok(output.into())
  }
}

pub struct DecodeTask {
  pub(crate) input: Buffer,
  pub(crate) window_size: u8,
  pub(crate) lookahead_size: u8,
  pub(crate) output_buffer_size: Option<u32>,
}

impl Task for DecodeTask {
  type Output = Vec<u8>;
  type JsValue = Buffer;

  fn compute(&mut self) -> Result<Self::Output> {
    let input_ref: &[u8] = self.input.as_ref();
    let config = match Config::new(self.window_size, self.lookahead_size) {
      Ok(cfg) => cfg,
      Err(err) => {
        return Err(Error::new(Status::GenericFailure, err));
      }
    };

    let initial_size = self.output_buffer_size
      .map(|s| s as usize)
      .unwrap_or_else(|| decode_buffer_size(self.input.len()));

    match decode_with_retry(input_ref, initial_size, &config) {
      Ok(out) => Ok(out),
      Err(msg) => Err(Error::new(Status::GenericFailure, msg)),
    }
  }

  fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
    Ok(output.into())
  }
}

#[napi(ts_return_type = "Promise<Buffer>")]
pub fn encode(input: Buffer, window_size: u8, lookahead_size: u8, signal: Option<AbortSignal>) -> Result<AsyncTask<EncodeTask>> {
  Ok(AsyncTask::with_optional_signal(EncodeTask { input, window_size, lookahead_size }, signal))
}

#[napi(ts_return_type = "Promise<Buffer>")]
pub fn decode(input: Buffer, window_size: u8, lookahead_size: u8, signal: Option<AbortSignal>, output_buffer_size: Option<u32>) -> Result<AsyncTask<DecodeTask>> {
  Ok(AsyncTask::with_optional_signal(DecodeTask { input, window_size, lookahead_size, output_buffer_size }, signal))
}
