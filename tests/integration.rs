use std::path::{Path, PathBuf};
use std::process::Command;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_nominatim-converter"))
}

fn test_data(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("test-data")
        .join(name)
}

fn config_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("converter.example.json")
}

fn temp_output(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "nominatim-integration-{}-{name}.ndjson",
        std::process::id()
    ))
}

fn run_converter(args: &[&str]) -> (bool, String, String) {
    let output = Command::new(binary())
        .args(args)
        .output()
        .expect("failed to execute binary");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (output.status.success(), stdout, stderr)
}

/// Run the standard single-source invocation `<subcmd> -i <fixture> -o <temp out> -c
/// <example config> -f` and return (success, stderr, output path). Tests with extra or
/// different flags (append mode, --min-lines, missing args, build) call `run_converter`
/// directly.
fn convert_fixture(subcmd: &str, fixture: &str, tag: &str) -> (bool, String, PathBuf) {
    let output = temp_output(tag);
    let (success, _, stderr) = run_converter(&[
        subcmd,
        "-i",
        test_data(fixture).to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "-c",
        config_path().to_str().unwrap(),
        "-f",
    ]);
    (success, stderr, output)
}

fn read_ndjson(path: &Path) -> Vec<serde_json::Value> {
    let content = std::fs::read_to_string(path).expect("failed to read output");
    content
        .lines()
        .map(|line| serde_json::from_str(line).expect("invalid JSON line"))
        .collect()
}

fn cleanup(path: &Path) {
    let _ = std::fs::remove_file(path);
}

// ===== build (config-driven import) =====

/// A complete converter config: every source section present with a local `file` input
/// pointing at a test-data fixture.
fn build_config_json() -> String {
    let f = |name: &str| test_data(name).display().to_string();
    format!(
        r#"{{
  "osm": {{
    "input": {{ "file": "{osm}" }},
    "defaultValue": 1.0,
    "rankAddress": {{ "boundary": 10, "place": 20, "road": 26, "building": 28, "poi": 30 }},
    "filters": [{{"key": "amenity", "value": "hospital", "priority": 9}}]
  }},
  "stedsnavn": {{ "input": {{ "file": "{sted}" }}, "defaultValue": 40.0, "rankAddress": 16 }},
  "matrikkel": {{ "input": {{ "file": "{matr}" }}, "addressPopularity": 20.0, "streetPopularity": 20.0, "rankAddress": 26 }},
  "poi": {{ "input": {{ "file": "{poi}" }}, "importance": 0.5, "rankAddress": 30 }},
  "stopPlace": {{
    "input": {{ "file": "{stop}" }},
    "defaultValue": 50, "rankAddress": 30,
    "stopTypeFactors": {{ "busStation": 2.0 }},
    "interchangeFactors": {{ "preferredInterchange": 10.0 }},
    "fareZones": {{ "input": {{ "file": "{farezones}" }} }}
  }}
}}"#,
        osm = f("terningmoen.osm.pbf"),
        sted = f("Basisdata_3420_Elverum_25833_Stedsnavn_GML.gml"),
        matr = f("Basisdata_3420_Elverum_25833_MatrikkelenAdresse.csv"),
        poi = f("poi-test.xml"),
        stop = f("stopPlaces.xml"),
        farezones = f("fareZones.xml"),
    )
}

fn write_temp_config(name: &str, contents: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("nominatim-build-{}-{name}.json", std::process::id()));
    std::fs::write(&path, contents).expect("write temp config");
    path
}

#[test]
fn build_combines_configured_sources_into_one_file() {
    let config = write_temp_config("combine", &build_config_json());
    let output = temp_output("build-combine");
    let (success, _, stderr) = run_converter(&["build", "-c", config.to_str().unwrap(), "-o", output.to_str().unwrap(), "-f"]);
    assert!(success, "build failed: {stderr}");

    let lines = read_ndjson(&output);
    assert_eq!(lines[0]["type"], "NominatimDumpFile", "first line should be the header");
    // Appending five sources must not duplicate the header.
    let headers = lines.iter().filter(|l| l["type"] == "NominatimDumpFile").count();
    assert_eq!(headers, 1, "expected exactly one header, got {headers}");

    // Entries from several distinct sources should be present (matrikkel, stedsnavn,
    // poi, stopplace, osm all tag extra.source differently).
    let sources: std::collections::HashSet<String> = lines
        .iter()
        .filter_map(|l| l["content"][0]["extra"]["source"].as_str().map(str::to_string))
        .collect();
    // Each configured source must actually contribute (a silent drop-out would leave one
    // missing). OSM is intentionally excluded: the tiny test PBF matches no configured filter.
    for expected in ["kartverket-matrikkelenadresse", "kartverket-stedsnavn", "custom-poi", "nsr"] {
        assert!(sources.contains(expected), "missing entries from {expected}; got {sources:?}");
    }
    // OSM is configured but the tiny test PBF matches no filter, so it contributes nothing.
    assert!(!sources.contains("openstreetmap"), "test PBF should yield no OSM entries; got {sources:?}");

    // The configured fareZones input must reach the output, not just resolve.
    let zoned = lines.iter().filter_map(|l| l["content"][0]["extra"]["fare_zones"].as_str()).count();
    assert!(zoned > 0, "stop places should carry fare zones from the configured fareZones input");

    // The stedsnavn source is resolved once and reused as matrikkel's county GML; a matrikkel
    // entry with a populated county proves that wiring held.
    let matrikkel_has_county = lines.iter().any(|l| {
        l["content"][0]["extra"]["source"] == "kartverket-matrikkelenadresse"
            && l["content"][0]["address"]["county"].is_string()
    });
    assert!(matrikkel_has_county, "matrikkel county should be populated from the reused stedsnavn GML");

    cleanup(&output);
    let _ = std::fs::remove_file(&config);
}

#[test]
fn build_skips_omitted_sources() {
    // The headline behavior: a config with only some source sections builds cleanly and
    // produces exactly that subset (this is the sweden-test deployment shape).
    let f = |name: &str| test_data(name).display().to_string();
    let config_json = format!(
        r#"{{
  "stedsnavn": {{ "input": {{ "file": "{sted}" }}, "defaultValue": 40.0, "rankAddress": 16 }},
  "poi": {{ "input": {{ "file": "{poi}" }}, "importance": 0.5, "rankAddress": 30 }}
}}"#,
        sted = f("Basisdata_3420_Elverum_25833_Stedsnavn_GML.gml"),
        poi = f("poi-test.xml"),
    );
    let config = write_temp_config("skip", &config_json);
    let output = temp_output("build-skip");
    let (success, _, stderr) = run_converter(&["build", "-c", config.to_str().unwrap(), "-o", output.to_str().unwrap(), "-f"]);
    assert!(success, "partial build failed: {stderr}");

    let lines = read_ndjson(&output);
    let sources: std::collections::HashSet<String> = lines
        .iter()
        .filter_map(|l| l["content"][0]["extra"]["source"].as_str().map(str::to_string))
        .collect();
    assert!(sources.contains("kartverket-stedsnavn") && sources.contains("custom-poi"), "configured sources missing: {sources:?}");
    assert!(
        !sources.contains("kartverket-matrikkelenadresse") && !sources.contains("nsr"),
        "omitted sections must not contribute entries: {sources:?}"
    );
    cleanup(&output);
    let _ = std::fs::remove_file(&config);
}

#[test]
fn build_rejects_region_input_on_wrong_section() {
    // `region` is only valid for matrikkel/stedsnavn; on poi it must fail with a clear,
    // section-tagged error rather than silently doing something surprising.
    let config_json = r#"{ "poi": { "input": { "region": "03" }, "importance": 0.5, "rankAddress": 30 } }"#;
    let config = write_temp_config("bad-region", config_json);
    let output = temp_output("build-bad-region");
    let (success, _, stderr) = run_converter(&["build", "-c", config.to_str().unwrap(), "-o", output.to_str().unwrap(), "-f"]);
    assert!(!success, "region input on poi should fail");
    assert!(stderr.contains("region") && stderr.contains("poi"), "unexpected stderr: {stderr}");
    cleanup(&output);
    let _ = std::fs::remove_file(&config);
}

#[test]
fn build_rejects_source_section_without_input() {
    // A declared source section with no `input` is a hard error - you said you want the
    // source but didn't say where its data comes from.
    let config_json = r#"{ "poi": { "importance": 0.5, "rankAddress": 30 } }"#;
    let config = write_temp_config("no-input", config_json);
    let output = temp_output("build-no-input");
    let (success, _, stderr) = run_converter(&["build", "-c", config.to_str().unwrap(), "-o", output.to_str().unwrap(), "-f"]);
    assert!(!success, "poi section without input should fail");
    assert!(stderr.to_lowercase().contains("input"), "error should mention the missing input: {stderr}");
    cleanup(&output);
    let _ = std::fs::remove_file(&config);
}

#[test]
fn build_errors_when_no_sources_configured() {
    // An empty config declares no sources at all - distinct from a section missing its input.
    let config = write_temp_config("empty", "{}");
    let output = temp_output("build-empty");
    let (success, _, stderr) = run_converter(&["build", "-c", config.to_str().unwrap(), "-o", output.to_str().unwrap(), "-f"]);
    assert!(!success, "build with no sources should fail");
    assert!(stderr.contains("No data sources configured"), "unexpected stderr: {stderr}");
    cleanup(&output);
    let _ = std::fs::remove_file(&config);
}

#[test]
fn build_refuses_existing_output_without_force() {
    let config = write_temp_config("exists", &build_config_json());
    let output = temp_output("build-exists");
    std::fs::write(&output, "preexisting\n").unwrap();
    let (success, _, stderr) = run_converter(&["build", "-c", config.to_str().unwrap(), "-o", output.to_str().unwrap()]);
    assert!(!success, "build should refuse to clobber existing output without -f");
    assert!(stderr.contains("already exists"), "unexpected stderr: {stderr}");
    cleanup(&output);
    let _ = std::fs::remove_file(&config);
}

// ===== CLI behavior =====

#[test]
fn no_args_shows_help() {
    let (success, _, stderr) = run_converter(&[]);
    assert!(!success);
    assert!(
        stderr.contains("Usage") || stderr.contains("usage"),
        "expected usage info in stderr: {stderr}"
    );
}

#[test]
fn missing_input_file_fails() {
    let output = temp_output("missing-input");
    let (success, _, stderr) = run_converter(&[
        "stopplace",
        "-i",
        "/nonexistent/file.xml",
        "-o",
        output.to_str().unwrap(),
        "-c",
        config_path().to_str().unwrap(),
        "-f",
    ]);
    assert!(!success);
    assert!(
        stderr.contains("Error") || stderr.contains("error"),
        "expected error in stderr: {stderr}"
    );
    cleanup(&output);
}

#[test]
fn output_file_exists_without_force_fails() {
    let output = temp_output("exists-no-force");
    std::fs::write(&output, "existing content").unwrap();

    let (success, _, stderr) = run_converter(&[
        "stopplace",
        "-i",
        test_data("stopPlaces.xml").to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "-c",
        config_path().to_str().unwrap(),
    ]);
    assert!(!success);
    assert!(stderr.contains("already exists"));
    cleanup(&output);
}

// ===== StopPlace conversion =====

#[test]
fn stopplace_fare_zones_flag_adds_zone_categories() {
    let output = temp_output("stopplace-fare-zones");
    let (success, _, stderr) = run_converter(&[
        "stopplace",
        "-i",
        test_data("stopPlaces.xml").to_str().unwrap(),
        "--fare-zones",
        test_data("fareZones.xml").to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "-c",
        config_path().to_str().unwrap(),
        "-f",
    ]);
    assert!(success, "stopplace failed: {stderr}");
    let content = std::fs::read_to_string(&output).unwrap();
    assert!(content.contains("fare_zone_id.RUT.FareZone.4"), "expected derived fare zone categories");
    assert!(content.contains("fare_zone_authority.RUT.Authority.RUT"));
    cleanup(&output);
}

#[test]
fn stopplace_without_fare_zones_flag_has_no_fare_zones() {
    let (success, stderr, output) = convert_fixture("stopplace", "stopPlaces.xml", "stopplace-no-fare-zones");
    assert!(success, "stopplace failed: {stderr}");
    let content = std::fs::read_to_string(&output).unwrap();
    assert!(!content.contains("fare_zone_id."), "fare zones without -z");
    assert!(content.contains("tariff_zone_id."), "tariff zones come from the stop place input");
    cleanup(&output);
}

#[test]
fn stopplace_produces_valid_ndjson() {
    let (success, stderr, output) = convert_fixture("stopplace", "stopPlaces.xml", "stopplace-valid");
    assert!(success, "stopplace failed: {stderr}");

    let lines = read_ndjson(&output);
    assert!(lines.len() >= 2, "expected header + at least 1 entry, got {}", lines.len());

    // First line is the header
    assert_eq!(lines[0]["type"], "NominatimDumpFile");

    // All data lines are Place type
    for line in &lines[1..] {
        assert_eq!(line["type"], "Place");
    }

    cleanup(&output);
}

#[test]
fn min_lines_below_threshold_fails() {
    let output = temp_output("min-lines-fail");
    let (success, _, stderr) = run_converter(&[
        "stopplace",
        "-i",
        test_data("stopPlaces.xml").to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "-c",
        config_path().to_str().unwrap(),
        "-f",
        "--min-lines",
        "1000000",
    ]);
    assert!(!success, "expected failure when output is below --min-lines");
    assert!(
        stderr.contains("below the minimum"),
        "expected min-lines error in stderr: {stderr}"
    );
    cleanup(&output);
}

#[test]
fn min_lines_met_succeeds() {
    let output = temp_output("min-lines-ok");
    let (success, _, stderr) = run_converter(&[
        "stopplace",
        "-i",
        test_data("stopPlaces.xml").to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "-c",
        config_path().to_str().unwrap(),
        "-f",
        "--min-lines",
        "1",
    ]);
    assert!(success, "expected success when output meets --min-lines: {stderr}");
    cleanup(&output);
}

#[test]
fn stopplace_entries_have_required_fields() {
    let (success, stderr, output) = convert_fixture("stopplace", "stopPlaces.xml", "stopplace-fields");
    assert!(success, "stopplace failed: {stderr}");

    let lines = read_ndjson(&output);
    for entry in &lines[1..] {
        let content = &entry["content"][0];
        assert!(content["place_id"].is_string(), "missing place_id");
        assert!(content["object_type"].is_string(), "missing object_type");
        assert!(content["categories"].is_array(), "missing categories");
        assert!(content["rank_address"].is_i64(), "missing rank_address");
        assert!(content["importance"].is_f64(), "missing importance");
        assert!(content["centroid"].is_array(), "missing centroid");
        assert_eq!(content["centroid"].as_array().unwrap().len(), 2, "centroid should have 2 elements");

        let name = &content["name"];
        if !name.is_null() {
            assert!(name["name"].is_string(), "name.name should be a string");
        }

        let extra = &content["extra"];
        assert!(extra["source"].is_string(), "missing extra.source");
        assert!(extra["id"].is_string(), "missing extra.id");
    }

    cleanup(&output);
}

#[test]
fn stopplace_has_groups_and_stops() {
    let (success, _, output) = convert_fixture("stopplace", "stopPlaces.xml", "stopplace-groups");
    assert!(success);

    let lines = read_ndjson(&output);
    let data: Vec<&serde_json::Value> = lines[1..].iter().collect();

    let has_stop = data.iter().any(|e| {
        e["content"][0]["extra"]["source"]
            .as_str()
            .is_some_and(|s| s == "nsr")
    });
    assert!(has_stop, "expected at least one StopPlace entry");
    assert!(data.len() >= 2, "expected multiple stop place entries");

    cleanup(&output);
}

// ===== POI conversion =====

#[test]
fn poi_produces_valid_ndjson() {
    let (success, stderr, output) = convert_fixture("poi", "poi-test.xml", "poi-valid");
    assert!(success, "poi failed: {stderr}");

    let lines = read_ndjson(&output);
    assert!(lines.len() >= 2, "expected header + entries");
    assert_eq!(lines[0]["type"], "NominatimDumpFile");

    for entry in &lines[1..] {
        let content = &entry["content"][0];
        assert_eq!(
            content["extra"]["source"].as_str(),
            Some("custom-poi"),
            "poi entries should have source=custom-poi"
        );
    }

    cleanup(&output);
}

#[test]
fn poi_filters_expired_entries() {
    let (success, _, output) = convert_fixture("poi", "poi-test.xml", "poi-expired");
    assert!(success);

    let content = std::fs::read_to_string(&output).unwrap();
    // expired entry (id 2) and future entry (id 3) should be filtered
    assert!(
        !content.contains("TEST:TopographicPlace:2"),
        "expired entry should be filtered"
    );
    assert!(
        !content.contains("TEST:TopographicPlace:3"),
        "future entry should be filtered"
    );
    // valid entries should be present
    assert!(content.contains("TEST:TopographicPlace:1"));
    assert!(content.contains("TEST:TopographicPlace:4"));

    cleanup(&output);
}

// ===== Stedsnavn conversion =====

#[test]
fn stedsnavn_produces_valid_ndjson() {
    let (success, stderr, output) = convert_fixture("stedsnavn", "bydel.gml", "stedsnavn-valid");
    assert!(success, "stedsnavn failed: {stderr}");

    let lines = read_ndjson(&output);
    assert!(lines.len() >= 2, "expected header + entries");
    assert_eq!(lines[0]["type"], "NominatimDumpFile");

    for entry in &lines[1..] {
        let content = &entry["content"][0];
        assert_eq!(content["extra"]["source"].as_str(), Some("kartverket-stedsnavn"));
        assert_eq!(content["centroid"].as_array().unwrap().len(), 2);
    }

    cleanup(&output);
}

#[test]
fn stedsnavn_preserves_norwegian_diacritics() {
    let (success, _, output) = convert_fixture("stedsnavn", "bydel.gml", "stedsnavn-diacritics");
    assert!(success);

    let content = std::fs::read_to_string(&output).unwrap();
    assert!(
        content.contains("Grünerløkka"),
        "should preserve diacritics in place names"
    );

    cleanup(&output);
}

// ===== Matrikkel conversion =====

#[test]
fn matrikkel_produces_valid_ndjson() {
    let output = temp_output("matrikkel-valid");
    let (success, _, stderr) = run_converter(&[
        "matrikkel",
        "-i",
        test_data("Basisdata_3420_Elverum_25833_MatrikkelenAdresse.csv").to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "-c",
        config_path().to_str().unwrap(),
        "-g",
        test_data("Basisdata_3420_Elverum_25833_Stedsnavn_GML.gml").to_str().unwrap(),
        "-f",
    ]);
    assert!(success, "matrikkel failed: {stderr}");

    let lines = read_ndjson(&output);
    assert!(lines.len() > 100, "expected many entries for Elverum, got {}", lines.len());
    assert_eq!(lines[0]["type"], "NominatimDumpFile");

    cleanup(&output);
}

#[test]
fn matrikkel_has_addresses_and_streets() {
    let output = temp_output("matrikkel-types");
    let (success, _, _) = run_converter(&[
        "matrikkel",
        "-i",
        test_data("Basisdata_3420_Elverum_25833_MatrikkelenAdresse.csv").to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "-c",
        config_path().to_str().unwrap(),
        "-g",
        test_data("Basisdata_3420_Elverum_25833_Stedsnavn_GML.gml").to_str().unwrap(),
        "-f",
    ]);
    assert!(success);

    let lines = read_ndjson(&output);
    let data: Vec<&serde_json::Value> = lines[1..].iter().collect();

    let has_address = data.iter().any(|e| {
        e["content"][0]["extra"]["source"]
            .as_str()
            .is_some_and(|s| s == "kartverket-matrikkelenadresse")
    });
    assert!(has_address, "expected matrikkel address entries");
    assert!(data.len() >= 100, "expected many address entries");

    cleanup(&output);
}

#[test]
fn matrikkel_no_county_flag_works() {
    let output = temp_output("matrikkel-no-county");
    let (success, _, stderr) = run_converter(&[
        "matrikkel",
        "-i",
        test_data("Basisdata_3420_Elverum_25833_MatrikkelenAdresse.csv").to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "-c",
        config_path().to_str().unwrap(),
        "--no-county",
        "-f",
    ]);
    assert!(success, "matrikkel --no-county failed: {stderr}");

    let lines = read_ndjson(&output);
    assert!(lines.len() > 1, "expected output even without county data");

    cleanup(&output);
}

#[test]
fn matrikkel_without_gml_or_flag_fails() {
    let output = temp_output("matrikkel-no-gml");
    let (success, _, stderr) = run_converter(&[
        "matrikkel",
        "-i",
        test_data("Basisdata_3420_Elverum_25833_MatrikkelenAdresse.csv").to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "-c",
        config_path().to_str().unwrap(),
        "-f",
    ]);
    assert!(!success);
    assert!(
        stderr.contains("requires -g") || stderr.contains("no-county"),
        "expected helpful error about missing GML: {stderr}"
    );
    cleanup(&output);
}

// ===== Belägenhetsadress conversion =====

#[test]
fn belagenhet_produces_valid_ndjson() {
    let (success, stderr, output) = convert_fixture("belagenhet", "belagenhetsadresser_kn0305.gpkg", "belagenhet-valid");
    assert!(success, "belagenhet failed: {stderr}");

    let lines = read_ndjson(&output);
    assert!(lines.len() >= 2, "expected header + entries, got {}", lines.len());
    assert_eq!(lines[0]["type"], "NominatimDumpFile");

    for entry in &lines[1..] {
        assert_eq!(entry["type"], "Place");
        let content = &entry["content"][0];
        assert!(content["place_id"].is_string(), "missing place_id");
        assert!(content["categories"].is_array(), "missing categories");
        assert_eq!(content["country_code"].as_str(), Some("se"));
    }

    cleanup(&output);
}

#[test]
fn belagenhet_has_addresses_and_streets() {
    let (success, _, output) = convert_fixture("belagenhet", "belagenhetsadresser_kn0305.gpkg", "belagenhet-types");
    assert!(success);

    let lines = read_ndjson(&output);
    let data: Vec<&serde_json::Value> = lines[1..].iter().collect();

    let address_count = data
        .iter()
        .filter(|e| {
            e["content"][0]["categories"]
                .as_array()
                .is_some_and(|cats| cats.iter().any(|c| c.as_str() == Some("layer.address")))
        })
        .count();

    let street_count = data
        .iter()
        .filter(|e| {
            e["content"][0]["categories"]
                .as_array()
                .is_some_and(|cats| cats.iter().any(|c| c.as_str() == Some("layer.street")))
        })
        .count();

    assert!(address_count > 0, "expected address entries, got 0");
    assert!(street_count > 0, "expected street entries, got 0");
    // Test data has 10 valid addresses and 8 unique streets
    assert_eq!(address_count, 10, "expected 10 addresses");
    assert_eq!(street_count, 8, "expected 8 streets");

    cleanup(&output);
}

#[test]
fn belagenhet_filters_non_current_addresses() {
    let (success, _, output) = convert_fixture("belagenhet", "belagenhetsadresser_kn0305.gpkg", "belagenhet-filter");
    assert!(success);

    let content = std::fs::read_to_string(&output).unwrap();
    // "Reserverad" entries (fid 26004096, 26004735) should be filtered out
    assert!(
        !content.contains("Biskops-Arnövägen"),
        "Reserverad address should be filtered out"
    );
    assert!(
        !content.contains("Hjalmars väg"),
        "Reserverad address with no postort should be filtered out"
    );
    // Valid entries should be present
    assert!(content.contains("Bastubacken"));
    assert!(content.contains("Tinbacken"));

    cleanup(&output);
}

#[test]
fn belagenhet_entries_have_valid_coordinates() {
    let (success, _, output) = convert_fixture("belagenhet", "belagenhetsadresser_kn0305.gpkg", "belagenhet-coords");
    assert!(success);

    let lines = read_ndjson(&output);
    for entry in &lines[1..] {
        let centroid = entry["content"][0]["centroid"].as_array().unwrap();
        let lon = centroid[0].as_f64().unwrap();
        let lat = centroid[1].as_f64().unwrap();
        // All test data is in Sweden (roughly 55-70°N, 10-25°E)
        assert!(
            (10.0..=25.0).contains(&lon) && (55.0..=70.0).contains(&lat),
            "coordinates should be in Sweden: [{lon}, {lat}]"
        );
    }

    cleanup(&output);
}

#[test]
fn belagenhet_entries_have_correct_source() {
    let (success, _, output) = convert_fixture("belagenhet", "belagenhetsadresser_kn0305.gpkg", "belagenhet-source");
    assert!(success);

    let lines = read_ndjson(&output);
    for entry in &lines[1..] {
        let source = entry["content"][0]["extra"]["source"].as_str().unwrap();
        assert_eq!(source, "lantmateriet-belagenhetsadress");
    }

    cleanup(&output);
}

#[test]
fn belagenhet_housenumber_with_letter_suffix() {
    let (success, _, output) = convert_fixture("belagenhet", "belagenhetsadresser_kn0305.gpkg", "belagenhet-hn");
    assert!(success);

    let content = std::fs::read_to_string(&output).unwrap();
    // Skogsvägen 42A has bokstavstillagg = "A"
    assert!(
        content.contains("42A"),
        "housenumber with letter suffix should be combined: expected '42A'"
    );

    cleanup(&output);
}

// ===== Append mode =====

#[test]
fn append_mode_does_not_duplicate_header() {
    let output = temp_output("append-header");
    // First write
    let (s1, _, _) = run_converter(&[
        "poi",
        "-i",
        test_data("poi-test.xml").to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "-c",
        config_path().to_str().unwrap(),
        "-f",
    ]);
    assert!(s1);

    // Append
    let (s2, _, _) = run_converter(&[
        "poi",
        "-i",
        test_data("poi-test.xml").to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "-c",
        config_path().to_str().unwrap(),
        "-a",
    ]);
    assert!(s2);

    let content = std::fs::read_to_string(&output).unwrap();
    let header_count = content
        .lines()
        .filter(|l| l.contains("NominatimDumpFile"))
        .count();
    assert_eq!(header_count, 1, "header should appear exactly once after append");

    cleanup(&output);
}

// ===== Force overwrite =====

#[test]
fn force_flag_overwrites_existing_output() {
    // temp_output is deterministic per tag, so the helper reuses this exact path.
    std::fs::write(temp_output("force-overwrite"), "garbage content").unwrap();

    let (success, _, output) = convert_fixture("poi", "poi-test.xml", "force-overwrite");
    assert!(success);

    let content = std::fs::read_to_string(&output).unwrap();
    assert!(!content.contains("garbage"));
    assert!(content.contains("NominatimDumpFile"));

    cleanup(&output);
}

// ===== Output coordinates are valid =====

#[test]
fn all_centroids_are_valid_coordinates() {
    let (success, _, output) = convert_fixture("stopplace", "stopPlaces.xml", "centroids-valid");
    assert!(success);

    let lines = read_ndjson(&output);
    for entry in &lines[1..] {
        let centroid = entry["content"][0]["centroid"].as_array().unwrap();
        let lon = centroid[0].as_f64().unwrap();
        let lat = centroid[1].as_f64().unwrap();
        assert!(
            (-180.0..=180.0).contains(&lon) && (-90.0..=90.0).contains(&lat),
            "invalid centroid: [{lon}, {lat}]"
        );
    }

    cleanup(&output);
}

// ===== OSM entrance/gate enrichment =====

/// Find the record whose `extra.id` ends with the given OSM id and return its centroid [lon, lat].
fn centroid_for_osm_id(lines: &[serde_json::Value], osm_id: i64) -> Option<(f64, f64)> {
    let suffix = format!(":{osm_id}");
    lines.iter().skip(1).find_map(|entry| {
        let content = &entry["content"][0];
        let id = content["extra"]["id"].as_str()?;
        if id.ends_with(&suffix) {
            let c = content["centroid"].as_array()?;
            Some((c[0].as_f64()?, c[1].as_f64()?))
        } else {
            None
        }
    })
}

#[test]
fn osm_entrance_enrichment_substitutes_gate_for_containing_features() {
    let output = temp_output("osm-entrance");
    let config = test_data("converter-entrance.json");
    let (success, _, stderr) = run_converter(&[
        "osm",
        "-i",
        test_data("terningmoen.osm.pbf").to_str().unwrap(),
        "-o",
        output.to_str().unwrap(),
        "-c",
        config.to_str().unwrap(),
        "-f",
    ]);
    assert!(success, "osm conversion failed: {stderr}");

    let lines = read_ndjson(&output);

    // Both parcels that physically contain the gate are enriched to the lift_gate node
    // (60.8750, 11.5600), NOT their polygon centroids. useEntrance applies per-feature, so the
    // co-named fence (518127311) and landuse parcel (518428220) both get the gate.
    for id in [518127311, 518428220] {
        let (lon, lat) = centroid_for_osm_id(&lines, id)
            .unwrap_or_else(|| panic!("feature {id} should be present"));
        assert!((lon - 11.5600).abs() < 1e-4, "{id} lon should be the gate's, got {lon}");
        assert!((lat - 60.8750).abs() < 1e-4, "{id} lat should be the gate's, got {lat}");
    }

    // A large area mapped as a multipolygon RELATION (9000001 "Terningmoen Skytefelt") is enriched
    // to its own gate node (60.9050, 11.5200), proving relation features are handled too.
    let (rlon, rlat) = centroid_for_osm_id(&lines, 9000001)
        .expect("relation 9000001 should be present");
    assert!((rlon - 11.5200).abs() < 1e-4, "relation lon should be the gate's, got {rlon}");
    assert!((rlat - 60.9050).abs() < 1e-4, "relation lat should be the gate's, got {rlat}");

    // The co-named outer parcel 50537344 does NOT contain the gate, so it keeps its own polygon
    // centroid (~60.862, 11.524) -- features without a gate are emitted unchanged.
    let (lon2, lat2) = centroid_for_osm_id(&lines, 50537344)
        .expect("parcel 50537344 should still be present (only useEntrance, no drop)");
    assert!((lon2 - 11.5240).abs() < 1e-3, "parcel 50537344 should keep its centroid, got {lon2}");
    assert!((lat2 - 60.8620).abs() < 1e-3, "parcel 50537344 should keep its centroid, got {lat2}");
    assert!((lon2 - 11.5600).abs() > 1e-3, "parcel 50537344 must not be moved to the gate");

    cleanup(&output);
}
