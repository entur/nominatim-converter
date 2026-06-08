// Data flow: CLI args → resolve input (URL/file/ZIP) → source converter → JsonWriter → NDJSON output
//
// Each subcommand maps to a converter in `source::*`. The `run_conversion` helper handles
// config loading, input resolution, timing, and output writing so individual converters
// only need to implement the transform step.

mod common;
mod config;
mod source;
mod target;

use clap::{Parser, Subcommand};
use common::input::{CACHE_DIR_ENV, CacheOptions, ResolvedInput, is_cached, resolve_input};
use common::norwegian_counties;
use common::usage::UsageBoost;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Parser)]
#[command(name = "nominatim-converter", about = "Convert geographic data to Nominatim NDJSON")]
struct Cli {
    /// Cache downloaded source files in DIR (reuses them on re-runs).
    #[arg(short = 'd', long, value_name = "DIR", global = true, env = CACHE_DIR_ENV)]
    cache_dir: Option<PathBuf>,

    /// Ignore cache entries and re-download (requires --cache-dir).
    #[arg(long, global = true, requires = "cache_dir")]
    refresh_cache: bool,

    /// Local `id;name;usage` CSV that boosts popular entities.
    #[arg(short = 'u', long = "usage", value_name = "FILE", global = true)]
    usage_csv: Option<PathBuf>,

    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Convert StopPlace NeTEx XML
    Stopplace(ConvertArgs),
    /// Convert Matrikkel CSV data (Kartverket)
    Matrikkel(MatrikkelArgs),
    /// Convert OSM PBF data
    Osm(ConvertArgs),
    /// Convert Stedsnavn GML data (Kartverket)
    Stedsnavn(StedsnavnArgs),
    /// Convert POI NeTEx XML data
    Poi(ConvertArgs),
    /// Convert Swedish belägenhetsadresser (Lantmäteriet)
    Belagenhet(BelagenhetArgs),
}

fn geonorge_matrikkel_url(region: &str) -> String {
    format!("https://nedlasting.geonorge.no/geonorge/Basisdata/MatrikkelenAdresse/CSV/Basisdata_{region}_25833_MatrikkelenAdresse_CSV.zip")
}

fn geonorge_stedsnavn_url(region: &str) -> String {
    format!("https://nedlasting.geonorge.no/geonorge/Basisdata/Stedsnavn/GML/Basisdata_{region}_25833_Stedsnavn_GML.zip")
}

#[derive(Parser)]
struct ConvertArgs {
    /// Input file or URL (http/https). ZIP archives are extracted automatically.
    #[arg(short, long)]
    input: PathBuf,
    /// Output file
    #[arg(short, long)]
    output: PathBuf,
    /// Configuration file (defaults to converter.json)
    #[arg(short, long)]
    config: Option<PathBuf>,
    /// Append to existing output file
    #[arg(short, long, default_value_t = false)]
    append: bool,
    /// Force overwrite if output file exists
    #[arg(short, long, default_value_t = false)]
    force: bool,
    /// Abort if fewer than N entries are written (overrides the per-source config minLines).
    #[arg(long = "min-lines", value_name = "N")]
    min_lines: Option<usize>,
}

#[derive(Parser)]
struct BelagenhetArgs {
    /// Input .gpkg file (required when -m is not used)
    #[arg(short, long)]
    input: Option<PathBuf>,
    /// Output file
    #[arg(short, long, required_unless_present = "list")]
    output: Option<PathBuf>,
    /// Download from Lantmäteriet by municipality code (e.g. 0180, 01, or "all")
    #[arg(short, long = "municipality", value_name = "CODE", num_args = 1.., conflicts_with = "input")]
    municipality: Option<Vec<String>>,
    /// List available municipalities for download
    #[arg(short, long, default_value_t = false)]
    list: bool,
    /// Configuration file (defaults to converter.json)
    #[arg(short, long)]
    config: Option<PathBuf>,
    /// Append to existing output file
    #[arg(short, long, default_value_t = false)]
    append: bool,
    /// Force overwrite if output file exists
    #[arg(short, long, default_value_t = false)]
    force: bool,
    /// Abort if fewer than N entries are written (overrides the per-source config minLines).
    #[arg(long = "min-lines", value_name = "N")]
    min_lines: Option<usize>,
}

#[derive(Parser)]
struct StedsnavnArgs {
    /// Input file or URL (use -r to download from Geonorge instead)
    #[arg(short, long, conflicts_with = "region")]
    input: Option<PathBuf>,
    /// Output file
    #[arg(short, long, required_unless_present = "list")]
    output: Option<PathBuf>,
    /// Download from Geonorge by county code (e.g. 03), name (e.g. Oslo), or "all"
    #[arg(short, long)]
    region: Option<String>,
    /// List available regions for download
    #[arg(short, long, default_value_t = false)]
    list: bool,
    /// Configuration file (defaults to converter.json)
    #[arg(short, long)]
    config: Option<PathBuf>,
    /// Append to existing output file
    #[arg(short, long, default_value_t = false)]
    append: bool,
    /// Force overwrite if output file exists
    #[arg(short, long, default_value_t = false)]
    force: bool,
    /// Abort if fewer than N entries are written (overrides the per-source config minLines).
    #[arg(long = "min-lines", value_name = "N")]
    min_lines: Option<usize>,
}

#[derive(Parser)]
struct MatrikkelArgs {
    /// Input CSV file or URL (use -r to download from Geonorge instead)
    #[arg(short, long, conflicts_with = "region")]
    input: Option<PathBuf>,
    /// Output file
    #[arg(short, long, required_unless_present = "list")]
    output: Option<PathBuf>,
    /// Download from Geonorge by county code (e.g. 03), name (e.g. Oslo), or "all"
    #[arg(short, long)]
    region: Option<String>,
    /// List available regions for download
    #[arg(short, long, default_value_t = false)]
    list: bool,
    /// Stedsnavn GML file or URL for county data (auto-downloaded when using -r)
    #[arg(short = 'g', long = "gml", value_name = "GML")]
    stedsnavn_gml: Option<PathBuf>,
    /// Skip county population
    #[arg(short = 'n', long = "no-county", default_value_t = false)]
    no_county: bool,
    /// Configuration file (defaults to converter.json)
    #[arg(short, long)]
    config: Option<PathBuf>,
    /// Append to existing output file
    #[arg(short, long, default_value_t = false)]
    append: bool,
    /// Force overwrite if output file exists
    #[arg(short, long, default_value_t = false)]
    force: bool,
    /// Abort if fewer than N entries are written (overrides the per-source config minLines).
    #[arg(long = "min-lines", value_name = "N")]
    min_lines: Option<usize>,
}

fn main() {
    // Suppress "Cannot find proj.db" warnings from bundled PROJ.
    // We use a pipeline string that doesn't need the database.
    if std::env::var_os("PROJ_DATA").is_none() {
        // SAFETY: called at the start of main before any other threads are spawned.
        // This is `unsafe` because modifying environment variables is not thread-safe
        // in general -- another thread could read the env concurrently. Here it's fine
        // because no threads exist yet.
        unsafe { std::env::set_var("PROJ_DATA", "/dev/null") };
    }

    // Load .env file for credentials (Lantmäteriet etc.) -- once, before any subcommand runs.
    dotenvy::dotenv().ok();

    let Cli { cache_dir, refresh_cache, usage_csv, action } = Cli::parse();
    let cache = CacheOptions::new(cache_dir.as_deref(), refresh_cache);
    let usage_csv = usage_csv.as_deref();

    let result = match action {
        Action::Stopplace(args) => run_conversion("StopPlace", args, Some("*.xml"), &cache, usage_csv, |cfg| cfg.stop_place.min_lines, |cfg, input, output, append, usage| {
            source::stopplace::convert(cfg, input, output, append, usage)
        }),
        Action::Matrikkel(args) => {
            if args.list {
                norwegian_counties::list_regions();
                return;
            }
            preflight_check(args.config.as_ref(), args.output.as_ref(), args.force, args.append);
            let input_path = match (&args.input, &args.region) {
                (Some(path), _) => path.clone(),
                (None, Some(region)) => {
                    let slug = resolve_geonorge_region(region);
                    PathBuf::from(geonorge_matrikkel_url(&slug))
                }
                (None, None) => {
                    eprintln!("Error: matrikkel requires -i <file> or -r <region> (e.g. -r 03 for Oslo, -r all for Norway)");
                    std::process::exit(1);
                }
            };

            // Auto-download stedsnavn GML for county data when using -m
            let gml_source = if args.no_county {
                None
            } else if let Some(gml) = args.stedsnavn_gml {
                Some(gml)
            } else if let Some(region) = &args.region {
                let slug = resolve_geonorge_region(region);
                Some(PathBuf::from(geonorge_stedsnavn_url(&slug)))
            } else {
                eprintln!("Error: matrikkel requires -g <stedsnavn.gml> for county data, or --no-county to skip it.");
                std::process::exit(1);
            };

            let gml_resolved = match gml_source.as_ref().map(|g| resolve_input(g, Some("*.gml"), &cache)) {
                Some(Ok(r)) => Some(r),
                Some(Err(e)) => {
                    eprintln!("Error resolving GML input: {e}");
                    std::process::exit(1);
                }
                None => None,
            };

            let convert_args = ConvertArgs {
                input: input_path,
                output: args.output.unwrap(),
                config: args.config,
                append: args.append,
                force: args.force,
                min_lines: args.min_lines,
            };

            let gml_path = gml_resolved.as_ref().map(ResolvedInput::path);
            run_conversion("Matrikkel", convert_args, Some("*.csv"), &cache, usage_csv, |cfg| cfg.matrikkel.min_lines, |cfg, input, output, append, usage| {
                source::matrikkel::convert(cfg, input, output, append, gml_path, usage)
            })
            // gml_resolved drops here; if temp, its file is cleaned up automatically.
        }
        Action::Osm(args) => run_conversion("OSM PBF", args, None, &cache, usage_csv, |cfg| cfg.osm.min_lines, |cfg, input, output, append, usage| {
            source::osm::convert(cfg, input, output, append, usage)
        }),
        Action::Stedsnavn(args) => {
            if args.list {
                norwegian_counties::list_regions();
                return;
            }
            preflight_check(args.config.as_ref(), args.output.as_ref(), args.force, args.append);
            let input_path = match (&args.input, &args.region) {
                (Some(path), _) => path.clone(),
                (None, Some(region)) => {
                    let slug = resolve_geonorge_region(region);
                    PathBuf::from(geonorge_stedsnavn_url(&slug))
                }
                (None, None) => {
                    eprintln!("Error: stedsnavn requires -i <file> or -r <region> (e.g. -r 03 for Oslo, -r all for Norway)");
                    std::process::exit(1);
                }
            };
            let convert_args = ConvertArgs {
                input: input_path,
                output: args.output.unwrap(),
                config: args.config,
                append: args.append,
                force: args.force,
                min_lines: args.min_lines,
            };
            run_conversion("Stedsnavn", convert_args, Some("*.gml"), &cache, usage_csv, |cfg| cfg.stedsnavn.min_lines, |cfg, input, output, append, usage| {
                source::stedsnavn::convert(cfg, input, output, append, usage)
            })
        }
        Action::Poi(args) => run_conversion("POI", args, None, &cache, usage_csv, |cfg| cfg.poi.min_lines, |cfg, input, output, append, usage| {
            source::poi::convert(cfg, input, output, append, usage)
        }),
        Action::Belagenhet(args) => {
            if args.list {
                list_swedish_municipalities();
                return;
            }
            preflight_check(args.config.as_ref(), args.output.as_ref(), args.force, args.append);
            if let Some(ref municipalities) = args.municipality {
                let codes = resolve_municipality_codes(municipalities);
                // Credentials are only needed when any municipality will hit
                // the network: no cache dir, or --refresh-cache, or at least
                // one municipality isn't cached yet. Checking ahead of time
                // means users with a fully warm cache don't need creds.
                if needs_belagenhet_credentials(&cache, &codes) {
                    preflight_check_credentials("LANTMATERIET_USER");
                    preflight_check_credentials("LANTMATERIET_PASS");
                }
                run_belagenhet_download(&args, &codes, &cache, usage_csv)
            } else {
                let input = args.input.as_ref().unwrap_or_else(|| {
                    eprintln!("Error: belagenhet requires either -i <file.gpkg> or -m <municipality_code>");
                    std::process::exit(1);
                });
                let convert_args = ConvertArgs {
                    input: input.clone(),
                    output: args.output.unwrap(),
                    config: args.config,
                    append: args.append,
                    force: args.force,
                    min_lines: args.min_lines,
                };
                run_conversion("Belägenhetsadress", convert_args, Some("*.gpkg"), &cache, usage_csv, |cfg| cfg.belagenhet.min_lines, |cfg, input, output, append, usage| {
                    source::belagenhet::convert(cfg, input, output, append, usage)
                })
            }
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

/// Validate config, output file, and credentials before starting any downloads.
fn preflight_check(config: Option<&PathBuf>, output: Option<&PathBuf>, force: bool, append: bool) {
    let config_path = config.map(|p| p.as_path()).unwrap_or_else(|| std::path::Path::new("converter.json"));
    if !config_path.exists() {
        eprintln!("Error: Cannot read config file '{}': No such file or directory", config_path.display());
        std::process::exit(1);
    }
    if let Some(output) = output
        && output.exists() && !force && !append
    {
        eprintln!("Error: Output file '{}' already exists. Use -f to overwrite or -a to append.", output.display());
        std::process::exit(1);
    }
    if let Some(output) = output
        && append && output.exists()
    {
        let size = std::fs::metadata(output).map(|m| m.len()).unwrap_or(0);
        if size > 0 {
            let mut buf = [0u8; 32];
            let n = std::fs::File::open(output)
                .and_then(|mut f| std::io::Read::read(&mut f, &mut buf))
                .unwrap_or(0);
            let header = std::str::from_utf8(&buf[..n]).unwrap_or("");
            if !header.contains("NominatimDumpFile") {
                eprintln!("Error: Output file '{}' does not appear to be a Nominatim NDJSON file. Refusing to append.", output.display());
                std::process::exit(1);
            }
        }
    }
}

fn preflight_check_credentials(env_var: &str) {
    if std::env::var(env_var).is_err() {
        eprintln!("Error: {env_var} environment variable not set. Set it directly or in a .env file.");
        std::process::exit(1);
    }
}

/// Decide whether any Lantmäteriet download will actually hit the network.
/// False only when every requested municipality already has a cache entry
/// and we aren't asked to refresh -- i.e., a fully warm cache run that
/// needs no auth.
fn needs_belagenhet_credentials(cache: &CacheOptions, codes: &[String]) -> bool {
    if cache.dir().is_none() || cache.is_refresh() {
        return true;
    }
    codes.iter().any(|code| {
        let url = source::belagenhet::download::municipality_url(code);
        !is_cached(&url, cache)
    })
}

/// Enforce the optional per-source minimum entry count. A conversion that writes fewer
/// entries than the threshold is treated as a failure (e.g. an empty or truncated upstream
/// download) so the import job aborts loudly instead of shipping a degraded index. On
/// success with a threshold set, log a confirmation so operators can see the guard ran
/// (a mistyped config key silently disables it). `None` means no check.
fn check_min_lines(name: &str, output: &Path, written: usize, min_lines: Option<usize>) -> Result<(), Box<dyn std::error::Error>> {
    match min_lines {
        Some(min) if written < min => Err(format!(
            "{name}: only {written} entries written, below the minimum of {min}. \
             Output at {} may be incomplete -- do not import it.",
            output.display()
        ).into()),
        Some(min) => {
            eprintln!("{name}: min-lines check passed ({written} >= {min}).");
            Ok(())
        }
        None => Ok(()),
    }
}

fn run_conversion<F>(
    name: &str,
    args: ConvertArgs,
    extract_glob: Option<&str>,
    cache: &CacheOptions,
    usage_csv: Option<&Path>,
    config_min: fn(&config::Config) -> Option<usize>,
    convert_fn: F,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnOnce(&config::Config, &Path, &Path, bool, &UsageBoost) -> Result<usize, Box<dyn std::error::Error>>,
{
    let cfg = config::Config::load(args.config.as_deref())?;
    let usage = UsageBoost::load(usage_csv, &cfg.usage)?;
    // CLI flag overrides the per-source config value; either may be absent (no check).
    let min_lines = args.min_lines.or_else(|| config_min(&cfg));

    let output = &args.output;
    if output.exists() {
        if !args.force && !args.append {
            return Err(format!(
                "Output file '{}' already exists. Use -f to overwrite or -a to append.",
                output.display()
            ).into());
        }
        if args.force {
            eprintln!("Overwriting existing file: {}", output.display());
            std::fs::remove_file(output)?;
        } else if args.append {
            eprintln!("Appending to existing file: {}", output.display());
        }
    }

    let input = resolve_input(&args.input, extract_glob, cache)?;

    eprintln!("Starting {name} conversion...");
    let start = Instant::now();
    let written = convert_fn(&cfg, input.path(), output, args.append, &usage)?;
    // `input` drops here; temp files are removed automatically.

    check_min_lines(name, output, written, min_lines)?;

    let duration = start.elapsed().as_secs_f64();
    let size_mb = std::fs::metadata(output).map(|m| m.len() as f64 / (1024.0 * 1024.0)).unwrap_or(0.0);
    let action = if args.append { "Appended to" } else { "Output written to" };
    eprintln!("{name} conversion completed in {duration:.2} seconds. {action} {}, size: {size_mb:.2} MB.", output.display());
    Ok(())
}

fn run_belagenhet_download(
    args: &BelagenhetArgs,
    municipalities: &[String],
    cache: &CacheOptions,
    usage_csv: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    let cfg = config::Config::load(args.config.as_deref())?;
    let usage = UsageBoost::load(usage_csv, &cfg.usage)?;
    let output = args.output.as_ref().expect("output required for download");

    if output.exists() {
        if !args.force && !args.append {
            return Err(format!(
                "Output file '{}' already exists. Use -f to overwrite or -a to append.",
                output.display()
            ).into());
        }
        if args.force {
            eprintln!("Overwriting existing file: {}", output.display());
            std::fs::remove_file(output)?;
        }
    }

    let start = Instant::now();
    let mut is_first = !args.append;
    let mut total_written = 0;

    for (i, kommun_id) in municipalities.iter().enumerate() {
        eprintln!("Processing municipality {kommun_id} ({}/{})...", i + 1, municipalities.len());

        let gpkg = source::belagenhet::download::download_municipality(kommun_id, cache)?;
        let appending = !is_first;
        total_written += source::belagenhet::convert(&cfg, gpkg.path(), output, appending, &usage)?;
        // `gpkg` drops here; temp files cleaned up, cached files preserved.
        is_first = false;
    }

    // The minimum applies to the whole run's total, not per municipality, so legitimately
    // tiny municipalities don't trip it. CLI flag overrides the per-source config value.
    let min_lines = args.min_lines.or(cfg.belagenhet.min_lines);
    check_min_lines("Belägenhetsadress", output, total_written, min_lines)?;

    let duration = start.elapsed().as_secs_f64();
    let size_mb = std::fs::metadata(output).map(|m| m.len() as f64 / (1024.0 * 1024.0)).unwrap_or(0.0);
    eprintln!(
        "Belägenhetsadress conversion completed in {duration:.2} seconds. {} municipalities processed. Output: {}, size: {size_mb:.2} MB.",
        municipalities.len(),
        output.display()
    );
    Ok(())
}

fn resolve_geonorge_region(region: &str) -> String {
    match norwegian_counties::resolve_geonorge_region(region) {
        Ok(slug) => slug,
        Err(msg) => {
            eprintln!("Error: {msg}");
            norwegian_counties::list_regions();
            std::process::exit(1);
        }
    }
}

fn list_swedish_municipalities() {
    use source::belagenhet::municipalities::MUNICIPALITIES;
    eprintln!("Available municipalities for Lantmäteriet download:");
    eprintln!("  all         All {} municipalities", MUNICIPALITIES.len());
    eprintln!("  XX          All municipalities in county XX (2-digit county code)");
    eprintln!();
    for (code, name) in MUNICIPALITIES {
        eprintln!("  {code}  {name}");
    }
}

/// Expand municipality arguments: "all" becomes all 290 codes, county prefixes (2-digit)
/// expand to all municipalities in that county, otherwise codes are passed through as-is.
fn resolve_municipality_codes(args: &[String]) -> Vec<String> {
    use source::belagenhet::municipalities::MUNICIPALITIES;

    let mut codes = Vec::new();
    for arg in args {
        let lower = arg.to_lowercase();
        if lower == "all" || lower == "00" {
            eprintln!("Expanding 'all' to all {} municipalities", MUNICIPALITIES.len());
            return MUNICIPALITIES.iter().map(|(c, _)| c.to_string()).collect();
        } else if arg.len() == 2 && arg.chars().all(|c| c.is_ascii_digit()) {
            // County prefix: expand to all municipalities in that län
            let matching: Vec<String> = MUNICIPALITIES.iter()
                .filter(|(c, _)| c.starts_with(arg.as_str()))
                .map(|(c, _)| c.to_string())
                .collect();
            if matching.is_empty() {
                eprintln!("Warning: no municipalities found for county code {arg}");
            } else {
                eprintln!("Expanding county {arg} to {} municipalities", matching.len());
                codes.extend(matching);
            }
        } else {
            codes.push(arg.clone());
        }
    }
    codes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_lines_unset_always_passes() {
        let out = Path::new("out.ndjson");
        assert!(check_min_lines("X", out, 0, None).is_ok());
    }

    #[test]
    fn min_lines_boundary() {
        let out = Path::new("out.ndjson");
        // Exactly meeting the threshold passes; one short fails. Guards against `<` vs `<=`.
        assert!(check_min_lines("X", out, 10, Some(10)).is_ok());
        assert!(check_min_lines("X", out, 9, Some(10)).is_err());
        assert!(check_min_lines("X", out, 100, Some(10)).is_ok());
    }
}
