use crate::target::nominatim_place::*;
use chrono::Local;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

pub struct JsonWriter {
    writer: BufWriter<File>,
    count: usize,
}

impl JsonWriter {
    /// Open a writer for the given output path, writing the header if needed.
    pub fn open(output: &Path, is_appending: bool) -> Result<Self, Box<dyn std::error::Error>> {
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // A missing or empty file needs the header even when appending (a metadata error
        // means the file does not exist, hence unwrap_or(true)).
        let file_is_empty = std::fs::metadata(output).map(|m| m.len() == 0).unwrap_or(true);
        let needs_header = !is_appending || file_is_empty;

        let file = if needs_header {
            File::create(output)?
        } else {
            // Only reached when the file already exists and has content.
            OpenOptions::new().append(true).open(output)?
        };
        let mut writer = BufWriter::new(file);

        if needs_header {
            let header = NominatimHeader {
                type_: "NominatimDumpFile".to_string(),
                content: HeaderContent {
                    version: "0.1.0".to_string(),
                    generator: "geocoder".to_string(),
                    database_version: "0.3.6-1".to_string(),
                    data_timestamp: Local::now().to_rfc3339(),
                    features: Features {
                        sorted_by_country: true,
                        has_addresslines: false,
                    },
                },
            };
            serde_json::to_writer(&mut writer, &header)?;
            writeln!(writer)?;
        }

        Ok(Self { writer, count: 0 })
    }

    /// Write a single entry to the output.
    pub fn write_entry(&mut self, entry: &NominatimPlace) -> Result<(), Box<dyn std::error::Error>> {
        serde_json::to_writer(&mut self.writer, entry)?;
        writeln!(self.writer)?;
        self.count += 1;
        Ok(())
    }

    /// Number of entries written through this writer (excludes the header line).
    pub fn count(&self) -> usize {
        self.count
    }

    /// Batch export (convenience method used by non-OSM converters).
    /// Returns the number of entries written.
    pub fn export(
        entries: &[NominatimPlace],
        output: &Path,
        is_appending: bool,
    ) -> Result<usize, Box<dyn std::error::Error>> {
        let mut writer = Self::open(output, is_appending)?;
        for entry in entries {
            writer.write_entry(entry)?;
        }
        Ok(writer.count())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::extra::Extra;

    fn make_place(place_id: &str, name: &str) -> NominatimPlace {
        NominatimPlace {
            type_: "Place".to_string(),
            content: vec![PlaceContent {
                place_id: place_id.to_string(),
                object_type: "N".to_string(),
                object_id: 0,
                categories: vec!["source.nsr".to_string()],
                rank_address: 30,
                importance: RawNumber::from_f64_6dp(0.5),
                parent_place_id: None,
                name: Some(Name {
                    name: Some(name.to_string()),
                    name_en: None,
                    alt_name: None,
                }),
                address: Address::default(),
                housenumber: None,
                postcode: None,
                country_code: Some("no".to_string()),
                centroid: vec![10.0, 59.0],
                bbox: vec![],
                extra: Extra::default(),
            }],
        }
    }

    #[test]
    fn test_export_creates_header_and_entries() {
        let dir = std::env::temp_dir().join(format!("jw-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.ndjson");

        let entries = vec![make_place("1","Oslo S"), make_place("2","Bergen")];
        let written = JsonWriter::export(&entries, &path, false).unwrap();
        assert_eq!(written, 2); // count excludes the header line

        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3); // header + 2 entries
        assert!(lines[0].contains("NominatimDumpFile"));
        assert!(lines[1].contains("Oslo S"));
        assert!(lines[2].contains("Bergen"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_export_append_no_duplicate_header() {
        let dir = std::env::temp_dir().join(format!("jw-append-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.ndjson");

        // First write
        JsonWriter::export(&[make_place("1","Oslo")], &path, false).unwrap();
        // Append: the returned count is this run's entries only, not the file total.
        let appended = JsonWriter::export(&[make_place("2","Bergen")], &path, true).unwrap();
        assert_eq!(appended, 1);

        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3); // 1 header + 2 entries
        let header_count = lines.iter().filter(|l| l.contains("NominatimDumpFile")).count();
        assert_eq!(header_count, 1);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_export_each_line_is_valid_json() {
        let dir = std::env::temp_dir().join(format!("jw-json-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.ndjson");

        JsonWriter::export(&[make_place("1","Test")], &path, false).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        for line in content.lines() {
            let parsed: serde_json::Value = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("Invalid JSON: {e}\nLine: {line}"));
            assert!(parsed.is_object());
        }

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
