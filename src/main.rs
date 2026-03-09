mod common;
mod config;
mod source;
mod target;

use clap::{Parser, Subcommand};
use common::input::{cleanup_input, resolve_input};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Parser)]
#[command(name = "nominatim-converter", about = "Convert geographic data to Nominatim NDJSON")]
struct Cli {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Convert StopPlace NeTEx XML
    Stopplace(ConvertArgs),
    /// Convert Matrikkel CSV data
    Matrikkel(MatrikkelArgs),
    /// Convert OSM PBF data
    Osm(ConvertArgs),
    /// Convert Stedsnavn GML data
    Stedsnavn(ConvertArgs),
    /// Convert POI NeTEx XML data
    Poi(ConvertArgs),
}

#[derive(Parser)]
struct ConvertArgs {
    /// Input file or URL (http/https). ZIP archives are extracted automatically.
    #[arg(short = 'i')]
    input: PathBuf,
    /// Output file
    #[arg(short = 'o')]
    output: PathBuf,
    /// Configuration file (defaults to converter.json)
    #[arg(short = 'c')]
    config: Option<PathBuf>,
    /// Append to existing output file
    #[arg(short = 'a', default_value_t = false)]
    append: bool,
    /// Force overwrite if output file exists
    #[arg(short = 'f', default_value_t = false)]
    force: bool,
}

#[derive(Parser)]
struct MatrikkelArgs {
    #[command(flatten)]
    common: ConvertArgs,
    /// Stedsnavn GML file or URL for county data
    #[arg(short = 'g')]
    stedsnavn_gml: Option<PathBuf>,
    /// Skip county population
    #[arg(long = "no-county", default_value_t = false)]
    no_county: bool,
}

fn main() {
    // Suppress "Cannot find proj.db" warnings from bundled PROJ.
    // We use a pipeline string that doesn't need the database.
    if std::env::var_os("PROJ_DATA").is_none() {
        // SAFETY: called at the start of main before any other threads are spawned.
        unsafe { std::env::set_var("PROJ_DATA", "/dev/null") };
    }

    let cli = Cli::parse();

    let result = match cli.action {
        Action::Stopplace(args) => run_conversion("StopPlace", args, Some("*.xml"), |cfg, input, output, append| {
            source::stopplace::convert(cfg, input, output, append)
        }),
        Action::Matrikkel(args) => {
            if args.stedsnavn_gml.is_none() && !args.no_county {
                eprintln!("Error: matrikkel requires -g <stedsnavn.gml> for county data, or --no-county to skip it.");
                std::process::exit(1);
            }

            // Resolve the -g input separately
            let gml_resolved = args.stedsnavn_gml.as_ref().map(|g| resolve_input(g, Some("*.gml")));
            let gml_result = match gml_resolved {
                Some(Ok((path, is_temp))) => Some((path, is_temp)),
                Some(Err(e)) => {
                    eprintln!("Error resolving GML input: {e}");
                    std::process::exit(1);
                }
                None => None,
            };

            let gml_path = gml_result.as_ref().map(|(p, _)| p.as_path());
            let result = run_conversion("Matrikkel", args.common, Some("*.csv"), |cfg, input, output, append| {
                source::matrikkel::convert(cfg, input, output, append, gml_path)
            });

            if let Some((path, is_temp)) = gml_result {
                cleanup_input(&path, is_temp);
            }

            result
        }
        Action::Osm(args) => run_conversion("OSM PBF", args, None, |cfg, input, output, append| {
            source::osm::convert(cfg, input, output, append)
        }),
        Action::Stedsnavn(args) => run_conversion("Stedsnavn", args, Some("*.gml"), |cfg, input, output, append| {
            source::stedsnavn::convert(cfg, input, output, append)
        }),
        Action::Poi(args) => run_conversion("POI", args, None, |cfg, input, output, append| {
            source::poi::convert(cfg, input, output, append)
        }),
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

fn run_conversion<F>(
    name: &str,
    args: ConvertArgs,
    extract_glob: Option<&str>,
    convert_fn: F,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnOnce(&config::Config, &std::path::Path, &std::path::Path, bool) -> Result<(), Box<dyn std::error::Error>>,
{
    let cfg = config::Config::load(args.config.as_deref())?;

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

    let (input, is_temp) = resolve_input(&args.input, extract_glob)?;

    eprintln!("Starting {name} conversion...");
    let start = Instant::now();
    let result = convert_fn(&cfg, &input, output, args.append);

    cleanup_input(&input, is_temp);

    result?;

    let duration = start.elapsed().as_secs_f64();
    let size_mb = std::fs::metadata(output).map(|m| m.len() as f64 / (1024.0 * 1024.0)).unwrap_or(0.0);
    let action = if args.append { "Appended to" } else { "Output written to" };
    eprintln!("{name} conversion completed in {duration:.2} seconds. {action} {}, size: {size_mb:.2} MB.", output.display());
    Ok(())
}
