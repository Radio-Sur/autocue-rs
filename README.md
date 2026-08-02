# autocue-rs

[![crates.io](https://img.shields.io/crates/v/autocue-rs.svg)](https://crates.io/crates/autocue-rs)
[![docs.rs](https://img.shields.io/docsrs/autocue-rs)](https://docs.rs/autocue-rs)

On-the-fly audio cue-in, cue-out, overlay and EBU R128 loudness analysis for
AutoDJ software. A Rust port of
[Moonbase59/autocue](https://github.com/Moonbase59/autocue).

It measures loudness with ffmpeg's `ebur128` filter and derives:

- **cue-in** — where playback should start (skips leading silence)
- **cue-out** — where playback should end (skips trailing silence / hidden
  tracks)
- **cross-start-next** — where the next track should begin (overlay), including
  sustained-ending and long-tail detection
- **loudness / loudness range / true peak** — EBU R128 integrated loudness,
  LRA and true peak
- **amplify** — a ReplayGain-style gain to normalise to a target loudness

Results are returned as JSON and can optionally be
written back into the file's tags (`liq_*`, `replaygain_*`, and the classic
`R128_TRACK_GAIN` for Opus).

## Requirements

- Rust 1.75+
- `ffmpeg` (with the `ebur128` filter) on `$PATH`, plus `ffprobe` when reading
  existing tags

## Installation

### CLI

```
cargo install autocue-rs
```

This installs the `autocue-rs` binary into `~/.cargo/bin` (make sure that
directory is on your `$PATH`). Run `autocue-rs --help` to verify.

### Library

Add it as a dependency of your own crate:

```
cargo add autocue-rs
```

Or add it manually to `Cargo.toml`:

```toml
[dependencies]
autocue-rs = "0.1"
```

## Using the CLI

```
autocue-rs [OPTIONS] FILE
```

| Option | Description | Default |
| ------ | ----------- | ------- |
| `-t, --target <LUFS>` | Target loudness | `-18` |
| `-s, --silence <LU>` | Silence detection level below target | `-42` |
| `-o, --overlay <LU>` | Overlay detection level below target | `-8` |
| `-l, --longtail <SEC>` | Long-tail detection window | `15` |
| `-x, --extra <LU>` | Extra level for long-tail overlay | `-12` |
| `-d, --drop <PCT>` | Sustained-ending drop percentage | `40` |
| `-k, --noclip` | Prevent clipping when amplifying | off |
| `-b, --blankskip [SEC]` | Blank-skip (hidden-track) detection; bare `-b` = 5s | off |
| `-w, --write` | Write analysis results into the file tags | off |
| `-r, --replaygain` | Also write ReplayGain tags | off |
| `-f, --force` | Re-analyse even if valid tags exist | off |
| `-n, --nice` | Run ffmpeg with reduced priority (`nice -n 18`) | off |
| `-j, --json <FILE>` | Override results with values from a JSON file (`-` = stdin) | off |

Example:

```
$ autocue-rs track.flac
Overlay: -35.78 LUFS, Longtail: -47.78 LUFS, Measured end avg: -27.77 LUFS, Drop: 0.00%
Overlay times: 4.90/4.90/0.00 s (normal/sustained/longtail), using: 4.90 s.
Cue out time: 4.90 s
{"duration":5.0,"liq_cue_duration":4.9,"liq_cue_in":0.0,"liq_cue_out":4.9,"liq_cross_start_next":4.9,"liq_longtail":false,"liq_sustained_ending":true,"liq_loudness":"-27.78 LUFS","liq_loudness_range":"0.00 LU","liq_amplify":"9.78 dB","liq_amplify_adjustment":"0.00 dB","liq_reference_loudness":"-18.00 LUFS","liq_blankskip":0.0,"liq_blank_skipped":false,"liq_true_peak":0.063,"liq_true_peak_db":"-24.01 dBFS"}
```

## Using the library

The crate exposes the analysis as a reusable library. It is organised into a
few modules:

- `measure` — runs `ffmpeg`'s `ebur128` filter and returns per-100-ms
  loudness frames
- `cue_engine` — computes cue-in, cue-out, overlay and loudness values
- `analysis` — the high-level, tag-aware pipeline (skip re-analysis when
  valid tags exist, JSON overrides, optional tag writing)
- `model`, `options`, `tags` — shared types, settings and tag writing

### Low-level: measure then compute cues

```rust
use autocue_rs::{compute_cues, measure, CueOptions};

let opts = CueOptions::default();
let points = measure(std::path::Path::new("track.flac"), &opts)?;
let (cues, _diagnostics) = compute_cues(&points, &opts)?;

// Convert to the JSON-friendly output struct and serialize it.
let json = serde_json::to_string(&cues.output())?;
```

`CueOptions` mirrors the CLI flags (target, silence, overlay, longtail, extra,
drop, blankskip, noclip, ffmpeg/ffprobe paths). `cues.output()` produces the
`CueOutput` struct that serializes to the exact JSON the CLI prints.

### High-level: the full tag-aware pipeline

`analysis::run` mirrors what the CLI does: it reads existing tags, skips the
analysis when the file already has valid results, applies JSON overrides, and
optionally writes the tags back to the file.

```rust
use std::collections::HashMap;
use autocue_rs::{analysis, CueOptions};

let opts = CueOptions {
    target_lufs: -18.0,
    ..CueOptions::default()
};

let result = analysis::run(
    std::path::Path::new("track.flac"),
    &opts,
    &HashMap::new(),   // JSON overrides, e.g. from `-j`
    false,             // force re-analysis?
    false,             // write tags into the file?
    false,             // also write ReplayGain tags?
)?;

println!("{}", serde_json::to_string(&result.output)?);
```

`run` returns a `RunResult` with the `CueOutput`, optional diagnostics, and
whether tags were written.

## Tag support

Written via the corresponding native tag format (see `tags.rs`):

- **MP3 / WAV / AIFF / FLAC** — ID3v2 (`TXXX:liq_*`, `TXXX:replaygain_*`)
- **M4A / MP4** — iTunes freeform atoms (`----:com.apple.iTunes:liq_*`)
- **OGG / Opus** — Vorbis comments (`liq_*`, `REPLAYGAIN_TRACK_GAIN`)
- **WavPack** — APE tags (`liq_*`, `replaygain_*`)
- other formats — left untouched (like the original tool)

Existing tags are always preserved: ID3v2 and MP4 keep unknown tags through
lofty's companion tags, while Vorbis/APE comment blocks are rebuilt from a
ffprobe read with the new values overlaid.

## License

GPL-3.0 (the original `autocue`/`cue_file` is MIT, but this port is released
under GPL-3.0).
