# AGENTS.md

Instructions for AI coding agents working on this codebase.

## Project overview

This is a Rust CLI that converts geographic data into Nominatim NDJSON. It is a rewrite of an earlier converter and must produce **identical output**. Any behavioral change should be validated against the original version.

## Build and test

```bash
cargo build --release    # PROJ is statically linked via bundled_proj
cargo test --release     # run all tests
cargo clippy --release   # should produce zero warnings
```

The release build uses LTO (`[profile.release] lto = true`).

## Commands

Two ways in (`src/main.rs`):

- **`build`** is the production entry point. It reads the config, converts every source whose section is present, and appends them all into one NDJSON in a fixed order (matrikkel, stedsnavn, poi, stopplace, osm, belagenhet). A source section is present only when you want that source, and a present section **must** carry an `input` (a missing `input` is a parse error); omit the section to skip the source. `build` owns all downloading and ordering, so the import shell script is just "call `build`."
- **Per-source subcommands** (`stopplace`, `matrikkel`, `osm`, `stedsnavn`, `poi`, `belagenhet`) convert a single **local** file via `-i`. They do not download - that is `build`'s job. They exist for tests and ad-hoc debugging. `regions`/`municipalities` print the codes accepted by `region`/`municipality` inputs.

A source's location is `config.<source>.input`, a `SourceInput` (`src/config.rs`): `{ "url": ... }`, `{ "file": ... }`, `{ "region": ... }` (Geonorge, matrikkel/stedsnavn only), or `{ "municipality": ... }` (Lantmäteriet, belagenhet only). `region`/`municipality` on the wrong section is a build-time error, not a type error. The usage CSV is modelled the same way: a `usage` section with an `input` field (plus optional `alpha`/`usageFloor`), present to enable the boost. `build` resolves the stedsnavn source once and reuses it as matrikkel's county GML, so a region/URL is fetched only once.

## Key design decisions

### Output must match the original converter exactly (unless `--usage` is in play)

This is the most important constraint when running without `--usage`. Specifically:

- **place_id values** are sanitized and truncated to Photon's `[0-9a-zA-Z_-]{1,60}` in `src/target/nominatim_id.rs`. IDs containing non-ASCII characters are transliterated via the shared table (Å -> Aa etc.) for readability and suffixed with a pinned FNV-1a hash of the original ID. The hash is required: place_id is Photon's OpenSearch document id, so collisions silently drop entries, and transliteration alone can collide with literal spellings (Åsenvegen vs a real Aasenvegen). Hash values are stable across Rust releases (test-vectored in `common::util`).
- **Floating-point formatting** uses exactly 6 decimal places for `importance`, `centroid`, and `bbox` fields (`src/target/nominatim_place.rs`). This is enforced via custom serde serializers using `serde_json::value::RawValue`.
- **Country detection** uses `boundaries60x30.ser`, embedded via `include_bytes!` (`src/common/geo.rs`). This file originates from [JOSM's boundaries.osm](https://josm.openstreetmap.de/browser/josm/trunk/resources/data/boundaries.osm), manually edited for border accuracy and stored in [entur/geocoder-data](https://github.com/entur/geocoder-data), then converted to `.ser` using the [countryboundaries](https://github.com/westnordost/countryboundaries) generator. Do not switch to the Rust crate's built-in data — it produces different results for border cases.
- **Country code mapping** covers all ISO 3166-1 countries (`src/common/country.rs`). Do not reduce to a subset.

### Optional usage-driven popularity boosts (`--usage`)

The global `--usage <FILE>` CLI flag points at a semicolon-separated CSV (`id;name;usage`, or just `id;usage` - the middle column is purely human-friendly and ignored) that nudges popular entities upward in the ranking (`src/common/usage.rs`). The boost is `1 + alpha * log10(usage / usageFloor)` (defaults: alpha=0.5, floor=100), applied as a multiplicative factor on each source's raw popularity *before* `ImportanceCalculator` runs. Missing IDs and IDs at or below the floor receive factor 1.0.

The CSV is shared across every subcommand. Each source converter looks up by its own ID format (`NSR:StopPlace:N`, `KVE:PostalAddress:N`, `OSM:PointOfInterest:N`, etc.) so a single file can carry signals for multiple sources.

The canonical CSV is generated from PostHog by the `posthog-popular-stops` job in [`geocoder/.github/workflows/cache-data-sources.yml`](../geocoder/.github/workflows/cache-data-sources.yml) and uploaded to `gs://ent-geocoder-prd/data-sources/popular-stops.csv`. The job merges both boardings (`fra`) and alightings (`til`) PostHog insights, summing usage per id.

`build` resolves the CSV from the `usage` section's `input` automatically (downloading/caching like any other source). The `--usage <FILE>` flag only accepts a local path, so for a single-source subcommand, download first:

```bash
gcloud storage cp gs://ent-geocoder-prd/data-sources/popular-stops.csv .
nominatim-converter --usage popular-stops.csv stopplace -i stops.xml -o stops.ndjson -c converter.json
```

(An explicit `--usage` path also overrides the `usage` section's `input` for a `build` run.)

GoSP popularity is the product of its member stops' (boosted) popularities, then run through `ImportanceCalculator::calculate_importance`, which clamps the output to `[floor, 1.0]` per the Nominatim 0-1 specification. Since the product saturates to 1.0 for any busy hub, non-secondary GoSP importance is then capped at `GOSP_IMPORTANCE_CAP` (0.92) so a city GoSP doesn't outrank an exact match on one of its own member stops ("Oslo" over "Oslo bussterminal"). Far-focus major cities outrank near-focus same-prefix streets via the geocoder proxy's request-side weight defaults (Photon `location_bias_scale ~ 0.5`), not via importance values above 1.

GoSP IDs listed in `stopPlace.groupOfStopPlaces.secondaryGosps` are demoted in autocomplete via two converter-side levers (no user-visible field is mutated): (1) importance is pinned to `SECONDARY_GOSP_IMPORTANCE` (0.001, hardcoded - must be strictly positive because Photon's `function_score` drops docs whose total collapses to zero), and (2) `rank_address` is set to 0, which maps the doc to Photon's `AddressType.OTHER` so it forfeits the +0.4 weight `SearchQueryBuilder.setupShortQuery` gives non-"other" docs. The list is explicit rather than heuristic-detected because the redundant-aggregator pattern (e.g. NSR:GroupOfStopPlaces:7 "Bergen" coexisting with GoSP:174 "Bergen sentrum") is hard to distinguish from canonical city aggregators that just happen to have a sibling. Today only GoSP:7 is configured.

### Fare zones come from their own NeTEx export

Fare zones are **not** read from the stop place NeTEx. Their source is
`https://api.entur.io/distance/netex/fare-zones` (the Distances and zones API, an open
endpoint that 302s to a signed GCS URL), configured as `stopPlace.fareZones.input` for `build`,
or passed as `--fare-zones` to the `stopplace` subcommand. NSR still mirrors fare
zones into each stop's `<tariffZones>` as `:FareZone:`-shaped refs; with an export loaded those
are ignored (`tariff_zone_refs` in `src/source/stopplace/convert.rs`), since they are due to
disappear from the NSR export anyway. Without one they are the fallback: the mirrored refs
become the fare zone IDs (sorted and deduped, as the export path's are), and their authorities
come from the same input's own `<FareFrame>`, which declares all 485 zones with an
`AuthorityRef`. The authority is not derivable from the zone prefix (`FIN:FareZone:31` belongs
to `FIN:Authority:FIN_ID`), so it has to be read from a file either way.

`ZoneSource` in `convert.rs` resolves this once per run, before any stop is converted, and the
two sources never mix: with an export configured the `<FareFrame>` is not consulted at all, and
a stop the export places in no zone stays zone-less rather than falling back. Tariff zones still come from NSR - they are being retired too,
but they cannot be derived from fare zones: the two systems disagree on membership for ~12k
stops, and a handful of tariff zones have no fare zone counterpart.

The export carries zone geometry, not stop references, so membership is derived per stop in
`src/source/stopplace/farezone.rs`:

- `ScopingMethod = explicitStops`: the zone's `<members>` (`NSR:ScheduledStopPoint:S<n>` is
  `NSR:StopPlace:<n>`, verified against every PassengerStopAssignment). **Its outline must be
  ignored** - those outlines cover far more stops than the zone has members, and honouring them
  mis-assigns ~10k stops. Nine zones scope this way and list nobody; they match nothing, and say
  so on stderr.
- members name quays' stop places, never the multimodal parent above them, so a stop's zones are
  unioned with its children's (`zones_with_children`). Without that, 49 of 1,100 parents lacked a
  zone their own children were in, 35 of them via `explicitStops` - and because the proxy
  defaults to `multimodal=parent`, a zone filter dropped the hub and returned only its children.
  Downward only: a child never inherits its parent's or a sibling's zones.
- everything else: point-in-polygon of the stop centroid against the outline, via the shared
  `common::geometry` helpers.

Checked against NSR's own assignment: one stop differs, where NSR held a stale zone version.

Silent degradation is the risk that shapes the error handling here, because a zone-less index
looks healthy to every downstream check (`minLines` counts stop places, which are unaffected).
So an export that yields zero zones is a hard error, and a truncated download fails on the
content-length check in `common::input`. Malformed geometry fails rather than degrades - an odd
or unparsable `posList` would re-pair every coordinate, and a second outline would silently
shrink a zone to one part.

The NSR fallback is the one place that risk is accepted, and it holds up better than its name
suggests. Diffed over a full run on 2026-08-26, the zone ID sets were identical on all 58,139
records, multi-zone membership included; the authorities read from the `<FareFrame>` match the
export on all 485 zones. That is structural rather than lucky - Tiamat holds the same zone
definitions and computes the assignment itself, and the distance API republishes Tiamat's data,
so the fallback is Tiamat's own answer for the same zones, not an independent guess. Zone
versions can be ignored: the emitted category is version-less, IDs are stable across versions,
and the two publishers version independently (the distance export runs 1-11 ahead on
byte-identical content), so there is nothing to be stale against.

What remains: a one-cycle lag if a geometry changes between Tiamat's recomputation and the next
NSR dump (self-healing, historically 0-1 stops), no validation that a mirrored ref names a zone
that exists, and the refs' pending removal from the NSR export. That last one is why
`fallback_warning` separates "falling back on N of M stop places" from "no `:FareZone:` refs at
all" - the second is the shape Sweden and Denmark already run
(`converter-{sweden,denmark}-test.json` set no `fareZones`, and neither input holds a single
`:FareZone:` ref), and the shape Norway takes the day the refs disappear. Nothing else would
catch it: `minLines` counts stop places, which are unaffected.

Zone data changes a few times a month, so `build` resolves it with a 30x `--warn-if-stale`
threshold; the run-wide value is tuned for daily sources. It is resolved before the multi-GB
sources so a fetch failure costs nothing. Requests to `api.entur.io` carry Entur's
`ET-Client-Name` header alongside the User-Agent.

### Optional per-source minimum line count (`minLines` / `--min-lines`)

Each source config section takes an optional `minLines` threshold; if a conversion writes fewer entries, it exits non-zero (a tripwire against empty/truncated downloads shipping a degraded index). In `build`, each source's `minLines` is enforced from its config section. The single-source subcommands additionally accept `--min-lines <N>`, which wins over `minLines.<source>` (and `--min-lines 0` disables the check for one run); unset in both means no check. Two non-obvious semantics: the count is the entries emitted by **this run** (tracked in `JsonWriter`, returned up through each converter as `Result<usize>`), so it is correct in `-a` append mode; and for a `municipality` build (multiple municipalities) the threshold applies to the **run total**, not per municipality. Thresholds are profile-specific: `geocoder/photon/import/config/converter-prod.json` holds Norway values, and `converter-sweden-test.json` holds Sweden values (much larger belagenhet, no Norway-only sources). Each per-environment `converter-*.json` is a complete profile (sources + scoring), picked by name by the import script. `converter.example.json` deliberately omits `minLines` (it doubles as the test fixture).

### Optional source freshness warning (`--warn-if-stale`)

Global, off unless given: warns (stderr, never a nonzero exit) about any resolved source older than 24h, or `=N` hours. Advisory, unlike `--min-lines`. In `src/common/input.rs`: `ResolvedInput.last_modified` is the *source* mtime (local file, or downloaded/cached raw file - not the extracted temp, whose mtime is "now"). For URLs it comes from `Last-Modified` (`parse_http_date`: IMF-fixdate only) stamped onto the file via `set_file_mtime` (curl -R style) so warm-cache hits report the upstream date; missing header leaves it at download time (never stale). Threshold plumbed via the `RunOptions` struct, no global state.

### Coordinate conversions have inherent precision differences

UTM33 (EPSG:25833) → WGS84 conversions use the `proj` crate, which produces results that differ from the original converter at the 6th decimal place (~0.1m). This is accepted as unavoidable — the difference is sub-meter.

### OSM converter specifics

The OSM converter (`src/source/osm/`) has several critical patterns for output compatibility:

- **PBF file order**: Entities must be processed in PBF file order, not HashMap iteration order. The pass 4 data structs use `ids: Vec<i64>` to preserve insertion order alongside `HashMap` lookups. Do not iterate over the HashMaps directly.
- **BTreeMap for filtered tags**: `filter_tags()` returns `BTreeMap<&str, &str>` (sorted by key) to match the original converter's alphabetical ordering. Using `HashMap` causes non-deterministic category ordering.
- **Alt names from filtered tags**: `alt_name`, `old_name`, etc. are looked up from the filtered tags (BTreeMap), not all_tags (HashMap). This matches the original converter's `filterTags()` behavior.
- **RefCell for StreetIndex cache**: `lookup_cache` uses `RefCell<HashMap>` for interior mutability so `find_nearest_street` can take `&self` instead of `&mut self`.
- **CoordinateStore at 1e5 scale**: The custom hash-based coordinate store uses 1e5 precision (~1.1m). Do not increase — it causes more diffs, not fewer, because it affects all coordinates including polygon centroid averaging.
- **4-pass PBF processing**: Relations → Ways → Nodes → Convert. This is critical for collecting the dependency graph (relations need way IDs, ways need node IDs).

### OSM address inheritance

A POI without its own `addr:street` inherits an address via `resolve_address` (`entity.rs`), in priority order: (1) own `addr:street` + `addr:housenumber`; (2) the addressed polygon that contains the POI centroid; (3) the nearest standalone address node within 20 m; (4) nearest road-segment name, never with a housenumber. Containment (2) is what makes an inherited housenumber trustworthy, unlike the bare nearest-street match. A POI's own `addr:housenumber` (which alone is dropped by 1) is preferred over an inherited number in 2 and 3.

Two grid indexes back this (`address_index.rs`), both mirroring `StreetIndex`: `AddressPolygonIndex` (closed `building` ways carrying `addr:street`, collected in pass 2, ray-cast containment via `geometry::ray_cast_contains`, smallest bounding box wins, ties by way id) and `AddressNodeIndex` (nodes with `addr:street` + `addr:housenumber`, collected in pass 3, nearest within 20 m via a fixed 3x3 grid scan, ties by node id). Only `building` ways are treated as addressed polygons; non-building addressed areas (landuse, parking, ...) are excluded since one address rarely covers the whole area. Multipolygon-relation buildings are not indexed yet. Retaining these adds node coordinates beyond the usual filtered subset (bounded to addressed buildings + address nodes); expect a few hundred MB extra on a full Norway run.

### Performance-sensitive code

- `geo::convert_utm33_to_lat_lon` caches the `Proj` instance in `thread_local!` storage. Creating a `Proj` per call is ~1000x slower. The `Proj` type is not `Send+Sync`, so `LazyLock` cannot be used.
- Matrikkel's `build_kommune_mapping` streams the GML via `BufReader` — do not use `read_to_string` on the 2.6GB file.
- Matrikkel parses the CSV once and reuses the vec for both address and street passes.
- OSM's StreetIndex uses grid-based spatial indexing (0.005° cells) with expanding ring search, plus a 0.001° lookup cache for repeated queries at similar coordinates.

## Project structure

- `src/common/` — Shared types and utilities (coordinates, countries, categories, text formatting)
- `src/source/` — One module per data source, each a thin facade (`name.rs`) with submodules (`name/`)
  - `stopplace/` — NeTEx StopPlace (xml, convert, popularity)
  - `matrikkel/` — Kartverket CSV addresses (parse, convert)
  - `stedsnavn/` — SSR GML place names (gml, convert)
  - `poi/` — NeTEx POI (xml, convert)
  - `osm/` — OSM PBF 4-pass (passes, pass4, entity, admin, street, address_index, popularity, coordinate, geometry, grid, indexing)
- `src/source.rs` — Module declarations + shared test helpers (`test_config`, `test_data_path`)
- `src/target/` — Output format (NDJSON schema, ID generation, JSON writer)
- `src/config.rs` — `converter.json` deserialization
- `data/` — Embedded binary data (country boundaries)

## Downstream pipeline context

This converter produces `nominatim.ndjson` which is imported into the **Photon geocoder**, proxied by `../geocoder/proxy`, and validated by `../geocoder-acceptance-tests/`. Understanding what the acceptance tests actually check helps prioritize what matters most in the converter output.

### Fields that acceptance tests validate

- **name / alt_name** — Fuzzy search, popular name matching (e.g. "gardermoen" → "Oslo lufthavn"). Norwegian diacritics (ø, å, æ, ü) must be preserved.
- **categories** — Layer/category filtering (`onstreetBus`, `railStation`, `airport`, `busStation`). Multi-modal stops must include all transport modes.
- **housenumber** — Address searches like "karl johans gate 2" depend on correct housenumber extraction.
- **source (extra field)** — Acceptance tests filter by data source (`openaddresses`, `openstreetmap`). Source tags must match expected values.
- **importance** — Directly affects result ranking. Acceptance tests use `priorityThresh` to verify top-N placement.
- **county_gid / locality_gid (extra fields)** — Used for `boundary.county_ids` filtering. Must support both full (`KVE:TopographicPlace:18`) and numeric (`18`) formats.
- **tariff_zones / fare_zones (extra fields)** — Used for zone-based filtering downstream. Tariff zones come from the stop place NeTEx; fare zones from the separate fare zone export.
- **centroid coordinates** — Reverse geocoding, focus-point disambiguation, and distance calculations all depend on coordinate accuracy.

### Acceptance test patterns worth knowing

- **Geographic disambiguation**: Same place name in multiple locations (e.g. "Haugen") — focus points select the closest. Correct coordinates are critical.
- **Data source priority**: NSR takes priority over WhosOnFirst for stop places. GroupOfStopPlaces rank above individual StopPlaces for major cities, except for "secondary" GoSPs whose name matches the locality and have a sibling - those are capped below real stops (see `identify_secondary_gosps`).
- **Popular vs official names**: "Gardermoen" (popular) should find "Oslo lufthavn" (official). Alt name deduplication and ordering matter.
- **House number edge cases**: Numbers can appear before street name ("10 schw"), with suffixes ("10B"), or after ("strandkaien 22").
- **Multi-modal categories**: Stavanger stasjon = railStation + onstreetBus. Oslo lufthavn = railStation + onstreetBus + busStation + airport. Category arrays must be complete.
- **Reverse geocoding should NOT return bare house numbers** — layer filtering depends on correct `object_type` and category assignment.

### Test coverage

All source converters have unit tests (`cargo test --release` runs ~240 tests). Coverage by module:

1. **stopplace** (59 tests): NeTEx parsing, fare zone membership (outline match, explicitStops members, ordering), popularity calculation (base × type factors × interchange), GroupOfStopPlaces popularity (product of member popularities), transport mode formatting (mode:submode, parent collecting children with dedup), alt name handling (label → visible, translation → indexed only), category generation (funicular included, bus excluded, multimodal.parent marker), zone category ordering, full conversion integration tests (coordinates, authority categories, county_gid/locality_gid, secondary-GoSP importance cap and rank_address)
2. **stedsnavn** (22 tests): Target type recognition (by/bydel/tettsted/tettsteddel/tettbebyggelse), spelling status filtering (vedtatt/godkjent/privat/samlevedtak accepted), GML parsing with historisk alt spelling, diacritics preservation, field validation (source, accuracy, country_code, importance, rank_address), locality/county GID format, coordinate ranges, titleized names
3. **matrikkel** (12 tests): CSV→NDJSON conversion, field validation (id, source, accuracy, country_a, locality, borough, housenumber with letter suffix), county population via stedsnavn GML, address + street entry generation, category correctness, coordinate validity, importance range, county GID in categories
4. **poi** (7 tests): ValidBetween date filtering (valid/expired/future/always-valid/open-ended), coordinate and category correctness
5. **integration** (34 tests, `tests/integration.rs`): Black-box binary tests via `std::process::Command`. CLI behavior (no args, missing input, output-exists-without-force), all subcommands produce valid NDJSON with correct headers/sources/fields, append mode doesn't duplicate headers, force flag overwrites, coordinate validity, matrikkel --no-county flag, matrikkel missing GML error, expired POI filtering, Norwegian diacritics; `build` combines configured sources into one file with a single header, skips omitted sections, rejects a section missing its `input`, rejects region-on-wrong-section, errors when no source is configured, and refuses an existing output without `-f`
6. **osm** (47 tests): Popularity formula (base × max priority, highest priority wins, unmatched/empty → zero), filter_tags (keeps only configured filters, sorted BTreeMap keys, empty for no matches), rank_address determination (boundary > place > road > building > poi priority), convert_node integration (object_type, accuracy, source, categories from filtered tags, alt name extraction from filtered tags only, en:name, OSM ID in extra and indexed alt_names, coordinates, importance reflects priority), admin boundary integration (county_gid, locality_gid, titleized municipality name, county_gid in categories), extract_country_code (ISO3166-2, country_code tag, numeric ref → Norway), as_category colon replacement, plus low-level tests (CoordinateStore, BoundingBox, ray casting, street segment distance, centroid calculation, titleize)

### Test data fixtures

- `test-data/stopPlaces.xml` — NeTEx with TopographicPlaces (counties/municipalities for topo lookups), 2 GroupOfStopPlaces, 6 StopPlaces (bus, rail, parent, child, alt names, submodes). Its `<FareFrame>` and the stops' `:FareZone:` refs are kept deliberately: NSR still ships them, and the tests prove the converter ignores them
- `test-data/fareZones.xml` — fare zone export: 3 spatial zones around the stop fixtures, one `explicitStops` zone whose outline covers both Oslo stops but which lists only one as a member, and one `explicitStops` zone with no members at all
- `test-data/poi-test.xml` — 5 TopographicPlaces with varying validity periods
- `test-data/bydel.gml` — 2 Oslo bydeler (Grünerløkka, Frogner) in UTM33
- `test-data/Basisdata_3420_Elverum_25833_MatrikkelenAdresse.csv` — Real Elverum address data (10,871 lines)
- `test-data/Basisdata_3420_Elverum_25833_Stedsnavn_GML.gml` — Kommune-fylke mapping for matrikkel tests

### Test patterns

- Shared test helpers live in `src/source.rs` (`test_config()`, `test_config_with_osm_filters()`, `test_data_path()`) — all modules import from there
- Stopplace-specific helpers (`make_stop_place`, etc.) live in `src/source/stopplace.rs` `tests::helpers`
- Temp output files use unique suffixes per test to avoid parallel test conflicts
- Integration tests (`tests/integration.rs`) run the binary as a black box via `std::process::Command`, testing all subcommands end-to-end
- Module-level integration tests call the module's `convert()` function end-to-end, then parse the NDJSON output

## Common pitfalls

- **XML tag names are case-sensitive**: `alternativeNames` not `AlternativeNames`, `parentSiteRef` has an `@ref` attribute (use `RefAttr` struct).
- **quick-xml `read_text` doesn't work with `Reader<BufReader<File>>`**: Use manual text collection with `Event::Text` instead.
- **Serde rename for XML attributes**: Use `#[serde(rename = "@ref")]` for XML attributes parsed by quick-xml.
- **Alt name deduplication must preserve order**: Use a `HashSet` seen-tracker with `Vec` output, not `BTreeSet` or `sort + dedup`.
- **Zone categories have specific ordering**: Built in 4 passes (tariff zone IDs, fare zone IDs, tariff zone authorities, fare zone authorities).
- **HashMap iteration is non-deterministic**: Never rely on HashMap iteration order for output. Use Vec for ordered processing, BTreeMap for sorted keys.
- **Street matching has edge cases**: The 100m threshold + 0.001° cache precision means ~0.1% of street lookups differ from the original converter due to coordinate quantization.
