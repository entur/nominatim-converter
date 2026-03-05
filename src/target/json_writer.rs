use crate::target::nominatim_place::*;
use chrono::Local;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

pub struct JsonWriter;

impl JsonWriter {
    pub fn export(
        entries: &[NominatimPlace],
        output: &Path,
        is_appending: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let needs_header = !is_appending || !output.exists() || std::fs::metadata(output).map(|m| m.len() == 0).unwrap_or(true);

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
            let file = File::create(output)?;
            let mut writer = BufWriter::new(file);
            serde_json::to_writer(&mut writer, &header)?;
            writeln!(writer)?;
        }

        let file = OpenOptions::new().create(true).append(true).open(output)?;
        let mut writer = BufWriter::new(file);
        for entry in entries {
            serde_json::to_writer(&mut writer, entry)?;
            writeln!(writer)?;
        }
        Ok(())
    }
}
