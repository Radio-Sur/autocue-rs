use std::collections::HashMap;
use std::path::Path;

use lofty::config::WriteOptions;
use lofty::prelude::*;
use lofty::tag::TagExt;

use crate::error::{Error, Result};

/// File types tagged by ffmpeg (the original `safe_ext`), which ffmpeg does
/// not corrupt.
const FFMPEG_EXT: &[&str] = &["mp3", "flac", "spx", "opus", "oga", "ogv", "wma", "wmv", "asf"];
/// MP4-like file types tagged via lofty's iTunes-style ilst atoms (was mutagen).
const MP4_EXT: &[&str] = &["m4a", "m4b", "m4p", "m4v", "m4r", "mp4", "alac"];
/// File types tagged via ID3v2 (was mutagen).
const ID3_EXT: &[&str] = &["mp2", "m2a", "aiff", "aif", "aifc", "wav"];
/// File types tagged via APEv2 (was mutagen).
const APE_EXT: &[&str] = &["mpc", "mp+", "wv", "ofr", "ofs", "ape", "aac"];
/// Ogg Vorbis files tagged via Vorbis comments (was mutagen).
const OGG_EXT: &[&str] = &["ogg"];

/// Raw tags read from a file via ffprobe, as the original does. Keys keep
/// their original case; values are strings.
#[derive(Debug, Default, Clone)]
pub struct RawTags {
    /// Tags from stream #0.
    pub stream_tags: HashMap<String, String>,
    /// Tags from the container's format metadata.
    pub format_tags: HashMap<String, String>,
    /// Duration of stream #0, in seconds.
    pub duration: Option<f64>,
}

/// Reads stream #0 and format tags via ffprobe, exactly like the original
/// `read_tags()`. The two sets are returned separately; the caller decides
/// the merge order.
pub fn read_tags_ffprobe(path: &Path, ffprobe_path: &str) -> Result<RawTags> {
    let output = std::process::Command::new(ffprobe_path)
        .args([
            "-v",
            "quiet",
            "-show_entries",
            "stream=codec_name,duration:stream_tags:format_tags",
            "-of",
            "json=compact=1",
            path.to_string_lossy().as_ref(),
        ])
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::FfprobeNotFound(ffprobe_path.to_string())
            } else {
                Error::Io {
                    path: path.to_path_buf(),
                    source: e,
                }
            }
        })?;

    if !output.status.success() {
        return Err(Error::Analysis {
            path: path.to_path_buf(),
            message: "ffprobe failed to read file tags".to_string(),
        });
    }

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|e| Error::Analysis {
        path: path.to_path_buf(),
        message: format!("could not parse ffprobe JSON output: {e}"),
    })?;

    let mut raw = RawTags::default();

    if let Some(first_stream) = value["streams"].as_array().and_then(|streams| streams.first()) {
        if let Some(tags) = first_stream["tags"].as_object() {
            for (k, v) in tags {
                if let Some(s) = v.as_str() {
                    raw.stream_tags.insert(k.clone(), s.to_string());
                }
            }
        }
        let duration = first_stream["duration"]
            .as_str()
            .and_then(|d| d.parse::<f64>().ok())
            .or_else(|| first_stream["duration"].as_f64());
        raw.duration = duration;
    }

    if let Some(tags) = value["format"]["tags"].as_object() {
        for (k, v) in tags {
            if let Some(s) = v.as_str() {
                raw.format_tags.insert(k.clone(), s.to_string());
            }
        }
    }

    Ok(raw)
}

/// Writes the given tag map to the file. `tags` must already contain the final
/// formatted string values (units, etc.). Only known, writable file types are
/// touched; anything else is silently ignored, like the original.
pub fn write_tags(path: &Path, tags: &HashMap<String, String>, ffprobe_path: &str) -> Result<()> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();

    if !is_supported_ext(&ext) {
        return Ok(());
    }
    if !is_writable(path) {
        return Ok(());
    }

    if FFMPEG_EXT.contains(&ext.as_str()) {
        write_ffmpeg(path, tags)?;
    } else if MP4_EXT.contains(&ext.as_str()) {
        write_mp4(path, tags)?;
    } else if ID3_EXT.contains(&ext.as_str()) {
        write_id3(path, tags)?;
    } else if APE_EXT.contains(&ext.as_str()) {
        write_ape(path, tags, ffprobe_path)?;
    } else if OGG_EXT.contains(&ext.as_str()) {
        write_vorbis(path, tags, ffprobe_path)?;
    }

    Ok(())
}

fn is_supported_ext(ext: &str) -> bool {
    FFMPEG_EXT.contains(&ext) || MP4_EXT.contains(&ext) || ID3_EXT.contains(&ext) || APE_EXT.contains(&ext) || OGG_EXT.contains(&ext)
}

fn is_writable(path: &Path) -> bool {
    std::fs::OpenOptions::new().append(true).open(path).map(|_| true).unwrap_or(false)
}

fn write_ffmpeg(path: &Path, tags: &HashMap<String, String>) -> Result<()> {
    let temp = path.with_extension(format!("tmp.{}", path.extension().unwrap().to_string_lossy()));
    let mut cmd = std::process::Command::new("ffmpeg");
    cmd.arg("-v").arg("quiet").arg("-nostdin").arg("-y");
    cmd.arg("-i").arg(path.as_os_str());
    cmd.arg("-map_metadata").arg("0");
    for (k, v) in tags {
        cmd.arg("-metadata").arg(format!("{k}={v}"));
    }
    cmd.arg("-c").arg("copy").arg(&temp);

    let output = cmd.output().map_err(|e| Error::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    if !output.status.success() {
        return Err(Error::Analysis {
            path: path.to_path_buf(),
            message: format!(
                "ffmpeg tag writing failed: {}",
                String::from_utf8_lossy(&output.stderr)
                    .lines()
                    .rev()
                    .take(3)
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        });
    }

    std::fs::rename(&temp, path).map_err(|e| Error::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    Ok(())
}

/// Reads an existing tag of type `T` from the file, or creates a fresh one.
///
/// For `Id3v2Tag` and `Ilst` this recovers the full format-specific tag
/// (including format-specific items), so existing tags are preserved.
fn existing_tag<T>(path: &Path, tag_type: lofty::tag::TagType) -> T
where
    T: TagExt + Default + TryFrom<lofty::tag::Tag, Error = std::convert::Infallible>,
{
    lofty::read_from_path(path)
        .ok()
        .and_then(|tagged| {
            tagged
                .tags()
                .iter()
                .find(|t| t.tag_type() == tag_type)
                .cloned()
                .and_then(|t| T::try_from(t).ok())
        })
        .unwrap_or_default()
}

fn write_id3(path: &Path, tags: &HashMap<String, String>) -> Result<()> {
    let mut id3 = existing_tag::<lofty::id3::v2::Id3v2Tag>(path, lofty::tag::TagType::Id3v2);
    for (k, v) in tags {
        id3.remove_user_text(k);
        id3.insert_user_text(k.clone(), v.clone());
    }
    id3.save_to_path(path, WriteOptions::default()).map_err(|e| Error::WriteTags {
        path: path.to_path_buf(),
        source: e,
    })
}

fn write_mp4(path: &Path, tags: &HashMap<String, String>) -> Result<()> {
    use lofty::mp4::{Atom, AtomData, AtomIdent};

    let mut ilst = existing_tag::<lofty::mp4::Ilst>(path, lofty::tag::TagType::Mp4Ilst);
    for (k, v) in tags {
        let ident = AtomIdent::Freeform {
            mean: std::borrow::Cow::Borrowed("com.apple.iTunes"),
            name: std::borrow::Cow::Owned(k.clone()),
        };
        ilst.insert(Atom::new(ident, AtomData::UTF8(v.clone())));
    }
    ilst.save_to_path(path, WriteOptions::default()).map_err(|e| Error::WriteTags {
        path: path.to_path_buf(),
        source: e,
    })
}

/// Vorbis comments cannot be fully reconstructed from a lofty `Tag`, so we
/// rebuild the comment block from ffprobe's view of the existing tags and
/// overlay the new ones. This preserves all text tags.
fn write_vorbis(path: &Path, tags: &HashMap<String, String>, ffprobe_path: &str) -> Result<()> {
    let mut vc = lofty::ogg::VorbisComments::new();
    if let Ok(existing) = read_tags_ffprobe(path, ffprobe_path) {
        for (k, v) in existing.stream_tags.into_iter().chain(existing.format_tags) {
            vc.insert(k, v);
        }
    }
    for (k, v) in tags {
        vc.insert(k.clone(), v.clone());
    }
    vc.save_to_path(path, WriteOptions::default()).map_err(|e| Error::WriteTags {
        path: path.to_path_buf(),
        source: e,
    })
}

/// Same as [`write_vorbis`]: rebuild the APEv2 tag from ffprobe and overlay
/// the new keys.
fn write_ape(path: &Path, tags: &HashMap<String, String>, ffprobe_path: &str) -> Result<()> {
    let mut ape = lofty::ape::ApeTag::new();
    if let Ok(existing) = read_tags_ffprobe(path, ffprobe_path) {
        for (k, v) in existing.stream_tags.into_iter().chain(existing.format_tags) {
            if let Ok(item) = lofty::ape::ApeItem::new(k, lofty::tag::ItemValue::Text(v)) {
                ape.insert(item);
            }
        }
    }
    for (k, v) in tags {
        let item = lofty::ape::ApeItem::new(k.clone(), lofty::tag::ItemValue::Text(v.clone())).map_err(|_| Error::Analysis {
            path: path.to_path_buf(),
            message: format!("invalid APE tag key `{k}`"),
        })?;
        ape.insert(item);
    }
    ape.save_to_path(path, WriteOptions::default()).map_err(|e| Error::WriteTags {
        path: path.to_path_buf(),
        source: e,
    })
}
