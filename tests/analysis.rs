//! End-to-end CLI tests. These require `ffmpeg` and `ffprobe` on `$PATH` and
//! pin the exact golden values to a given ffmpeg/ebur128 build, so they are
//! `#[ignore]`d by default. Run with:
//!
//! ```sh
//! cargo test -- --ignored --nocapture
//! ```

use std::process::Command;

fn ffmpeg_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn bin() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/target/debug/autocue-rs")
}

fn gen_sine_wav(path: &std::path::Path) {
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"])
        .arg("-i")
        .arg("sine=frequency=440:duration=5")
        .args(["-af", "volume=0.5", "-ac", "1"])
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success(), "ffmpeg failed to generate {path:?}");
}

fn run(args: &[&str]) -> (String, String) {
    let out = Command::new(bin()).args(args).output().unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
#[ignore]
fn golden_analysis_matches_expected_json() {
    if !ffmpeg_available() {
        eprintln!("ffmpeg not found; skipping");
        return;
    }
    let dir = std::env::temp_dir().join("autocue-rs-test");
    std::fs::create_dir_all(&dir).unwrap();
    let wav = dir.join("golden.wav");
    gen_sine_wav(&wav);

    let (stdout, stderr) = run(&[wav.to_str().unwrap()]);

    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let o = v.as_object().unwrap();

    let eq = |k: &str, want: f64| {
        assert_eq!(o[k].as_f64().unwrap(), want, "field {k}");
    };
    eq("duration", 5.0);
    eq("liq_cue_in", 0.0);
    eq("liq_cue_out", 4.9);
    eq("liq_cue_duration", 4.9);
    eq("liq_cross_start_next", 4.9);
    eq("liq_blankskip", 0.0);
    assert!(o["liq_sustained_ending"].as_bool().unwrap());
    assert!(!o["liq_longtail"].as_bool().unwrap());
    assert!(!o["liq_blank_skipped"].as_bool().unwrap());
    assert_eq!(o["liq_loudness"].as_str().unwrap(), "-27.78 LUFS");
    assert_eq!(o["liq_amplify"].as_str().unwrap(), "9.78 dB");
    assert_eq!(o["liq_true_peak_db"].as_str().unwrap(), "-24.01 dBFS");

    // Diagnostics on stderr.
    assert!(stderr.contains("Overlay: -35.78 LUFS"), "stderr: {stderr}");
    assert!(stderr.contains("Cue out time: 4.90 s"), "stderr: {stderr}");
}

#[test]
#[ignore]
fn write_tags_then_skip_analysis() {
    if !ffmpeg_available() {
        eprintln!("ffmpeg not found; skipping");
        return;
    }
    let dir = std::env::temp_dir().join("autocue-rs-test");
    std::fs::create_dir_all(&dir).unwrap();
    let wav = dir.join("write.wav");
    gen_sine_wav(&wav);

    // Write liq_* + replaygain tags, then re-run without -f: analysis is skipped
    // (tags are complete) so no diagnostics are emitted.
    let (stdout, _) = run(&["-w", "-r", wav.to_str().unwrap()]);
    assert!(serde_json::from_str::<serde_json::Value>(&stdout).is_ok());

    let (stdout, stderr) = run(&[wav.to_str().unwrap()]);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["liq_cue_out"].as_f64().unwrap(), 4.9);
    assert!(
        !stderr.contains("Overlay times"),
        "expected analysis skip, got diagnostics: {stderr}"
    );
}
