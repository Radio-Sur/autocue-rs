use std::collections::HashMap;
use std::path::Path;

use crate::cue_engine::compute_cues;
use crate::error::Result;
use crate::measure;
use crate::model::{CueOutput, Diagnostics};
use crate::options::CueOptions;
use crate::tags;

/// A typed tag value, as produced by the tag conversion logic.
#[derive(Debug, Clone, PartialEq)]
pub enum TagValue {
    Num(f64),
    Bool(bool),
}

/// A map of lowercase tag keys to typed values.
pub type TagMap = HashMap<String, TagValue>;

#[derive(Copy, Clone, PartialEq)]
enum Conv {
    Float,
    FloatBlankskip,
    IsTrue,
    Int,
}

/// All tags known to the tool (`tags_to_check` in the original).
const TAGS_TO_CHECK: &[(&str, Conv)] = &[
    ("duration", Conv::Float),
    ("liq_amplify_adjustment", Conv::Float),
    ("liq_amplify", Conv::Float),
    ("liq_blankskip", Conv::FloatBlankskip),
    ("liq_blank_skipped", Conv::IsTrue),
    ("liq_cross_duration", Conv::Float),
    ("liq_cross_start_next", Conv::Float),
    ("liq_cue_duration", Conv::Float),
    ("liq_cue_file", Conv::IsTrue),
    ("liq_cue_in", Conv::Float),
    ("liq_cue_out", Conv::Float),
    ("liq_fade_in", Conv::Float),
    ("liq_fade_out", Conv::Float),
    ("liq_longtail", Conv::IsTrue),
    ("liq_loudness", Conv::Float),
    ("liq_loudness_range", Conv::Float),
    ("liq_reference_loudness", Conv::Float),
    ("liq_sustained_ending", Conv::IsTrue),
    ("liq_true_peak_db", Conv::Float),
    ("liq_true_peak", Conv::Float),
    ("r128_track_gain", Conv::Int),
    ("replaygain_reference_loudness", Conv::Float),
    ("replaygain_track_gain", Conv::Float),
    ("replaygain_track_peak", Conv::Float),
    ("replaygain_track_range", Conv::Float),
    ("liq_hook1_in", Conv::Float),
    ("liq_hook1_out", Conv::Float),
    ("liq_hook2_in", Conv::Float),
    ("liq_hook2_out", Conv::Float),
    ("liq_hook3_in", Conv::Float),
    ("liq_hook3_out", Conv::Float),
    ("liq_ramp1", Conv::Float),
    ("liq_ramp2", Conv::Float),
    ("liq_ramp3", Conv::Float),
];

/// The absolute minimum set of tags that must be present to (possibly) avoid
/// a re-analysis.
const TAGS_MANDATORY: &[&str] = &[
    "duration",
    "liq_cue_in",
    "liq_cue_out",
    "liq_cross_start_next",
    "replaygain_track_gain",
];

/// Tags that carry a unit suffix and need it stripped before conversion.
const SUFFIXED_TAGS: &[&str] = &[
    "liq_amplify",
    "liq_amplify_adjustment",
    "liq_loudness",
    "liq_loudness_range",
    "liq_reference_loudness",
    "replaygain_track_gain",
    "replaygain_track_range",
    "replaygain_reference_loudness",
    "liq_true_peak_db",
    "liq_true_peak",
];

fn tag_num(map: &TagMap, key: &str) -> f64 {
    match map.get(key) {
        Some(TagValue::Num(v)) => *v,
        _ => 0.0,
    }
}

fn tag_bool(map: &TagMap, key: &str) -> bool {
    match map.get(key) {
        Some(TagValue::Bool(v)) => *v,
        _ => false,
    }
}

fn strip_unit(value: &str) -> Option<f64> {
    let re = regex::Regex::new(r"([+-]?\d*\.?\d+)").unwrap();
    re.captures(value)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse::<f64>().ok())
}

fn float_blankskip(value: &str, blankskip: f64) -> Option<TagValue> {
    if let Ok(f) = value.trim().parse::<f64>() {
        Some(TagValue::Num(f))
    } else if value.trim().eq_ignore_ascii_case("true") {
        Some(TagValue::Num(blankskip))
    } else {
        Some(TagValue::Num(0.0))
    }
}

/// Converts a map of raw string tags into the typed form, keeping only known
/// tags, lowercasing keys and stripping unit suffixes.
fn convert_tags(raw: &HashMap<String, String>, blankskip: f64) -> TagMap {
    let mut out = TagMap::new();
    for (k, value) in raw {
        let lk = k.to_lowercase();
        let Some(&(_, conv)) = TAGS_TO_CHECK.iter().find(|(tk, _)| *tk == lk) else {
            continue;
        };
        let converted = match conv {
            Conv::Float => {
                if SUFFIXED_TAGS.contains(&lk.as_str()) {
                    strip_unit(value).map(TagValue::Num)
                } else {
                    value.trim().parse::<f64>().ok().map(TagValue::Num)
                }
            }
            Conv::Int => value.trim().parse::<i64>().ok().map(|v| TagValue::Num(v as f64)),
            Conv::IsTrue => Some(TagValue::Bool(value.trim().eq_ignore_ascii_case("true"))),
            Conv::FloatBlankskip => float_blankskip(value, blankskip),
        };
        if let Some(v) = converted {
            out.insert(lk, v);
        }
    }
    out
}

fn convert_json_tags(tags_json: &HashMap<String, serde_json::Value>, blankskip: f64) -> TagMap {
    let mut raw = HashMap::new();
    for (k, v) in tags_json {
        let s = match v {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Bool(b) => b.to_string(),
            _ => continue,
        };
        raw.insert(k.clone(), s);
    }
    convert_tags(&raw, blankskip)
}

/// Corrects the gain for true peaks, if clipping prevention is requested.
fn amplify_correct(target: f64, loudness: f64, true_peak_db: f64, noclip: bool) -> (f64, f64) {
    let mut correction = 0.0;
    let amplify = if noclip {
        let amp = target - loudness;
        let max_amp = -1.0 - true_peak_db;
        if amp > max_amp {
            correction = max_amp - amp;
            max_amp
        } else {
            amp
        }
    } else {
        target - loudness
    };
    (amplify, correction)
}

/// Context used by the tag reading/merging logic.
#[derive(Debug, Clone, Copy)]
pub struct ReadContext {
    pub target_lufs: f64,
    pub blankskip: f64,
    pub noclip: bool,
}

/// Result of reading existing tags and deciding whether a re-analysis is
/// needed.
pub struct ReadResult {
    pub skip_analysis: bool,
    pub tags: TagMap,
}

/// Reads existing tags from the file (and optional JSON overrides) and
/// determines whether a re-analysis can be avoided. Port of the original
/// `read_tags()`.
pub fn read_tags(path: &Path, tags_json: &HashMap<String, serde_json::Value>, ctx: &ReadContext, ffprobe_path: &str) -> Result<ReadResult> {
    let raw = tags::read_tags_ffprobe(path, ffprobe_path)?;

    let tags_in_stream = convert_tags(&raw.stream_tags, ctx.blankskip);
    let tags_in_format = convert_tags(&raw.format_tags, ctx.blankskip);
    let tags_in_json = convert_json_tags(tags_json, ctx.blankskip);

    // unify, format and JSON override stream tags in that order
    let mut tags_found = tags_in_stream;
    for (k, v) in tags_in_format {
        tags_found.insert(k, v);
    }
    for (k, v) in tags_in_json {
        tags_found.insert(k, v);
    }

    match raw.duration {
        Some(d) => {
            tags_found.insert("duration".to_string(), TagValue::Num(d));
        }
        None => {
            tags_found.remove("duration");
        }
    }

    // create replaygain_track_gain from Opus R128_TRACK_GAIN (ref: -23 LUFS)
    if tags_found.contains_key("r128_track_gain") {
        let rg = tag_num(&tags_found, "r128_track_gain") / 256.0 + (ctx.target_lufs - -23.0);
        tags_found.insert("replaygain_track_gain".to_string(), TagValue::Num(rg));
    }

    if !tags_found.contains_key("liq_amplify") && tags_found.contains_key("replaygain_track_gain") {
        tags_found.insert("liq_amplify".to_string(), tags_found.get("replaygain_track_gain").cloned().unwrap());
    }

    // Handle old RG1/mp3gain positive loudness reference ("89 dB" SPL = -14 LUFS).
    if let Some(TagValue::Num(ref_loud)) = tags_found.get("replaygain_reference_loudness") {
        if *ref_loud > 0.0 {
            tags_found.insert("replaygain_reference_loudness".to_string(), TagValue::Num(ref_loud - 107.0));
        }
    }

    if !tags_found.contains_key("liq_reference_loudness") && tags_found.contains_key("replaygain_reference_loudness") {
        tags_found.insert(
            "liq_reference_loudness".to_string(),
            tags_found.get("replaygain_reference_loudness").cloned().unwrap(),
        );
    }

    if tags_found.contains_key("liq_cue_in") && tags_found.contains_key("liq_cue_out") {
        let cue_dur = tag_num(&tags_found, "liq_cue_out") - tag_num(&tags_found, "liq_cue_in");
        tags_found.insert("liq_cue_duration".to_string(), TagValue::Num(cue_dur));
    }

    let mut skip_analysis = TAGS_MANDATORY.iter().all(|k| tags_found.contains_key(*k));

    // avoid re-analysis if we have enough data but a different loudness target
    if skip_analysis && tags_found.contains_key("liq_amplify") && tags_found.contains_key("liq_reference_loudness") {
        let adjust = ctx.target_lufs - tag_num(&tags_found, "liq_reference_loudness");
        tags_found.insert(
            "liq_amplify".to_string(),
            TagValue::Num(tag_num(&tags_found, "liq_amplify") + adjust),
        );
        tags_found.insert("liq_reference_loudness".to_string(), TagValue::Num(ctx.target_lufs));
    } else {
        skip_analysis = false;
    }

    // we need liq_true_peak_db, liq_true_peak and liq_loudness for RG writing
    // and clipping prevention
    if skip_analysis
        && tags_found.contains_key("liq_true_peak_db")
        && tags_found.contains_key("liq_true_peak")
        && tags_found.contains_key("liq_loudness")
    {
        let (amplify, adjustment) = amplify_correct(
            ctx.target_lufs,
            tag_num(&tags_found, "liq_loudness"),
            tag_num(&tags_found, "liq_true_peak_db"),
            ctx.noclip,
        );
        tags_found.insert("liq_amplify".to_string(), TagValue::Num(amplify));
        tags_found.insert("liq_amplify_adjustment".to_string(), TagValue::Num(adjustment));
    } else {
        skip_analysis = false;
    }

    // if liq_blankskip differs from the requested value, re-analyse
    if skip_analysis && tags_found.contains_key("liq_blankskip") && tag_num(&tags_found, "liq_blankskip") != ctx.blankskip {
        skip_analysis = false;
    }

    // liq_loudness_range is informational but we want correct values
    if skip_analysis && !tags_found.contains_key("liq_loudness_range") {
        skip_analysis = false;
    }

    Ok(ReadResult {
        skip_analysis,
        tags: tags_found,
    })
}

/// Fills in sensible defaults for tags that may be missing when a re-analysis
/// was avoided. Port of the original `add_missing()`.
fn add_missing(tags: &mut TagMap, target: f64, blankskip: f64) {
    if !tags.contains_key("liq_longtail") {
        tags.insert("liq_longtail".to_string(), TagValue::Bool(false));
    }
    if !tags.contains_key("liq_sustained_ending") {
        tags.insert("liq_sustained_ending".to_string(), TagValue::Bool(false));
    }
    if !tags.contains_key("liq_amplify") {
        tags.insert(
            "liq_amplify".to_string(),
            tags.get("replaygain_track_gain").cloned().unwrap_or(TagValue::Num(0.0)),
        );
    }
    if !tags.contains_key("liq_amplify_adjustment") {
        tags.insert("liq_amplify_adjustment".to_string(), TagValue::Num(0.0));
    }
    if !tags.contains_key("liq_loudness") {
        let loudness = target - tag_num(tags, "replaygain_track_gain");
        tags.insert("liq_loudness".to_string(), TagValue::Num(loudness));
    }
    if !tags.contains_key("liq_blankskip") {
        tags.insert("liq_blankskip".to_string(), TagValue::Num(blankskip));
    }
    if !tags.contains_key("liq_blank_skipped") {
        tags.insert("liq_blank_skipped".to_string(), TagValue::Bool(false));
    }
    if !tags.contains_key("liq_reference_loudness") {
        tags.insert("liq_reference_loudness".to_string(), TagValue::Num(target));
    }
    if !tags.contains_key("replaygain_track_gain") {
        tags.insert(
            "replaygain_track_gain".to_string(),
            tags.get("liq_amplify").cloned().unwrap_or(TagValue::Num(0.0)),
        );
    }
    if !tags.contains_key("replaygain_track_peak") {
        tags.insert(
            "replaygain_track_peak".to_string(),
            tags.get("liq_true_peak").cloned().unwrap_or(TagValue::Num(0.0)),
        );
    }
    if !tags.contains_key("replaygain_track_range") {
        tags.insert(
            "replaygain_track_range".to_string(),
            tags.get("liq_loudness_range").cloned().unwrap_or(TagValue::Num(0.0)),
        );
    }
    if !tags.contains_key("replaygain_reference_loudness") {
        tags.insert(
            "replaygain_reference_loudness".to_string(),
            tags.get("liq_reference_loudness").cloned().unwrap_or(TagValue::Num(target)),
        );
    }
}

/// Applies JSON overrides on top of an analysis result, unless
/// `liq_cue_file` is true.
fn override_from_json(result: &mut TagMap, tags_json: &TagMap) {
    if tag_bool(result, "liq_cue_file") {
        return;
    }
    for (k, v) in tags_json {
        result.insert(k.clone(), v.clone());
    }
}

/// Result of a full analysis run.
pub struct RunResult {
    /// Whether the result came from existing tags (no re-analysis).
    pub skip_analysis: bool,
    /// The stringified JSON output for the caller (stdout of the CLI).
    pub output: CueOutput,
    /// Diagnostics from the analysis, if one was performed.
    pub diagnostics: Option<Diagnostics>,
    /// Whether tags were written back to the file.
    pub wrote_tags: bool,
}

/// Runs the full pipeline: read existing tags, analyse (or reuse tags), apply
/// JSON overrides, optionally write tags. Port of the original `main()`.
#[allow(clippy::too_many_arguments)]
pub fn run(
    path: &Path,
    opts: &CueOptions,
    tags_json: &HashMap<String, serde_json::Value>,
    force: bool,
    write: bool,
    replaygain: bool,
) -> Result<RunResult> {
    let ctx = ReadContext {
        target_lufs: opts.target_lufs,
        blankskip: opts.blankskip_seconds,
        noclip: opts.noclip,
    };

    let read = read_tags(path, tags_json, &ctx, &opts.ffprobe_path)?;
    let mut result: TagMap;

    let diagnostics = if force || !read.skip_analysis {
        let points = measure::measure(path, opts)?;
        let (cues, diag) = compute_cues(&points, opts)?;

        result = TagMap::new();
        result.insert("duration".to_string(), TagValue::Num(cues.duration));
        result.insert("liq_cue_duration".to_string(), TagValue::Num(cues.cue_duration));
        result.insert("liq_cue_in".to_string(), TagValue::Num(cues.cue_in));
        result.insert("liq_cue_out".to_string(), TagValue::Num(cues.cue_out));
        result.insert("liq_cross_start_next".to_string(), TagValue::Num(cues.cross_start_next));
        result.insert("liq_longtail".to_string(), TagValue::Bool(cues.longtail));
        result.insert("liq_sustained_ending".to_string(), TagValue::Bool(cues.sustained_ending));
        result.insert("liq_loudness".to_string(), TagValue::Num(cues.loudness));
        result.insert("liq_loudness_range".to_string(), TagValue::Num(cues.loudness_range));
        result.insert("liq_amplify".to_string(), TagValue::Num(cues.amplify));
        result.insert("liq_amplify_adjustment".to_string(), TagValue::Num(cues.amplify_adjustment));
        result.insert("liq_reference_loudness".to_string(), TagValue::Num(cues.reference_loudness));
        result.insert("liq_blankskip".to_string(), TagValue::Num(cues.blankskip));
        result.insert("liq_blank_skipped".to_string(), TagValue::Bool(cues.blank_skipped));
        result.insert("liq_true_peak".to_string(), TagValue::Num(cues.true_peak));
        result.insert("liq_true_peak_db".to_string(), TagValue::Num(cues.true_peak_db));
        result.insert("replaygain_track_gain".to_string(), TagValue::Num(cues.amplify));
        result.insert("replaygain_track_peak".to_string(), TagValue::Num(cues.true_peak));
        result.insert("replaygain_track_range".to_string(), TagValue::Num(cues.loudness_range));
        result.insert("replaygain_reference_loudness".to_string(), TagValue::Num(cues.reference_loudness));

        // allow JSON to override even the analysis results (not if liq_cue_file=true)
        if !tags_json.is_empty() {
            let json_map = convert_json_tags(tags_json, ctx.blankskip);
            override_from_json(&mut result, &json_map);
        }

        // override duration, ffprobe can be more exact
        if let Some(TagValue::Num(d)) = read.tags.get("duration") {
            result.insert("duration".to_string(), TagValue::Num(*d));
        }

        Some(diag)
    } else {
        result = read.tags;
        add_missing(&mut result, ctx.target_lufs, ctx.blankskip);
        None
    };

    let mut wrote_tags = false;
    if write {
        let tags_new = build_tags_new(&result, ctx.target_lufs, replaygain, path);
        tags::write_tags(path, &tags_new, &opts.ffprobe_path)?;
        wrote_tags = true;
    }

    let output = to_output(&result);

    Ok(RunResult {
        skip_analysis: !force && read.skip_analysis,
        output,
        diagnostics,
        wrote_tags,
    })
}

/// Builds the final string tag map for writing, mirroring the original
/// `write_tags()` value preparation.
fn build_tags_new(result: &TagMap, target: f64, replaygain: bool, path: &Path) -> HashMap<String, String> {
    let rg_tags = [
        "replaygain_track_gain",
        "replaygain_track_peak",
        "replaygain_track_range",
        "replaygain_reference_loudness",
    ];

    let known: Vec<&str> = TAGS_TO_CHECK.iter().map(|(k, _)| *k).collect();
    let mut tags_new = HashMap::new();
    for (k, v) in result {
        if !known.contains(&k.as_str()) && !rg_tags.contains(&k.as_str()) {
            continue;
        }
        let s = match v {
            TagValue::Num(n) => format!("{n:.2}"),
            TagValue::Bool(b) => b.to_string().to_lowercase(),
        };
        tags_new.insert(k.clone(), s);
    }

    // liq_true_peak and replaygain_track_peak have 6 decimals
    for key in ["liq_true_peak", "replaygain_track_peak"] {
        if let Some(TagValue::Num(n)) = result.get(key) {
            tags_new.insert(key.to_string(), format!("{n:.6}"));
        }
    }

    // pre-calculate Opus R128_TRACK_GAIN (ref: -23 LUFS), just in case
    let target = if target == 0.0 {
        tag_num(result, "liq_reference_loudness")
    } else {
        target
    };
    let amp = tag_num(result, "liq_amplify");
    let og = ((amp - (target - -23.0)) * 256.0).round() as i64;
    tags_new.insert("R128_TRACK_GAIN".to_string(), og.to_string());

    for (key, unit) in [
        ("liq_amplify", " dB"),
        ("liq_amplify_adjustment", " dB"),
        ("liq_loudness", " LUFS"),
        ("liq_loudness_range", " LU"),
        ("liq_reference_loudness", " LUFS"),
        ("liq_true_peak_db", " dBFS"),
        ("replaygain_track_gain", " dB"),
        ("replaygain_track_range", " dB"),
        ("replaygain_reference_loudness", " LUFS"),
    ] {
        if let Some(v) = tags_new.get_mut(key) {
            v.push_str(unit);
        }
    }

    let is_opus = path
        .extension()
        .map(|e| e.to_string_lossy().eq_ignore_ascii_case("opus"))
        .unwrap_or(false);

    if replaygain {
        // for Opus, delete the replaygain_* tags; for all others, delete the
        // Opus track gain tag
        if is_opus {
            for k in rg_tags {
                tags_new.remove(k);
            }
        } else {
            tags_new.remove("R128_TRACK_GAIN");
        }
    } else {
        // no ReplayGain override, remove all "gain"-type tags
        for k in rg_tags {
            tags_new.remove(k);
        }
        tags_new.remove("R128_TRACK_GAIN");
    }

    // never write "duration"
    tags_new.remove("duration");

    tags_new
}

/// Builds the stringified JSON output form.
fn to_output(result: &TagMap) -> CueOutput {
    CueOutput {
        duration: tag_num(result, "duration"),
        liq_cue_duration: tag_num(result, "liq_cue_duration"),
        liq_cue_in: tag_num(result, "liq_cue_in"),
        liq_cue_out: tag_num(result, "liq_cue_out"),
        liq_cross_start_next: tag_num(result, "liq_cross_start_next"),
        liq_longtail: tag_bool(result, "liq_longtail"),
        liq_sustained_ending: tag_bool(result, "liq_sustained_ending"),
        liq_loudness: format!("{:.2} LUFS", tag_num(result, "liq_loudness")),
        liq_loudness_range: format!("{:.2} LU", tag_num(result, "liq_loudness_range")),
        liq_amplify: format!("{:.2} dB", tag_num(result, "liq_amplify")),
        liq_amplify_adjustment: format!("{:.2} dB", tag_num(result, "liq_amplify_adjustment")),
        liq_reference_loudness: format!("{:.2} LUFS", tag_num(result, "liq_reference_loudness")),
        liq_blankskip: tag_num(result, "liq_blankskip"),
        liq_blank_skipped: tag_bool(result, "liq_blank_skipped"),
        liq_true_peak: tag_num(result, "liq_true_peak"),
        liq_true_peak_db: format!("{:.2} dBFS", tag_num(result, "liq_true_peak_db")),
    }
}
