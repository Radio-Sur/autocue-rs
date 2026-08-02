use crate::error::{Error, Result};
use crate::model::{CueResult, Diagnostics, MeasurePoint};
use crate::options::CueOptions;

/// Computes cue-in, cue-out, overlay and loudness data from a list of
/// measurement points. This is a pure port of the `analyse()` function of the
/// original `cue_file`.
pub fn compute_cues(measure: &[MeasurePoint], opts: &CueOptions) -> Result<(CueResult, Diagnostics)> {
    let end_len = measure.len();
    if end_len == 0 {
        return Err(Error::NoMeasurements {
            path: std::path::PathBuf::from("<input>"),
        });
    }

    // Duration from the last PTS plus one frame length (100 ms).
    let duration = measure[end_len - 1].t + 0.1;
    let loudness = measure[end_len - 1].i;

    // True peak (highest channel) and LRA from the last frame's remaining lines.
    let mut true_peak: f64 = 0.0;
    let mut loudness_range: f64 = 0.0;
    for line in &measure[end_len - 1].rest {
        if let Some(rest) = line.strip_prefix("lavfi.r128.true_peaks_ch") {
            if let Some((_, v)) = rest.split_once('=') {
                if let Ok(f) = v.trim().parse::<f64>() {
                    true_peak = true_peak.max(f);
                }
            }
        } else if let Some(v) = line.strip_prefix("lavfi.r128.LRA=") {
            if let Ok(f) = v.trim().parse::<f64>() {
                loudness_range = f;
            }
        }
    }
    let true_peak_db = if true_peak > 0.0 {
        20.0 * true_peak.log10()
    } else {
        f64::NEG_INFINITY
    };

    let silence_level = loudness + opts.silence_lu;

    let mut start = 0usize;
    let mut cue_in_time = 0.0;
    for (i, p) in measure.iter().enumerate() {
        if p.m > silence_level {
            cue_in_time = p.t;
            start = i;
            break;
        }
    }
    // EBU R.128 measures over the last 400 ms; start at zero if we land before that.
    if cue_in_time < 0.4 {
        cue_in_time = 0.0;
    }

    // Blank-skip: find internal silence of the configured length ("hidden tracks").
    let mut end = end_len;
    let mut end_blank = end_len;
    let mut cue_out_time_blank = 0.0;
    if opts.blankskip_seconds != 0.0 {
        let mut i = start;
        while i < end {
            if measure[i].m <= silence_level {
                let blank_start = measure[i].t;
                let blank_stop = measure[i].t + opts.blankskip_seconds;
                end_blank = i + 1;
                while i < end && measure[i].m <= silence_level && measure[i].t <= blank_stop {
                    i += 1;
                }
                if i >= end {
                    // Ran into the end of the track; blank not long enough.
                    end_blank = end;
                    break;
                }
                if measure[i].t >= blank_stop {
                    // Silence long enough; cue out at its beginning.
                    cue_out_time_blank = blank_start;
                    break;
                }
                i += 1;
            } else {
                i += 1;
            }
        }
    }

    let mut cue_out_time = 0.0;
    for i in (start..end).rev() {
        if measure[i].m > silence_level {
            cue_out_time = measure[i].t;
            end = i + 1;
            break;
        }
    }
    cue_out_time = cue_out_time.max(duration - cue_out_time);

    // Adjust cue-out if a blank was found; flag early cue-out.
    let mut blank_skipped = false;
    if opts.blankskip_seconds != 0.0 {
        if cue_out_time_blank > 0.0 && cue_out_time_blank < cue_out_time {
            cue_out_time = cue_out_time_blank;
            blank_skipped = true;
        }
        end = end_blank;
    }

    // Overlay: where the next track should start, searching backwards.
    let cue_duration = cue_out_time - cue_in_time;
    let mut start_next_level = loudness + opts.overlay_lu;
    let mut start_next_time = 0.0;
    let mut start_next_idx = end;
    for i in (start..end).rev() {
        if measure[i].m > start_next_level {
            start_next_time = measure[i].t;
            start_next_idx = i;
            break;
        }
    }
    start_next_time = start_next_time.max(cue_out_time - start_next_time);

    let mut sustained = false;
    let mut start_next_time_sustained = 0.0;
    let mut end_avg_lufs = 0.0;
    let mut drop_pct = 0.0;
    if start_next_idx < end {
        let (_mid_time, _mid_lufs, _angle, lufs_ratio_pct, end_lufs) = calc_ending(&measure[start_next_idx..end]);
        end_avg_lufs = end_lufs;
        drop_pct = lufs_ratio_pct;
        if lufs_ratio_pct < opts.sustained_drop_pct {
            sustained = true;
            start_next_level = end_lufs.max(loudness + opts.overlay_lu + opts.longtail_extra_lu);
            for i in (start..end).rev() {
                if measure[i].m > start_next_level {
                    start_next_time_sustained = measure[i].t;
                    break;
                }
            }
            start_next_time_sustained = start_next_time_sustained.max(cue_out_time - start_next_time_sustained);
        }
    }

    let mut longtail = false;
    let mut start_next_time_longtail = 0.0;
    if cue_out_time - start_next_time > opts.longtail_seconds {
        longtail = true;
        start_next_level = loudness + opts.overlay_lu + opts.longtail_extra_lu;
        for i in (start..end).rev() {
            if measure[i].m > start_next_level {
                start_next_time_longtail = measure[i].t;
                break;
            }
        }
        start_next_time_longtail = start_next_time_longtail.max(cue_out_time - start_next_time_longtail);
    }

    let used_start = start_next_time.max(start_next_time_sustained).max(start_next_time_longtail);

    // Amplify (ReplayGain-like) with optional clipping prevention.
    let mut amplify_adjustment = 0.0;
    let amplify = if opts.noclip {
        let amp = opts.target_lufs - loudness;
        let max_amp = -1.0 - true_peak_db;
        if amp > max_amp {
            amplify_adjustment = max_amp - amp;
            max_amp
        } else {
            amp
        }
    } else {
        opts.target_lufs - loudness
    };

    let cues = CueResult {
        duration,
        cue_duration,
        cue_in: cue_in_time,
        cue_out: cue_out_time,
        cross_start_next: used_start,
        longtail,
        sustained_ending: sustained,
        loudness,
        loudness_range,
        amplify,
        amplify_adjustment,
        reference_loudness: opts.target_lufs,
        blankskip: opts.blankskip_seconds,
        blank_skipped,
        true_peak,
        true_peak_db,
    };

    let diagnostics = Diagnostics {
        overlay_level: loudness + opts.overlay_lu,
        longtail_level: loudness + opts.overlay_lu + opts.longtail_extra_lu,
        normal_start: start_next_time,
        sustained_start: start_next_time_sustained,
        longtail_start: start_next_time_longtail,
        used_start,
        cue_out_time,
        end_avg_lufs,
        drop_pct,
    };

    Ok((cues, diagnostics))
}

/// Calculates the loudness drop over a run of measure points, splitting them
/// into left and right halves. Returns `(mid_time, mid_lufs, angle, ratio_pct,
/// end_lufs)`, mirroring the original `calc_ending()`.
fn calc_ending(elements: &[MeasurePoint]) -> (f64, f64, f64, f64, f64) {
    let l = elements.len();
    let l2 = l / 2;
    let (p1, p2) = if l >= 2 {
        (&elements[..l2], &elements[l2 + l % 2..])
    } else {
        (elements, elements)
    };

    let mid_time = elements[l2].t;
    let mut x1 = p1.iter().map(|e| e.t).sum::<f64>();
    let mut x2 = p2.iter().map(|e| e.t).sum::<f64>();
    let mut y1 = p1.iter().map(|e| e.m).sum::<f64>();
    let mut y2 = p2.iter().map(|e| e.m).sum::<f64>();
    if l2 > 0 {
        x1 /= p1.len() as f64;
        x2 /= p2.len() as f64;
        y1 /= p1.len() as f64;
        y2 /= p2.len() as f64;
    }
    let dx = x2 - x1;
    let dy = y2 - y1;
    let angle = dy.atan2(dx).to_degrees();
    let mid_lufs = elements[l2].m;
    let lufs_ratio_pct = if y2 != 0.0 { (1.0 - y1 / y2) * 100.0 } else { f64::NEG_INFINITY };

    (mid_time, mid_lufs, angle, lufs_ratio_pct, y2)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds `n` frames of constant momentary loudness, 0.1 s apart, using an
    /// exact decimal `t` to avoid float accumulation from `start + i*0.1`.
    fn tone(n: usize, m: f64, start_t: f64) -> Vec<MeasurePoint> {
        (0..n)
            .map(|i| MeasurePoint {
                t: (start_t * 10.0 + i as f64) / 10.0,
                m,
                i: f64::NAN,
                rest: Vec::new(),
            })
            .collect()
    }

    fn finalize(mut pts: Vec<MeasurePoint>, loudness: f64, lra: f64, true_peak: f64) -> Vec<MeasurePoint> {
        let last = pts.last_mut().unwrap();
        last.i = loudness;
        last.rest = vec![format!("lavfi.r128.LRA={lra}"), format!("lavfi.r128.true_peaks_ch1={true_peak}")];
        pts
    }

    fn cues(pts: Vec<MeasurePoint>, opts: &CueOptions) -> CueResult {
        compute_cues(&pts, opts).unwrap().0
    }

    fn assert_close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "expected {b}, got {a}");
    }

    #[test]
    fn decaying_tail_is_not_sustained() {
        let mut pts = tone(25, -20.0, 0.0); // t=0.0..2.4
        for (i, m) in [-40.0, -50.0, -62.0, -70.0, -80.0].iter().enumerate() {
            pts.push(MeasurePoint {
                t: (25.0 + i as f64) / 10.0,
                m: *m,
                i: f64::NAN,
                rest: Vec::new(),
            });
        }
        let pts = finalize(pts, -20.0, 2.5, 0.10);

        let c = cues(pts, &CueOptions::default());
        assert_close(c.duration, 3.0);
        assert_close(c.cue_in, 0.0);
        assert_close(c.cue_out, 2.6); // last frame above the -62 LUFS silence level
        assert_close(c.cue_duration, 2.6);
        assert_close(c.loudness, -20.0);
        assert_close(c.loudness_range, 2.5);
        assert_close(c.amplify, 2.0);
        assert_close(c.cross_start_next, 2.4);
        assert!(!c.sustained_ending);
        assert!(!c.longtail);
        assert!(!c.blank_skipped);
        assert_close(c.true_peak, 0.10);
        assert_close(c.true_peak_db, -20.0);
    }

    #[test]
    fn constant_tone_is_sustained() {
        let pts = finalize(tone(20, -20.0, 0.0), -20.0, 1.0, 0.5);
        let c = cues(pts, &CueOptions::default());
        assert_close(c.duration, 2.0);
        assert_close(c.cue_out, 1.9);
        assert_close(c.cue_duration, 1.9);
        assert!(c.sustained_ending);
        assert!(!c.longtail);
        assert_close(c.cross_start_next, 1.9);
    }

    #[test]
    fn silence_at_start_sets_cue_in() {
        let mut pts = tone(4, f64::NEG_INFINITY, 0.0); // t=0.0..0.3
        pts.extend(tone(16, -20.0, 0.4)); // t=0.4..1.9
        let pts = finalize(pts, -20.0, 1.0, 0.5);
        let c = cues(pts, &CueOptions::default());
        assert_close(c.cue_in, 0.4);
        assert_close(c.cue_out, 1.9);
    }

    #[test]
    fn lead_in_shorter_than_ebur_window_rounds_cue_in_to_zero() {
        let pts = finalize(tone(20, -20.0, 0.0), -20.0, 1.0, 0.5);
        let c = cues(pts, &CueOptions::default());
        assert_close(c.cue_in, 0.0); // first loud frame at t=0.0 < 0.4
    }

    #[test]
    fn blankskip_moves_cue_out_to_internal_silence() {
        let mut pts = tone(16, -20.0, 0.0); // t=0.0..1.5
        pts.extend(tone(60, f64::NEG_INFINITY, 1.6)); // t=1.6..7.5
        pts.extend(tone(10, -20.0, 7.6)); // t=7.6..8.5
        let pts = finalize(pts, -20.0, 1.0, 0.5);

        let opts = CueOptions {
            blankskip_seconds: 5.0,
            ..CueOptions::default()
        };
        let c = cues(pts, &opts);
        assert!(c.blank_skipped);
        assert_close(c.cue_out, 1.6);
        assert_close(c.cue_duration, 1.6);
        // Overlay sits on the last loud frame before the blank.
        assert_close(c.cross_start_next, 1.5);
    }

    #[test]
    fn blankskip_ignored_when_silence_too_short() {
        let mut pts = tone(16, -20.0, 0.0); // t=0.0..1.5
        pts.extend(tone(30, f64::NEG_INFINITY, 1.6)); // only 3.0s of silence
        pts.extend(tone(10, -20.0, 4.6)); // t=4.6..5.5
        let pts = finalize(pts, -20.0, 1.0, 0.5);

        let opts = CueOptions {
            blankskip_seconds: 5.0,
            ..CueOptions::default()
        };
        let c = cues(pts, &opts);
        assert!(!c.blank_skipped);
        assert_close(c.cue_out, 5.5);
    }

    #[test]
    fn noclip_caps_amplify_and_reports_adjustment() {
        let pts = finalize(tone(20, -20.0, 0.0), -20.0, 1.0, 1.0);
        let opts = CueOptions {
            noclip: true,
            ..CueOptions::default()
        };
        let c = cues(pts, &opts);
        // true_peak=1.0 -> 0 dBFS -> max_amp = -1.0; amp = 2.0 clipped to -1.0.
        assert_close(c.amplify, -1.0);
        assert_close(c.amplify_adjustment, -3.0);
    }

    #[test]
    fn silent_file_has_no_measurements_error() {
        let opts = CueOptions::default();
        assert!(compute_cues(&[], &opts).is_err());
    }
}
