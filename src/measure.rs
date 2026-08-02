use std::process::Command;

use crate::error::{Error, Result};
use crate::model::MeasurePoint;
use crate::options::CueOptions;

/// Runs ffmpeg's `ebur128` + `ametadata` chain over the input file and parses
/// the per-100-ms loudness frames, mirroring the original `cue_file`.
pub fn measure(path: &std::path::Path, opts: &CueOptions) -> Result<Vec<MeasurePoint>> {
    let filter = format!(
        "ebur128=target={}:peak=true:metadata=1,ametadata=mode=print:file=-",
        opts.target_lufs
    );

    let mut cmd = Command::new(&opts.ffmpeg_path);
    if opts.nice {
        cmd.arg("-n").arg("18");
        cmd.args([
            &opts.ffmpeg_path,
            "-v",
            "quiet",
            "-nostdin",
            "-y",
            "-i",
            path.to_string_lossy().as_ref(),
            "-vn",
            "-af",
            &filter,
            "-f",
            "null",
            "null",
        ]);
    } else {
        cmd.args([
            "-v",
            "quiet",
            "-nostdin",
            "-y",
            "-i",
            path.to_string_lossy().as_ref(),
            "-vn",
            "-af",
            &filter,
            "-f",
            "null",
            "null",
        ]);
    }

    let output = cmd.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Error::FfmpegNotFound(opts.ffmpeg_path.clone())
        } else {
            Error::Analysis {
                path: path.to_path_buf(),
                message: format!("failed to spawn ffmpeg: {e}"),
            }
        }
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail = stderr.lines().rev().take(5).collect::<Vec<_>>().join("\n");
        return Err(Error::FfmpegFailed {
            status: output.status.code(),
            message: tail,
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse(stdout.as_ref()))
}

/// Parses the ffmpeg `ametadata=mode=print:file=-` output into measurement
/// points `(t, m, i, rest)`. `nan` momentary loudness is converted to
/// `f64::NEG_INFINITY` for comparability in silence detection, exactly like
/// the original.
pub fn parse(stdout: &str) -> Vec<MeasurePoint> {
    let mut points: Vec<MeasurePoint> = Vec::new();
    let mut current: Option<MeasurePoint> = None;

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed.starts_with("frame:") {
            if let Some(prev) = current.take() {
                points.push(prev);
            }
            let t = extract_pts_time(trimmed).unwrap_or(0.0);
            current = Some(MeasurePoint {
                t,
                m: f64::NEG_INFINITY,
                i: f64::NAN,
                rest: Vec::new(),
            });
        } else if let Some(rest) = trimmed.strip_prefix("lavfi.r128.M=") {
            if let Some(point) = current.as_mut() {
                point.m = parse_lufs(rest, true);
            }
        } else if trimmed.starts_with("lavfi.r128.S=") {
            // Short-term loudness, not used by the algorithm.
        } else if let Some(rest) = trimmed.strip_prefix("lavfi.r128.I=") {
            if let Some(point) = current.as_mut() {
                point.i = parse_lufs(rest, false);
            }
        } else if let Some(point) = current.as_mut() {
            point.rest.push(trimmed.to_string());
        }
    }

    if let Some(prev) = current.take() {
        points.push(prev);
    }

    points
}

fn extract_pts_time(line: &str) -> Option<f64> {
    let idx = line.find("pts_time:")?;
    let value = &line[idx + "pts_time:".len()..];
    let value: String = value.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    value.parse::<f64>().ok()
}

fn parse_lufs(value: &str, momentary: bool) -> f64 {
    let v = value.trim();
    if v == "nan" {
        if momentary {
            f64::NEG_INFINITY
        } else {
            f64::NAN
        }
    } else {
        v.parse::<f64>().unwrap_or({
            if momentary {
                f64::NEG_INFINITY
            } else {
                f64::NAN
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "expected {b}, got {a}");
    }

    const SAMPLE: &str = "\
frame:0    pts:0       pts_time:0
lavfi.r128.M=-91.0
lavfi.r128.S=-91.0
lavfi.r128.I=-91.0
frame:1    pts:4800    pts_time:0.1
lavfi.r128.M=-20.0
lavfi.r128.S=-20.0
lavfi.r128.I=-20.0
lavfi.r128.LRA=2.5
lavfi.r128.true_peaks_ch1=0.5
";

    #[test]
    fn parses_frames_and_preserves_rest_lines() {
        let pts = parse(SAMPLE);
        assert_eq!(pts.len(), 2);
        assert_close(pts[0].t, 0.0);
        assert_close(pts[0].m, -91.0);
        assert_close(pts[0].i, -91.0);
        assert_close(pts[1].t, 0.1);
        assert_close(pts[1].m, -20.0);
        assert_close(pts[1].i, -20.0);
        assert_eq!(pts[1].rest.len(), 2);
        assert_eq!(pts[1].rest[0], "lavfi.r128.LRA=2.5");
        assert_eq!(pts[1].rest[1], "lavfi.r128.true_peaks_ch1=0.5");
    }

    #[test]
    fn nan_is_normalised() {
        let out = parse("frame:0 pts:0 pts_time:0\nlavfi.r128.M=nan\nlavfi.r128.I=nan\n");
        assert_eq!(out.len(), 1);
        assert!(out[0].m == f64::NEG_INFINITY);
        assert!(out[0].i.is_nan());
    }

    #[test]
    fn handles_bare_noise_and_leading_garbage() {
        let out = parse("garbage line\n\nframe:0 pts:0 pts_time:0\nlavfi.r128.M=-10\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].m, -10.0);
    }
}
