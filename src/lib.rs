//! On-the-fly cue-in, cue-out, overlay and EBU R128 loudness analysis for
//! AutoDJ software. A Rust port of the original `autocue`/`cue_file` tool.
//!
//! The analysis pipeline is:
//!
//! 1. [`tags::read_tags_ffprobe`] reads existing tags via `ffprobe`; if enough
//!    data is present, a re-analysis can be avoided.
//! 2. [`measure::measure`] runs `ffmpeg`'s `ebur128` + `ametadata` chain and
//!    returns per-100-ms loudness frames.
//! 3. [`cue_engine::compute_cues`] computes cue-in, cue-out, overlay and
//!    loudness values (the pure port of the original algorithm).
//! 4. [`tags::write_tags`] optionally writes the `liq_*`/ReplayGain tags back
//!    to the file (via `lofty`, or `ffmpeg` for the file types ffmpeg handles
//!    safely).
//!
//! The high-level entry point is [`analysis::run`].

pub mod analysis;
pub mod cue_engine;
pub mod error;
pub mod measure;
pub mod model;
pub mod options;
pub mod tags;

pub use analysis::{run, ReadContext, ReadResult, RunResult, TagMap, TagValue};
pub use cue_engine::compute_cues;
pub use error::{Error, Result};
pub use measure::measure;
pub use model::{AnalysisResult, CueOutput, CueResult, Diagnostics, MeasurePoint};
pub use options::CueOptions;
