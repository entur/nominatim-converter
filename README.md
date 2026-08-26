# nominatim-converter

A Rust CLI tool that converts geographic data sources into Nominatim-compatible NDJSON.

## Data sources

| Source | Input format | Description |
|--------|-------------|-------------|
| **stopplace** | NeTEx XML | NSR/SAM stop places and groups of stop places (plus a second NeTEx input for fare zones) |
| **matrikkel** | CSV + GML | Kartverket address registry (vegadresser) with street aggregation |
| **stedsnavn** | GML | Kartverket place names (SSR) |
| **poi** | NeTEx XML | Points of interest from NeTEx |
| **osm** | PBF | OpenStreetMap entities (nodes, ways, relations) |
| **belagenhet** | GeoPackage | Lantmäteriet belägenhetsadresser (Swedish cadastral addresses) |

## Building

Requires Rust 2024 edition (1.85+). PROJ is statically linked via `bundled_proj`.

```bash
cargo build --release
```

The binary is at `target/release/nominatim-converter`.

## Usage

Everything is driven by a `converter.json` configuration file (see
[`converter.example.json`](converter.example.json) for the schema - it lists
every source section, including `belagenhet`, for completeness; a real config
contains only the sources it imports).

### `build` (the normal path)

`build` runs a full import: it converts every source whose section sets an
`input` and appends them all into one NDJSON, in a fixed order (matrikkel,
stedsnavn, poi, stopplace, osm, belagenhet). Sources, their locations, and the
scoring all live in the config, so this is a single command:

```bash
nominatim-converter build -c converter.json -o output.ndjson -f
```

Each source's `input` says where its data comes from:

```json
"matrikkel":  { "input": { "region": "all" }, ... },                 // Geonorge download
"stopPlace":  { "input": { "url": "https://.../Current_latest.zip" }, ... },
"osm":        { "input": { "file": "norway-latest.osm.pbf" }, ... },  // local file
"belagenhet": { "input": { "municipality": "all" }, ... }            // Lantmäteriet download
```

`region` (matrikkel/stedsnavn, Geonorge) and `municipality` (belagenhet,
Lantmäteriet) trigger downloads; `url` and `file` are generic. **A section is
present only when you want to import that source, and a present section must have
an `input`** (a section without one is a hard error - you declared a source but
not where its data comes from). To skip a source, omit its section entirely. The
usage CSV goes in a `usage` section with its own `input` (plus optional `alpha`/
`usageFloor` tuning) - present it to enable the boost, omit it to skip. `build` resolves the
stedsnavn source once and reuses it as matrikkel's county GML, so it is fetched
only once.

`build` with a `municipality` input requires Lantmäteriet Geotorget credentials
via environment variables or a `.env` file:

```bash
export LANTMATERIET_USER=your_username
export LANTMATERIET_PASS=your_password
```

### Fare zones

Stop place fare zones come from their own NeTEx export, not from the stop place file:

```json
"stopPlace": {
  "input":     { "url": "https://.../Current_latest.zip" },
  "fareZones": { "input": { "url": "https://api.entur.io/distance/netex/fare-zones" } },
  ...
}
```

`fareZones` is optional. Leave it out and the converter falls back to the `:FareZone:` refs NSR
mirrors into each stop, with authorities from the same input's `<FareFrame>`; on a full run that
reproduces the export's zone IDs and authorities exactly. It is still a mirror of a source due
to disappear from the NSR export, so set `fareZones` for anything long-lived - but an ad-hoc run
or a country without fare zones no longer needs it. The two never mix: with `fareZones` set, the
`<FareFrame>` is not read. The `stopplace` subcommand takes the same file via `--fare-zones`.
See AGENTS.md for how stop membership is derived.

### Single-source subcommands (local files)

The per-source subcommands convert one **local** file - handy for debugging or
ad-hoc runs. They do not download; use `build` with an `input` for that.

```bash
nominatim-converter stopplace  -i stop_places.xml -o out.ndjson -c converter.json --fare-zones fare-zones.xml
nominatim-converter matrikkel  -i adresse.csv -o out.ndjson -c converter.json -g stedsnavn.gml
nominatim-converter matrikkel  -i adresse.csv -o out.ndjson -c converter.json --no-county
nominatim-converter stedsnavn  -i stedsnavn.gml -o out.ndjson -c converter.json
nominatim-converter poi        -i poi.xml -o out.ndjson -c converter.json
nominatim-converter osm        -i planet.osm.pbf -o out.ndjson -c converter.json
nominatim-converter belagenhet -i belagenhetsadresser_kn0180.gpkg -o out.ndjson -c converter.json
```

`regions` and `municipalities` list the codes accepted by `region`/
`municipality` inputs.

### Common flags

| Flag | Description |
|------|-------------|
| `-i` | Input local file (single-source subcommands; required) |
| `-o` | Output file (required) |
| `-c` | Config file (defaults to `converter.json` in CWD; required for `build`) |
| `-f` | Force overwrite existing output |
| `-a` | Append to existing output |
| `-d` | Cache directory for downloads (see below); also via `NOMINATIM_CACHE_DIR` |
| `--refresh-cache` | Ignore cache hits and re-download |
| `-u` | Local `id;name;usage` CSV that boosts popular entities (see below) |
| `--min-lines <N>` | Abort if fewer than N entries are written (single-source subcommands; sanity check; see below) |
| `--warn-if-stale[=HOURS]` | Warn when a resolved source is older than HOURS (bare = 24; advisory, see below) |

### Minimum line count (`--min-lines`)

A sanity check against silently broken imports (an empty download, a changed
upstream format). If a conversion writes fewer than the threshold, it exits
non-zero instead of shipping a degraded index. The check counts the entries
*this run* emits (not the file total), so it works correctly in `-a` append
mode too.

Set a threshold per source by adding a `minLines` key to that source's config
section:

```json
"osm": {
  "defaultValue": 0.5,
  "rankAddress": { ... },
  "filters": [ ... ],
  "minLines": 30000
},
"matrikkel": {
  "addressPopularity": 1.0,
  "streetPopularity": 2.0,
  "rankAddress": 26,
  "minLines": 2000000
}
```

`build` enforces each source's config `minLines` (per source; for a
`municipality` run it applies to the run's total, not per municipality). The
single-source subcommands additionally accept `--min-lines <N>` to override the
config value for one run - useful for a smaller region where the national
baseline doesn't apply, with `--min-lines 0` disabling the check. When unset in
both places, no check is performed. The example config
(`converter.example.json`) omits `minLines` on purpose - it doubles as the
test fixture; see `geocoder/photon/import/config/converter-prod.json` for
production values.

### Source freshness (`--warn-if-stale`)

An advisory check for silently frozen upstreams: a rolling URL that stopped
updating, or a local file nobody refreshed. Pass `--warn-if-stale` to warn (on
stderr) about any resolved source older than 24 hours, or `--warn-if-stale=N`
to set the threshold in hours. Off unless the flag is given.

The age comes from:

- **URLs**: the server's `Last-Modified` header. It's stamped onto the
  downloaded file (like `curl -R`), so a warm-cache run still reports the true
  upstream date rather than when the cache was populated. A server that sends no
  `Last-Modified` falls back to the download time (so a freshly fetched file
  never looks stale).
- **Local files** (and ZIP archives): the file's modification time. For a ZIP
  it's the archive's date, not the extracted entry's.

The check is purely a warning - it never changes the exit code or aborts the
run (unlike `--min-lines`). A source whose date can't be determined is reported
and skipped. Works with `build` (every configured source) and the single-source
subcommands. For a `municipality` belagenhet run, each municipality's archive is
checked as it downloads.

### Caching downloads

`-d <DIR>` (or `NOMINATIM_CACHE_DIR`) persists files downloaded by `build`
(`url`/`region`/`municipality` inputs) and reuses them on subsequent runs. For
ZIP sources, the extracted entry is cached too. With a warm cache, a
`municipality` build runs without `LANTMATERIET_*`.

```bash
nominatim-converter -d ~/.cache/nominatim-converter build -c converter.json -o out.ndjson -f
```

Rolling URLs like `Current_latest.zip` or `norway-latest.osm.pbf` silently
serve stale data from the cache. Pass `--refresh-cache` to force a
re-download. Or just `rm` the cache directory.

The cache directory is created with default umask permissions; use a
user-owned location, not a shared one.

### Boosting popular entities (usage)

`build` resolves the usage CSV automatically from the `usage` section's `input`
(`url`/`file`) and applies it to every source - no flag needed. The `-u <FILE>`
(`--usage`) flag is for the single-source subcommands and takes a local path
only. Either way it points at a semicolon-separated CSV that nudges popular
entities upward in the importance ranking:

```
id;name;usage
NSR:StopPlace:59872;Oslo S;139608
NSR:StopPlace:58366;Jernbanetorget;12304
...
```

- First field is the entity ID, last field is the usage count, anything in
  between is treated as a human-readable label and ignored. `id;usage` works
  too.
- Header row optional. Blank lines and `#` comments skipped.
- IDs use each source's native format (`NSR:StopPlace:N`,
  `KVE:PostalAddress:N`, `OSM:PointOfInterest:N`, etc.). Missing IDs and IDs
  at or below `usageFloor` get factor 1.0 - no penalty.

The boost is `1 + alpha * log10(usage / usageFloor)`, applied as a
multiplicative factor on raw popularity *before* the log10 importance
normalization. Tune via the `usage` block in `converter.json`:

```json
"usage": { "alpha": 0.5, "usageFloor": 100 }
```

Defaults: `alpha=0.5`, `usageFloor=100`. With those, a stop with 10000x the
floor (~1M boardings) gets a ~3x popularity nudge - meaningful but bounded;
airports keep dominating the top by structural ranking.

## Entrance enrichment (OSM)

Large area features (military areas, parks, quarries, campuses) are emitted with their polygon
**centroid** as the coordinate, which is a poor routing destination - it can sit deep inside an
inaccessible area. For OSM ways and multipolygon relations you can substitute an
**entrance/gate** coordinate instead, per POI filter:

```json
{ "key": "landuse", "value": "military", "priority": 1, "useEntrance": true }
```

**`useEntrance`** - if the feature contains an entrance/gate node, emit that node's coordinate
instead of the centroid; features without one keep their centroid. Candidate nodes are
`entrance=*` (except `entrance=no`), `routing:entrance=*`, or a passable `barrier=*` (gate,
lift_gate, swing_gate, bollard, cycle_barrier, kissing_gate, block, chain). When a feature has
several, one is chosen by priority: an explicit `*=main` marker > a pedestrian `entrance=*` node >
a `barrier=*` gate node > a routable gate (on a `highway=*`) > the gate on the most major road;
ties broken by smaller node id.

It only applies to features at least `MIN_AREA_SIZE_METERS` (150 m) across their longer
bounding-box side; smaller features are always emitted unchanged. Selection is per-feature, so
every co-named parcel that physically contains the gate is enriched (not just one). Enrichment
runs only if at least one filter sets the flag, and the run log reports how many features were
enriched and the centroid->entrance distance distribution.

## Output format

NDJSON (newline-delimited JSON). First line is a header:

```json
{"type":"NominatimDumpFile","content":{"version":"0.1.0","generator":"geocoder",...}}
```

Subsequent lines are place entries:

```json
{"type":"Place","content":[{"place_id":"KVE-PostalAddress-225678815","object_type":"N","categories":[...],...}]}
```

All floating-point values are serialized with exactly 6 decimal places.

## Architecture

```
src/
├── main.rs                  # CLI entry point (clap)
├── config.rs                # converter.json schema
├── common/
│   ├── category.rs          # Category string constants
│   ├── coordinate.rs        # Lat/lon coordinate type
│   ├── country.rs           # ISO 3166-1 alpha-2/alpha-3 mapping (full set)
│   ├── extra.rs             # Extra metadata fields
│   ├── geo.rs               # Coordinate projection (UTM33, SWEREF99 TM → WGS84), country detection
│   ├── importance.rs        # Log-normalized importance scoring
│   ├── text.rs              # OSM tag formatting
│   ├── translator.rs        # Name/type translations
│   └── util.rs              # titleize, round6, etc.
├── source/
│   ├── stopplace/           # NeTEx StopPlace (xml, convert, popularity)
│   ├── matrikkel/           # Kartverket CSV addresses (parse, convert)
│   ├── stedsnavn/           # SSR GML place names (gml, convert)
│   ├── poi/                 # NeTEx POI (xml, convert)
│   ├── belagenhet/          # Lantmäteriet GeoPackage addresses (parse, convert, download)
│   └── osm/                 # OSM PBF 4-pass (passes, entity, admin, street, ...)
└── target/
    ├── json_writer.rs       # NDJSON output with header
    ├── nominatim_id.rs      # Structured ID → Photon place_id sanitization
    └── nominatim_place.rs   # Nominatim NDJSON schema (serde)
```

## Embedded data

- `data/boundaries60x30.ser` — Country boundary raster data, embedded in the binary via `include_bytes!`. Originally from [JOSM's boundaries.osm](https://josm.openstreetmap.de/browser/josm/trunk/resources/data/boundaries.osm), converted to `.ser` format using the [countryboundaries](https://github.com/westnordost/countryboundaries) generator.

## License

EUPL-1.2
