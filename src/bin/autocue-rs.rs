use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;

use autocue_rs::options::CueOptions;
use clap::Parser;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone)]
struct RangedF64Parser {
    min: f64,
    max: f64,
}

impl clap::builder::TypedValueParser for RangedF64Parser {
    type Value = f64;

    fn parse_ref(&self, _cmd: &clap::Command, _arg: Option<&clap::Arg>, value: &std::ffi::OsStr) -> Result<Self::Value, clap::Error> {
        let s = value
            .to_str()
            .ok_or_else(|| clap::Error::new(clap::error::ErrorKind::InvalidValue))?;
        let v: f64 = s.parse().map_err(|_| clap::Error::new(clap::error::ErrorKind::InvalidValue))?;
        if !(self.min..=self.max).contains(&v) {
            return Err(clap::Error::new(clap::error::ErrorKind::InvalidValue));
        }
        Ok(v)
    }
}

fn ranged(min: f64, max: f64) -> RangedF64Parser {
    RangedF64Parser { min, max }
}

#[derive(clap::Parser)]
#[command(
    name = "autocue-rs",
    version = VERSION,
    about = "Analyse audio file for cue-in, cue-out, overlay and EBU R128 loudness data, results as JSON. Optionally writes tags to original audio file, avoiding unnecessary re-analysis and getting results MUCH faster. This software is mainly intended for use with Liquidsoap \"autocue:\" protocol.",
    long_about = None,
    override_usage = "autocue-rs [OPTIONS] <FILE>",
    after_help = "Note autocue-rs will use the LARGER value from the sustained ending and longtail calculations to set the next track overlay point. This ensures special song endings are always kept intact in transitions.\n\nA full audio file analysis can take some time. autocue-rs tries to avoid a (re-)analysis if all required data can be read from existing tags in the file."
)]
struct Cli {
    /// File to be processed
    #[arg(value_name = "FILE")]
    file: PathBuf,

    /// LUFS reference target; -23.0 to 0.0
    #[arg(short, long, default_value_t = -18.0, value_parser = ranged(-23.0, 0.0))]
    target: f64,

    /// LU below integrated track loudness for cue-in & cue-out points (silence
    /// removal at beginning & end of a track)
    #[arg(short, long, default_value_t = -42.0, value_parser = ranged(-96.0, 0.0))]
    silence: f64,

    /// LU below integrated track loudness to trigger next track
    #[arg(short, long, default_value_t = -8.0, value_parser = ranged(-96.0, 0.0))]
    overlay: f64,

    /// More than so many seconds of calculated overlay duration are considered
    /// a long tail, and will force a recalculation using --extra, thus keeping
    /// long song endings intact
    #[arg(short, long, default_value_t = 15.0, value_parser = ranged(0.0, 60.0))]
    longtail: f64,

    /// Extra LU below overlay loudness to trigger next track for songs with
    /// long tail
    #[arg(short, long, default_value_t = -12.0, value_parser = ranged(-96.0, 0.0))]
    extra: f64,

    /// Max. percent loudness drop at the end to be still considered having a
    /// sustained ending. Such tracks will be recalculated using --extra,
    /// keeping the song ending intact. Zero (0.0) to switch off.
    #[arg(short, long, default_value_t = 40.0, value_parser = ranged(0.0, 100.0))]
    drop: f64,

    /// Clipping prevention: Lowers track gain if needed, to avoid peaks going
    /// above -1 dBFS. Uses true peak values of all audio channels.
    #[arg(short = 'k', long)]
    noclip: bool,

    /// Skip blank (silence) within track if longer than [BLANKSKIP] seconds
    /// (get rid of "hidden tracks"). Sets the cue-out point to where the
    /// silence begins. Don't use this with spoken or TTS-generated text, as it
    /// will often cut the message short. Zero (0.0) to switch off. Omitting
    /// [BLANKSKIP] defaults to 5 s.
    #[arg(short, long, num_args = 0..=1, default_missing_value = "5", default_value_t = 0.0, value_parser = ranged(0.0, 60.0))]
    blankskip: f64,

    /// Write Liquidsoap liq_* tags to file. Ensure you have enough free space
    /// to hold a copy of the original file.
    #[arg(short, long)]
    write: bool,

    /// Write ReplayGain tags to file (track only, no album). Useful if your
    /// files have no previous RG tags. Only valid if -w/--write is also
    /// specified.
    #[arg(short, long)]
    replaygain: bool,

    /// Force re-analysis, even if tags exist
    #[arg(short, long)]
    force: bool,

    /// Linux/MacOS only: Use nice? Will run analysis at nice level 18.
    #[arg(short, long)]
    nice: bool,

    /// Read/override tags from a JSON file. Use - to read from stdin. Intended
    /// for pre-processing software which can, for instance, fill in values
    /// from their database here.
    #[arg(short, long, value_name = "FILE")]
    json: Option<PathBuf>,
}

fn main() {
    let cli = Cli::parse();

    let mut tags_json = HashMap::new();
    if let Some(json_path) = &cli.json {
        let mut content = String::new();
        if json_path.as_os_str() == "-" {
            if let Err(e) = std::io::stdin().read_to_string(&mut content) {
                eprintln!("Error reading stdin: {e}");
                std::process::exit(1);
            }
        } else if let Err(e) = std::fs::File::open(json_path).and_then(|mut f| f.read_to_string(&mut content)) {
            eprintln!("Error reading JSON file {}: {e}", json_path.display());
            std::process::exit(1);
        }
        if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(&content) {
            for (k, val) in map {
                tags_json.insert(k, val);
            }
        }
    }

    let opts = CueOptions {
        target_lufs: cli.target,
        silence_lu: cli.silence,
        overlay_lu: cli.overlay,
        longtail_seconds: cli.longtail,
        longtail_extra_lu: cli.extra,
        sustained_drop_pct: cli.drop,
        blankskip_seconds: cli.blankskip,
        noclip: cli.noclip,
        nice: cli.nice,
        ..CueOptions::default()
    };

    match autocue_rs::analysis::run(&cli.file, &opts, &tags_json, cli.force, cli.write, cli.replaygain) {
        Ok(result) => {
            print_diagnostics(&result);
            let json = serde_json::to_string(&result.output).expect("JSON serialization");
            println!("{json}");
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

fn print_diagnostics(result: &autocue_rs::RunResult) {
    let Some(d) = &result.diagnostics else {
        return;
    };
    eprintln!(
        "Overlay: {:.2} LUFS, Longtail: {:.2} LUFS, Measured end avg: {} LUFS, Drop: {}%",
        d.overlay_level,
        d.longtail_level,
        fmt_inf(d.end_avg_lufs),
        fmt_inf(d.drop_pct)
    );
    eprintln!(
        "Overlay times: {:.2}/{:.2}/{:.2} s (normal/sustained/longtail), using: {:.2} s.",
        d.normal_start, d.sustained_start, d.longtail_start, d.used_start
    );
    eprintln!("Cue out time: {:.2} s", d.cue_out_time);
}

fn fmt_inf(v: f64) -> String {
    if v == f64::NEG_INFINITY {
        "-inf".to_string()
    } else {
        format!("{v:.2}")
    }
}
