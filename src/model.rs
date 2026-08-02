use serde::Serialize;

/// One 100 ms loudness measurement frame, as emitted by the ffmpeg
/// `ebur128` + `ametadata` chain.
#[derive(Debug, Clone)]
pub struct MeasurePoint {
    /// Time in seconds (from `pts_time`).
    pub t: f64,
    /// Momentary loudness in LUFS (`f64::NEG_INFINITY` when the source reported `nan`).
    pub m: f64,
    /// Integrated loudness in LUFS.
    pub i: f64,
    /// Remaining raw `lavfi.r128.*` lines following the `I=` line of this frame
    /// (holds `LRA=` and `true_peaks_chN=` on the final frame).
    pub rest: Vec<String>,
}

/// Numeric cue/loudness analysis result. This is the structured form used by
/// the library; use [`CueResult::output`] for the stringified JSON form that
/// matches the original `cue_file` output.
#[derive(Debug, Clone)]
pub struct CueResult {
    /// Real file duration in seconds.
    pub duration: f64,
    /// Playout duration (cue-in to cue-out), in seconds.
    pub cue_duration: f64,
    /// Cue-in point in seconds.
    pub cue_in: f64,
    /// Cue-out point in seconds.
    pub cue_out: f64,
    /// Suggested start point of the next track, in seconds.
    pub cross_start_next: f64,
    /// Whether the track has a long fade-out.
    pub longtail: bool,
    /// Whether the track has a sustained ending.
    pub sustained_ending: bool,
    /// EBU R128 integrated loudness, in LUFS.
    pub loudness: f64,
    /// Loudness range (dynamics), in LU.
    pub loudness_range: f64,
    /// "ReplayGain"-like gain relative to the target, in dB.
    pub amplify: f64,
    /// Gain adjustment applied by clipping prevention, in dB.
    pub amplify_adjustment: f64,
    /// Reference loudness target used, in LUFS.
    pub reference_loudness: f64,
    /// Blank-skip threshold used, in seconds.
    pub blankskip: f64,
    /// Whether the cue-out was moved early because of internal silence.
    pub blank_skipped: bool,
    /// Linear true peak.
    pub true_peak: f64,
    /// True peak in dBFS.
    pub true_peak_db: f64,
}

impl CueResult {
    /// Produces the stringified JSON form identical to the original `cue_file`.
    pub fn output(&self) -> CueOutput {
        CueOutput {
            duration: self.duration,
            liq_cue_duration: self.cue_duration,
            liq_cue_in: self.cue_in,
            liq_cue_out: self.cue_out,
            liq_cross_start_next: self.cross_start_next,
            liq_longtail: self.longtail,
            liq_sustained_ending: self.sustained_ending,
            liq_loudness: format!("{:.2} LUFS", self.loudness),
            liq_loudness_range: format!("{:.2} LU", self.loudness_range),
            liq_amplify: format!("{:.2} dB", self.amplify),
            liq_amplify_adjustment: format!("{:.2} dB", self.amplify_adjustment),
            liq_reference_loudness: format!("{:.2} LUFS", self.reference_loudness),
            liq_blankskip: self.blankskip,
            liq_blank_skipped: self.blank_skipped,
            liq_true_peak: self.true_peak,
            liq_true_peak_db: format!("{:.2} dBFS", self.true_peak_db),
        }
    }
}

/// The stringified JSON representation of a [`CueResult`], matching the
/// original `cue_file` stdout contract.
#[derive(Debug, Clone, Serialize)]
pub struct CueOutput {
    pub duration: f64,
    pub liq_cue_duration: f64,
    pub liq_cue_in: f64,
    pub liq_cue_out: f64,
    pub liq_cross_start_next: f64,
    pub liq_longtail: bool,
    pub liq_sustained_ending: bool,
    pub liq_loudness: String,
    pub liq_loudness_range: String,
    pub liq_amplify: String,
    pub liq_amplify_adjustment: String,
    pub liq_reference_loudness: String,
    pub liq_blankskip: f64,
    pub liq_blank_skipped: bool,
    pub liq_true_peak: f64,
    pub liq_true_peak_db: String,
}

/// Human-readable diagnostics produced during analysis (printed to stderr by
/// the CLI, mirroring the original tool).
#[derive(Debug, Clone)]
pub struct Diagnostics {
    /// Overlay loudness level, in LUFS (`loudness + overlay`).
    pub overlay_level: f64,
    /// Long-tail loudness level, in LUFS (`loudness + overlay + extra`).
    pub longtail_level: f64,
    /// Overlay start from the normal calculation, in seconds.
    pub normal_start: f64,
    /// Overlay start from the sustained-ending calculation, in seconds.
    pub sustained_start: f64,
    /// Overlay start from the long-tail calculation, in seconds.
    pub longtail_start: f64,
    /// The chosen overlay start, in seconds.
    pub used_start: f64,
    /// Cue-out time, in seconds.
    pub cue_out_time: f64,
    /// Average momentary loudness of the measured ending, in LUFS.
    pub end_avg_lufs: f64,
    /// Ending loudness drop ratio, in percent.
    pub drop_pct: f64,
}

/// Full result of an analysis: the structured cues plus diagnostics.
#[derive(Debug, Clone)]
pub struct AnalysisResult {
    pub cues: CueResult,
    pub diagnostics: Diagnostics,
}
