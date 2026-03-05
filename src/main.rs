mod common;
mod config;
mod source;
mod target;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Parser)]
#[command(name = "nominatim-convert", about = "Convert geographic data to Nominatim NDJSON")]
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
    /// Input file
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
    /// Input CSV file
    #[arg(short = 'i')]
    input: PathBuf,
    /// Output file
    #[arg(short = 'o')]
    output: PathBuf,
    /// Stedsnavn GML file for county data
    #[arg(short = 'g')]
    stedsnavn_gml: Option<PathBuf>,
    /// Skip county population
    #[arg(long = "no-county", default_value_t = false)]
    no_county: bool,
    /// Configuration file
    #[arg(short = 'c')]
    config: Option<PathBuf>,
    /// Append to existing output file
    #[arg(short = 'a', default_value_t = false)]
    append: bool,
    /// Force overwrite if output file exists
    #[arg(short = 'f', default_value_t = false)]
    force: bool,
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.action {
        Action::Stopplace(args) => run_conversion("StopPlace", &args, |cfg, input, output, append| {
            source::stopplace::convert(cfg, input, output, append)
        }),
        Action::Matrikkel(args) => {
            if args.stedsnavn_gml.is_none() && !args.no_county {
                eprintln!("Error: matrikkel requires -g <stedsnavn.gml> for county data, or --no-county to skip it.");
                std::process::exit(1);
            }
            let convert_args = ConvertArgs {
                input: args.input,
                output: args.output,
                config: args.config,
                append: args.append,
                force: args.force,
            };
            let gml = args.stedsnavn_gml;
            run_conversion("Matrikkel", &convert_args, |cfg, input, output, append| {
                source::matrikkel::convert(cfg, input, output, append, gml.as_deref())
            })
        }
        Action::Osm(args) => run_conversion("OSM PBF", &args, |cfg, input, output, append| {
            source::osm::convert(cfg, input, output, append)
        }),
        Action::Stedsnavn(args) => run_conversion("Stedsnavn", &args, |cfg, input, output, append| {
            source::stedsnavn::convert(cfg, input, output, append)
        }),
        Action::Poi(args) => run_conversion("POI", &args, |cfg, input, output, append| {
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
    args: &ConvertArgs,
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
            println!("Overwriting existing file: {}", output.display());
            std::fs::remove_file(output)?;
        } else if args.append {
            println!("Appending to existing file: {}", output.display());
        }
    }

    println!("Starting {name} conversion...");
    let start = Instant::now();
    convert_fn(&cfg, &args.input, output, args.append)?;
    let duration = start.elapsed().as_secs_f64();
    let size_mb = std::fs::metadata(output).map(|m| m.len() as f64 / (1024.0 * 1024.0)).unwrap_or(0.0);
    let action = if args.append { "Appended to" } else { "Output written to" };
    println!("{name} conversion completed in {duration:.2} seconds. {action} {}, size: {size_mb:.2} MB.", output.display());
    Ok(())
}
