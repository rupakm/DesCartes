use std::fs;
use std::path::Path;

use crate::trace::Trace;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceFormat {
    Json,
    Postcard,
}

#[derive(Debug, Clone)]
pub struct TraceIoConfig {
    pub format: TraceFormat,
}

impl Default for TraceIoConfig {
    fn default() -> Self {
        Self {
            format: TraceFormat::Json,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TraceIoError {
    #[error("failed reading trace file: {0}")]
    Read(#[from] std::io::Error),

    #[error("failed decoding json trace: {0}")]
    JsonDecode(#[from] serde_json::Error),

    #[error("failed encoding postcard trace: {0}")]
    PostcardEncode(#[source] postcard::Error),

    #[error("failed decoding postcard trace: {0}")]
    PostcardDecode(#[source] postcard::Error),

    #[error("failed writing trace file: {0}")]
    Write(#[source] std::io::Error),
}

pub fn write_trace_to_path(
    path: impl AsRef<Path>,
    trace: &Trace,
    cfg: TraceIoConfig,
) -> Result<(), TraceIoError> {
    let bytes = match cfg.format {
        TraceFormat::Json => serde_json::to_vec_pretty(trace)?,
        TraceFormat::Postcard => {
            postcard::to_stdvec(trace).map_err(TraceIoError::PostcardEncode)?
        }
    };

    fs::write(path, bytes).map_err(TraceIoError::Write)
}

pub fn read_trace_from_path(
    path: impl AsRef<Path>,
    cfg: TraceIoConfig,
) -> Result<Trace, TraceIoError> {
    let bytes = fs::read(path)?;

    let trace = match cfg.format {
        TraceFormat::Json => serde_json::from_slice(&bytes)?,
        TraceFormat::Postcard => {
            postcard::from_bytes(&bytes).map_err(TraceIoError::PostcardDecode)?
        }
    };

    Ok(trace)
}
