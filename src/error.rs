use std::path::PathBuf;

/// Errors produced by the `autocue-rs` crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("ffmpeg executable `{0}` was not found on PATH")]
    FfmpegNotFound(String),
    #[error("ffprobe executable `{0}` was not found on PATH")]
    FfprobeNotFound(String),
    #[error("ffmpeg failed with exit status {status:?}: {message}")]
    FfmpegFailed { status: Option<i32>, message: String },
    #[error("failed to analyse `{path}`: {message}")]
    Analysis { path: PathBuf, message: String },
    #[error("audio file `{path}` produced no measurable audio frames")]
    NoMeasurements { path: PathBuf },
    #[error("failed to read tags from `{path}`: {source}")]
    ReadTags {
        path: PathBuf,
        #[source]
        source: lofty::error::LoftyError,
    },
    #[error("failed to write tags to `{path}`: {source}")]
    WriteTags {
        path: PathBuf,
        #[source]
        source: lofty::error::LoftyError,
    },
    #[error("unsupported audio container for tag writing: `{path}`")]
    UnsupportedContainer { path: PathBuf },
    #[error("file `{path}` is not writable")]
    NotWritable { path: PathBuf },
    #[error("I/O error on `{path}`: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub type Result<T> = std::result::Result<T, Error>;
