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
use common::input::{CACHE_DIR_ENV, CacheOptions, ResolvedInput, is_cached, resolve_input, warn_if_stale};
use common::norwegian_counties;
use common::usage::UsageBoost;
use config::SourceInput;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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

    /// Warn (advisory) when a resolved source is older than HOURS: URL
    /// `Last-Modified`, or local file mtime. Bare flag = 24; `=N` overrides.
    #[arg(
        long,
        value_name = "HOURS",
        global = true,
        require_equals = true,
        num_args = 0..=1,
        default_missing_value = "24"
    )]
    warn_if_stale: Option<f64>,

    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Run a full config-driven import: convert every source whose `input` is set in
    /// the config file and append them into one NDJSON. This is the production entry point.
    Build(BuildArgs),
    /// Convert StopPlace NeTEx XML (local file)
    Stopplace(StopplaceArgs),
    /// Convert Matrikkel CSV data (Kartverket, local file)
    Matrikkel(MatrikkelArgs),
    /// Convert OSM PBF data (local file)
    Osm(ConvertArgs),
    /// Convert Stedsnavn GML data (Kartverket, local file)
    Stedsnavn(ConvertArgs),
    /// Convert POI NeTEx XML data (local file)
    Poi(ConvertArgs),
    /// Convert Swedish belägenhetsadresser (Lantmäteriet, local .gpkg file)
    Belagenhet(ConvertArgs),
    /// List Geonorge regions available for matrikkel/stedsnavn `region` inputs
    Regions,
    /// List Lantmäteriet municipalities available for belagenhet `municipality` inputs
    Municipalities,
}

fn geonorge_matrikkel_url(region: &str) -> String {
    format!("https://nedlasting.geonorge.no/geonorge/Basisdata/MatrikkelenAdresse/CSV/Basisdata_{region}_25833_MatrikkelenAdresse_CSV.zip")
}

fn geonorge_stedsnavn_url(region: &str) -> String {
    format!("https://nedlasting.geonorge.no/geonorge/Basisdata/Stedsnavn/GML/Basisdata_{region}_25833_Stedsnavn_GML.zip")
}

#[derive(Parser)]
struct ConvertArgs {
    /// Input local file (ZIP archives are extracted automatically). Use `build` to fetch
    /// from a URL, Geonorge region, or Lantmäteriet municipality.
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
struct BuildArgs {
    /// Configuration file describing the sources (their `input`) and scoring.
    #[arg(short, long)]
    config: PathBuf,
    /// Output file (combined NDJSON for every configured source)
    #[arg(short, long)]
    output: PathBuf,
    /// Append to existing output file instead of starting fresh
    #[arg(short, long, default_value_t = false)]
    append: bool,
    /// Force overwrite if output file exists
    #[arg(short, long, default_value_t = false)]
    force: bool,
}

/// The fare zone export is a second input to the StopPlace conversion: it is the only
/// source of fare zones, so without it stop places carry none.
#[derive(Parser)]
struct StopplaceArgs {
    #[command(flatten)]
    convert: ConvertArgs,
    /// Fare zone NeTEx export (from api.entur.io/distance/netex/fare-zones)
    #[arg(long = "fare-zones", value_name = "XML")]
    fare_zones: Option<PathBuf>,
}

#[derive(Parser)]
struct MatrikkelArgs {
    #[command(flatten)]
    convert: ConvertArgs,
    /// Stedsnavn GML file for county data
    #[arg(short = 'g', long = "gml", value_name = "GML")]
    stedsnavn_gml: Option<PathBuf>,
    /// Skip county population
    #[arg(short = 'n', long = "no-county", default_value_t = false)]
    no_county: bool,
}

/// The global CLI flags, bundled so they thread through as one `Copy` arg.
#[derive(Clone, Copy)]
struct RunOptions<'a> {
    cache: &'a CacheOptions,
    usage_csv: Option<&'a Path>,
    stale_after: Option<Duration>,
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

    let Cli { cache_dir, refresh_cache, usage_csv, warn_if_stale, action } = Cli::parse();
    let cache = CacheOptions::new(cache_dir.as_deref(), refresh_cache);
    let usage_csv = usage_csv.as_deref();
    // Bad values (negative, NaN, inf, overflow) disable the check rather than panic.
    let stale_after = warn_if_stale.and_then(|hours| Duration::try_from_secs_f64(hours * 3600.0).ok());
    let opts = RunOptions { cache: &cache, usage_csv, stale_after };

    let result = match action {
        Action::Build(args) => run_build(args, opts),
        Action::Stopplace(args) => run_stopplace(args, opts),
        Action::Matrikkel(args) => run_matrikkel(args, opts),
        Action::Osm(args) => run_conversion("OSM PBF", args, None, opts, |cfg| cfg.osm.as_ref().and_then(|s| s.min_lines), source::osm::convert),
        Action::Stedsnavn(args) => run_conversion("Stedsnavn", args, Some("*.gml"), opts, |cfg| cfg.stedsnavn.as_ref().and_then(|s| s.min_lines), source::stedsnavn::convert),
        Action::Poi(args) => run_conversion("POI", args, None, opts, |cfg| cfg.poi.as_ref().and_then(|s| s.min_lines), source::poi::convert),
        Action::Belagenhet(args) => run_conversion("Belägenhetsadress", args, Some("*.gpkg"), opts, |cfg| cfg.belagenhet.as_ref().and_then(|s| s.min_lines), source::belagenhet::convert),
        Action::Regions => {
            norwegian_counties::list_regions();
            return;
        }
        Action::Municipalities => {
            list_swedish_municipalities();
            return;
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
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

/// Validate and prepare the output path before writing: error if it exists without
/// `--force`/`--append`, remove it on `--force`, otherwise leave it for append. Shared by
/// `build` and the single-source subcommands so both behave identically.
fn prepare_output(output: &Path, force: bool, append: bool) -> Result<(), Box<dyn std::error::Error>> {
    if output.exists() {
        if !force && !append {
            return Err(format!(
                "Output file '{}' already exists. Use -f to overwrite or -a to append.",
                output.display()
            ).into());
        }
        if force {
            eprintln!("Overwriting existing file: {}", output.display());
            std::fs::remove_file(output)?;
        } else if append {
            eprintln!("Appending to existing file: {}", output.display());
        }
    }
    Ok(())
}

fn run_conversion<F>(
    name: &str,
    args: ConvertArgs,
    extract_glob: Option<&str>,
    opts: RunOptions,
    config_min: fn(&config::Config) -> Option<usize>,
    convert_fn: F,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnOnce(&config::Config, &Path, &Path, bool, &UsageBoost) -> Result<usize, Box<dyn std::error::Error>>,
{
    let cfg = config::Config::load(args.config.as_deref())?;
    let (alpha, floor) = usage_tuning(&cfg);
    let usage = UsageBoost::load(opts.usage_csv, alpha, floor)?;
    // CLI flag overrides the per-source config value; either may be absent (no check).
    let min_lines = args.min_lines.or_else(|| config_min(&cfg));

    let output = &args.output;
    prepare_output(output, args.force, args.append)?;

    let input = resolve_input(&args.input, extract_glob, opts.cache)?;
    warn_if_stale(name, &input, opts.stale_after);

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

/// StopPlace takes the fare zone export as a second input; resolve it here, then defer to
/// the shared `run_conversion` plumbing.
fn run_stopplace(
    args: StopplaceArgs,
    opts: RunOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let fare_zones = args.fare_zones.as_deref()
        .map(|p| resolve_input(p, None, opts.cache).map_err(|e| format!("resolving fare zone input: {e}")))
        .transpose()?;
    if let Some(fz) = fare_zones.as_ref() {
        warn_if_stale("Fare zones", fz, opts.stale_after);
    }
    let fare_zone_path = fare_zones.as_ref().map(ResolvedInput::path);
    run_conversion("StopPlace", args.convert, Some("*.xml"), opts, |cfg| cfg.stop_place.as_ref().and_then(|s| s.min_lines), |cfg, input, output, append, usage| {
        source::stopplace::convert(cfg, input, output, append, fare_zone_path, usage)
    })
    // `fare_zones` drops here; if temp, its file is cleaned up automatically.
}

/// Matrikkel needs an extra input next to the usual conversion args: the stedsnavn GML for
/// county data (or `--no-county` to skip it). Resolve that here, then defer to the shared
/// `run_conversion` plumbing.
fn run_matrikkel(
    args: MatrikkelArgs,
    opts: RunOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let gml_resolved = if args.no_county {
        None
    } else {
        match args.stedsnavn_gml.as_ref() {
            Some(gml) => Some(
                resolve_input(gml, Some("*.gml"), opts.cache)
                    .map_err(|e| format!("resolving GML input: {e}"))?,
            ),
            None => {
                return Err("matrikkel requires -g <stedsnavn.gml> for county data, or --no-county to skip it.".into());
            }
        }
    };
    if let Some(gml) = gml_resolved.as_ref() {
        warn_if_stale("Matrikkel county GML", gml, opts.stale_after);
    }
    let gml_path = gml_resolved.as_ref().map(ResolvedInput::path);
    run_conversion("Matrikkel", args.convert, Some("*.csv"), opts, |cfg| cfg.matrikkel.as_ref().and_then(|s| s.min_lines), |cfg, input, output, append, usage| {
        source::matrikkel::convert(cfg, input, output, append, gml_path, usage)
    })
    // gml_resolved drops here; if temp, its file is cleaned up automatically.
}

/// Download each municipality's belägenhetsadresser and append them into `output`.
/// Called only from `build`, which has already prepared the output file, so every
/// municipality appends (the first one creates the file via `JsonWriter`'s header logic).
fn run_belagenhet_download(
    cfg: &config::Config,
    output: &Path,
    municipalities: &[String],
    opts: RunOptions,
    usage: &UsageBoost,
    min_lines: Option<usize>,
) -> Result<(), Box<dyn std::error::Error>> {
    let start = Instant::now();
    let mut total_written = 0;

    for (i, kommun_id) in municipalities.iter().enumerate() {
        eprintln!("Processing municipality {kommun_id} ({}/{})...", i + 1, municipalities.len());

        let gpkg = source::belagenhet::download::download_municipality(kommun_id, opts.cache)?;
        warn_if_stale(&format!("Belägenhetsadress {kommun_id}"), &gpkg, opts.stale_after);
        total_written += source::belagenhet::convert(cfg, gpkg.path(), output, true, usage)?;
        // `gpkg` drops here; temp files cleaned up, cached files preserved.
    }

    // The minimum applies to the whole run's total, not per municipality, so legitimately
    // tiny municipalities don't trip it.
    check_min_lines("Belägenhetsadress", output, total_written, min_lines)?;

    let duration = start.elapsed().as_secs_f64();
    eprintln!(
        "Belägenhetsadress: {total_written} entries from {} municipalities in {duration:.2}s.",
        municipalities.len()
    );
    Ok(())
}

/// Config-driven import. Convert every source whose `input` is set, in a fixed order
/// (matrikkel, stedsnavn, poi, stopplace, osm, belagenhet), appending into one NDJSON.
fn run_build(args: BuildArgs, opts: RunOptions) -> Result<(), Box<dyn std::error::Error>> {
    let cfg = config::Config::load(Some(&args.config))?;
    let output = &args.output;

    let any_configured = cfg.matrikkel.is_some()
        || cfg.stedsnavn.is_some()
        || cfg.poi.is_some()
        || cfg.stop_place.is_some()
        || cfg.osm.is_some()
        || cfg.belagenhet.is_some();
    if !any_configured {
        return Err("No data sources configured. Add at least one source section (with an `input`) to the config.".into());
    }

    // Prepare the output once, up front: then every source appends, so the first writer
    // creates the file and the rest add to it (a single header).
    prepare_output(output, args.force, args.append)?;

    // Usage CSV: an explicit --usage path wins; otherwise resolve the config's usage.input.
    let usage_resolved = match (opts.usage_csv, cfg.usage.as_ref()) {
        (None, Some(u)) => Some(resolve_source(&u.input, "usage", None, opts)?),
        _ => None,
    };
    let usage_path = opts.usage_csv.or_else(|| usage_resolved.as_ref().map(ResolvedInput::path));
    let (alpha, floor) = usage_tuning(&cfg);
    let usage = UsageBoost::load(usage_path, alpha, floor)?;

    let start = Instant::now();

    // Stedsnavn is resolved once and reused both as matrikkel's county GML and as its
    // own import, so a region/URL is only downloaded a single time.
    let stedsnavn_resolved = match cfg.stedsnavn.as_ref() {
        Some(s) => Some(resolve_source(&s.input, "stedsnavn", Some("*.gml"), opts)?),
        None => None,
    };
    let stedsnavn_path = stedsnavn_resolved.as_ref().map(ResolvedInput::path);

    // Smallest and flakiest input, so resolve it before the multi-GB downloads. Its export
    // changes a few times a month, hence the longer staleness threshold.
    let fare_zones_resolved = match cfg.stop_place.as_ref() {
        Some(sp) => {
            let slow = RunOptions { stale_after: opts.stale_after.map(|d| d.saturating_mul(30)), ..opts };
            Some(resolve_source(&sp.fare_zones.input, "farezones", None, slow)?)
        }
        None => None,
    };

    if let Some(matrikkel) = cfg.matrikkel.as_ref() {
        let input = resolve_source(&matrikkel.input, "matrikkel", Some("*.csv"), opts)?;
        run_one_source("Matrikkel", input.path(), output, &cfg, matrikkel.min_lines, &usage, |c, i, o, a, u| {
            source::matrikkel::convert(c, i, o, a, stedsnavn_path, u)
        })?;
    }
    if let (Some(stedsnavn), Some(path)) = (cfg.stedsnavn.as_ref(), stedsnavn_path) {
        run_one_source("Stedsnavn", path, output, &cfg, stedsnavn.min_lines, &usage, source::stedsnavn::convert)?;
    }
    if let Some(poi) = cfg.poi.as_ref() {
        let input = resolve_source(&poi.input, "poi", None, opts)?;
        run_one_source("POI", input.path(), output, &cfg, poi.min_lines, &usage, source::poi::convert)?;
    }
    if let Some(stop_place) = cfg.stop_place.as_ref() {
        let input = resolve_source(&stop_place.input, "stopplace", Some("*.xml"), opts)?;
        let fare_zone_path = fare_zones_resolved.as_ref().map(ResolvedInput::path);
        run_one_source("StopPlace", input.path(), output, &cfg, stop_place.min_lines, &usage, |c, i, o, a, u| {
            source::stopplace::convert(c, i, o, a, fare_zone_path, u)
        })?;
    }
    if let Some(osm) = cfg.osm.as_ref() {
        let input = resolve_source(&osm.input, "osm", None, opts)?;
        run_one_source("OSM PBF", input.path(), output, &cfg, osm.min_lines, &usage, source::osm::convert)?;
    }
    if let Some(belagenhet) = cfg.belagenhet.as_ref() {
        match &belagenhet.input {
            SourceInput::Municipality(spec) => {
                let codes = resolve_municipality_codes(spec);
                if needs_belagenhet_credentials(opts.cache, &codes) {
                    preflight_check_credentials("LANTMATERIET_USER");
                    preflight_check_credentials("LANTMATERIET_PASS");
                }
                run_belagenhet_download(&cfg, output, &codes, opts, &usage, belagenhet.min_lines)?;
            }
            other => {
                let input = resolve_source(other, "belagenhet", Some("*.gpkg"), opts)?;
                run_one_source("Belägenhetsadress", input.path(), output, &cfg, belagenhet.min_lines, &usage, source::belagenhet::convert)?;
            }
        }
    }

    let duration = start.elapsed().as_secs_f64();
    let size_mb = std::fs::metadata(output).map(|m| m.len() as f64 / (1024.0 * 1024.0)).unwrap_or(0.0);
    eprintln!("Build completed in {duration:.2} seconds. Output: {}, size: {size_mb:.2} MB.", output.display());
    Ok(())
}

/// Run one source in a `build`: convert (always appending), then enforce its minLines.
fn run_one_source<F>(
    name: &str,
    input: &Path,
    output: &Path,
    cfg: &config::Config,
    min_lines: Option<usize>,
    usage: &UsageBoost,
    convert_fn: F,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnOnce(&config::Config, &Path, &Path, bool, &UsageBoost) -> Result<usize, Box<dyn std::error::Error>>,
{
    eprintln!("Starting {name} conversion...");
    let start = Instant::now();
    let written = convert_fn(cfg, input, output, true, usage)?;
    check_min_lines(name, output, written, min_lines)?;
    let duration = start.elapsed().as_secs_f64();
    eprintln!("{name}: {written} entries in {duration:.2}s.");
    Ok(())
}

/// Boost tuning (alpha, usage_floor) from the optional `usage` section, or the defaults when
/// it is absent. The values only matter when a usage CSV is actually loaded.
fn usage_tuning(cfg: &config::Config) -> (f64, u64) {
    cfg.usage.as_ref().map_or(
        (common::usage::DEFAULT_ALPHA, common::usage::DEFAULT_USAGE_FLOOR),
        |u| (u.alpha, u.usage_floor),
    )
}

/// Resolve a configured `SourceInput` to a local path (downloading/caching as needed).
/// `section` is used for Geonorge URL selection and for clear validation errors when a
/// `region`/`municipality` input lands on a section that does not support it.
fn resolve_source(
    src: &SourceInput,
    section: &str,
    extract_glob: Option<&str>,
    opts: RunOptions,
) -> Result<ResolvedInput, Box<dyn std::error::Error>> {
    let cache = opts.cache;
    let resolved = match src {
        SourceInput::Url(u) => resolve_input(Path::new(u), extract_glob, cache),
        SourceInput::File(p) => {
            // Fail fast with a section-tagged error rather than deep inside the converter.
            if !p.exists() {
                return Err(format!("{section}: input file not found: {}", p.display()).into());
            }
            resolve_input(p, extract_glob, cache)
        }
        SourceInput::Region(region) => {
            let slug = norwegian_counties::resolve_geonorge_region(region).map_err(|msg| format!("{section}: {msg}"))?;
            let url = match section {
                "matrikkel" => geonorge_matrikkel_url(&slug),
                "stedsnavn" => geonorge_stedsnavn_url(&slug),
                _ => return Err(format!("{section}: `region` input is only valid for matrikkel and stedsnavn").into()),
            };
            resolve_input(Path::new(&url), extract_glob, cache)
        }
        SourceInput::Municipality(_) => {
            return Err(format!("{section}: `municipality` input is only valid for belagenhet").into());
        }
    }
    .map_err(|e| format!("{}: {e}", source_label(section)))?;
    warn_if_stale(source_label(section), &resolved, opts.stale_after);
    Ok(resolved)
}

/// Display label for a config section, matching the single-source subcommand names.
fn source_label(section: &str) -> &str {
    match section {
        "matrikkel" => "Matrikkel",
        "stedsnavn" => "Stedsnavn",
        "poi" => "POI",
        "stopplace" => "StopPlace",
        "farezones" => "Fare zones",
        "osm" => "OSM PBF",
        "belagenhet" => "Belägenhetsadress",
        "usage" => "Usage",
        _ => section,
    }
}

fn list_swedish_municipalities() {
    use source::belagenhet::municipalities::MUNICIPALITIES;
    // A listing the user reads or pipes, so it goes to stdout.
    println!("Available municipalities for Lantmäteriet download:");
    println!("  all         All {} municipalities", MUNICIPALITIES.len());
    println!("  XX          All municipalities in county XX (2-digit county code)");
    println!();
    for (code, name) in MUNICIPALITIES {
        println!("  {code}  {name}");
    }
}

/// Expand a municipality spec: "all" becomes all 290 codes, a county prefix (2-digit)
/// expands to all municipalities in that county, otherwise the code is passed through as-is.
fn resolve_municipality_codes(spec: &str) -> Vec<String> {
    use source::belagenhet::municipalities::MUNICIPALITIES;

    let lower = spec.to_lowercase();
    if lower == "all" || lower == "00" {
        eprintln!("Expanding 'all' to all {} municipalities", MUNICIPALITIES.len());
        MUNICIPALITIES.iter().map(|(c, _)| c.to_string()).collect()
    } else if spec.len() == 2 && spec.chars().all(|c| c.is_ascii_digit()) {
        // County prefix: expand to all municipalities in that län
        let matching: Vec<String> = MUNICIPALITIES.iter()
            .filter(|(c, _)| c.starts_with(spec))
            .map(|(c, _)| c.to_string())
            .collect();
        if matching.is_empty() {
            eprintln!("Warning: no municipalities found for county code {spec}");
        } else {
            eprintln!("Expanding county {spec} to {} municipalities", matching.len());
        }
        matching
    } else {
        vec![spec.to_string()]
    }
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
