/// Analysis options, mirroring the `cue_file` command-line parameters.
#[derive(Debug, Clone)]
pub struct CueOptions {
    /// EBU R128 reference loudness target, in LUFS (-23.0 to 0.0).
    pub target_lufs: f64,
    /// LU below integrated track loudness for cue-in/cue-out detection ("silence").
    pub silence_lu: f64,
    /// LU below integrated track loudness to trigger the next track ("overlay").
    pub overlay_lu: f64,
    /// Overlay durations longer than this (in seconds) are considered a long tail
    /// and trigger a recalculation using [`CueOptions::longtail_extra_lu`].
    pub longtail_seconds: f64,
    /// Extra LU below the overlay loudness used for long-tail recalculation.
    pub longtail_extra_lu: f64,
    /// Maximum percent loudness drop at the end to still be considered a
    /// "sustained ending". Such tracks are recalculated using the long-tail level.
    /// `0.0` disables sustained-ending detection.
    pub sustained_drop_pct: f64,
    /// Skip blank (silence) within a track longer than this many seconds
    /// ("hidden tracks"). `0.0` disables blank skipping.
    pub blankskip_seconds: f64,
    /// Lower track gain if needed so true peaks do not exceed -1 dBFS.
    pub noclip: bool,
    /// Wrap the ffmpeg measurement in `nice -n 18` (Linux/macOS only).
    pub nice: bool,
    /// Path to the ffmpeg binary.
    pub ffmpeg_path: String,
    /// Path to the ffprobe binary.
    pub ffprobe_path: String,
}

impl Default for CueOptions {
    fn default() -> Self {
        Self {
            target_lufs: -18.0,
            silence_lu: -42.0,
            overlay_lu: -8.0,
            longtail_seconds: 15.0,
            longtail_extra_lu: -12.0,
            sustained_drop_pct: 40.0,
            blankskip_seconds: 0.0,
            noclip: false,
            nice: false,
            ffmpeg_path: "ffmpeg".to_string(),
            ffprobe_path: "ffprobe".to_string(),
        }
    }
}
