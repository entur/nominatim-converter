//! Fare zones from the NeTEx export at `https://api.entur.io/distance/netex/fare-zones`.
//!
//! The export has no stop references, so membership is derived: `explicitStops` zones use
//! their member list, every other zone matches by point-in-polygon. Using the outline of an
//! `explicitStops` zone would over-assign badly - see AGENTS.md.

use crate::common::coordinate::Coordinate;
use crate::common::geometry::{ray_cast_contains, BoundingBox};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::collections::HashSet;
use std::path::Path;

/// `NSR:ScheduledStopPoint:S<n>` is `NSR:StopPlace:<n>`, verified against every
/// PassengerStopAssignment in the stop place export. Quay-scoped `Q<n>` members would need a
/// quay lookup, so they are reported rather than silently dropped.
const SCHEDULED_STOP_POINT_PREFIX: &str = "NSR:ScheduledStopPoint:S";
const STOP_PLACE_PREFIX: &str = "NSR:StopPlace:";
const EXPLICIT_STOPS: &str = "explicitStops";

#[derive(Debug)]
pub(crate) struct FareZone {
    pub id: String,
    pub authority: Option<String>,
    version: u32,
    members: HashSet<String>,
    /// `ScopingMethod = explicitStops`: members only, outline ignored.
    explicit_stops: bool,
    /// `None` when the zone has no usable outline; it then reaches members only.
    bbox: Option<BoundingBox>,
    outline: Vec<Coordinate>,
}

impl FareZone {
    fn has_stop(&self, stop_id: &str, coord: &Coordinate) -> bool {
        if self.explicit_stops {
            self.members.contains(stop_id)
        } else {
            self.bbox.is_some_and(|b| b.contains(coord)) && ray_cast_contains(&self.outline, coord)
        }
    }
}

pub(crate) struct FareZones {
    zones: Vec<FareZone>,
}

impl FareZones {
    pub(crate) fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let mut zones = parse_fare_zones(&std::fs::read_to_string(path)?)?;
        // By ID so zones_for is stable, newest version first so the dedup keeps it.
        zones.sort_by(|a, b| a.id.cmp(&b.id).then(b.version.cmp(&a.version)));
        let parsed = zones.len();
        zones.dedup_by(|a, b| a.id == b.id);
        if zones.len() < parsed {
            eprintln!("warning: fare zone export holds {} duplicate zone IDs", parsed - zones.len());
        }
        // One line, not one per zone: today's export has nine, every build.
        let memberless: Vec<&str> = zones.iter()
            .filter(|z| z.explicit_stops && z.members.is_empty())
            .map(|z| z.id.as_str())
            .collect();
        if !memberless.is_empty() {
            eprintln!("warning: {} fare zones scope to explicit stops but list none: {}",
                memberless.len(), memberless.join(", "));
        }
        Ok(Self { zones })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.zones.is_empty()
    }

    /// The zones a stop place belongs to, ordered by zone ID.
    pub(crate) fn zones_for(&self, stop_id: &str, coord: &Coordinate) -> Vec<&FareZone> {
        self.zones.iter().filter(|z| z.has_stop(stop_id, coord)).collect()
    }
}

fn attr(e: &BytesStart<'_>, key: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.into_inner() == key)
        .map(|a| String::from_utf8_lossy(&a.value).into_owned())
}

#[derive(Default)]
struct ZoneBuilder {
    id: String,
    version: u32,
    authority: Option<String>,
    members: HashSet<String>,
    explicit_stops: bool,
    outline: Vec<Coordinate>,
}

impl ZoneBuilder {
    fn build(self) -> FareZone {
        FareZone {
            bbox: BoundingBox::from_coordinates(&self.outline),
            id: self.id,
            version: self.version,
            authority: self.authority,
            members: self.members,
            explicit_stops: self.explicit_stops,
            outline: self.outline,
        }
    }
}

/// The elements whose text is read. Both are leaves, so the next text event is the value.
#[derive(Clone, Copy)]
enum Field {
    ScopingMethod,
    PosList,
}

fn parse_fare_zones(xml: &str) -> Result<Vec<FareZone>, Box<dyn std::error::Error>> {
    let mut zones = Vec::new();
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut zone: Option<ZoneBuilder> = None;
    let mut in_members = false;
    let mut in_exterior = false;
    let mut field: Option<Field> = None;
    let mut quay_members = 0usize;
    let mut buf = Vec::new();

    loop {
        // Element names carry the producer's namespace prefix (ns2:Polygon), so only the
        // local name is stable to match on.
        match reader.read_event_into(&mut buf)? {
            Event::Start(ref e) => {
                field = None;
                match e.name().local_name().as_ref() {
                    b"FareZone" => {
                        zone = attr(e, b"id").map(|id| ZoneBuilder {
                            id,
                            version: attr(e, b"version").and_then(|v| v.parse().ok()).unwrap_or(0),
                            ..Default::default()
                        });
                    }
                    b"members" => in_members = true,
                    b"exterior" => in_exterior = true,
                    b"ScopingMethod" => field = Some(Field::ScopingMethod),
                    b"posList" if in_exterior => field = Some(Field::PosList),
                    _ => {}
                }
            }
            Event::Empty(ref e) => match e.name().local_name().as_ref() {
                b"ScheduledStopPointRef" if in_members => {
                    if let (Some(z), Some(ref_)) = (zone.as_mut(), attr(e, b"ref")) {
                        match ref_.strip_prefix(SCHEDULED_STOP_POINT_PREFIX) {
                            Some(n) => { z.members.insert(format!("{STOP_PLACE_PREFIX}{n}")); }
                            None => quay_members += 1,
                        }
                    }
                }
                b"AuthorityRef" => {
                    if let Some(z) = zone.as_mut() {
                        z.authority = attr(e, b"ref");
                    }
                }
                _ => {}
            },
            Event::Text(ref e) => {
                if let (Some(z), Some(f)) = (zone.as_mut(), field.take()) {
                    let text = String::from_utf8_lossy(e.as_ref());
                    match f {
                        Field::ScopingMethod => z.explicit_stops = text.trim() == EXPLICIT_STOPS,
                        // Only the first ring is kept, so a second one would silently shrink
                        // the zone to one part.
                        Field::PosList if !z.outline.is_empty() => {
                            return Err(format!("fare zone {} has more than one outline", z.id).into());
                        }
                        Field::PosList => z.outline = parse_pos_list(&text, &z.id)?,
                    }
                }
            }
            Event::End(ref e) => match e.name().local_name().as_ref() {
                b"FareZone" => zones.extend(zone.take().map(ZoneBuilder::build)),
                b"members" => in_members = false,
                b"exterior" => in_exterior = false,
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    if quay_members > 0 {
        eprintln!("warning: {quay_members} fare zone members are quays, not stop places; those stops lose the zone");
    }
    Ok(zones)
}

/// A gml posList holds whitespace-separated `lat lon` pairs. An unparsable or odd-length list
/// is an error: dropping a token would re-pair the rest into a plausible wrong ring.
fn parse_pos_list(text: &str, zone_id: &str) -> Result<Vec<Coordinate>, Box<dyn std::error::Error>> {
    let nums = text
        .split_whitespace()
        .map(str::parse::<f64>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("fare zone {zone_id} outline: {e}"))?;
    if nums.len() % 2 != 0 {
        return Err(format!("fare zone {zone_id} outline has {} values, expected lat/lon pairs", nums.len()).into());
    }
    Ok(nums.chunks_exact(2).map(|p| Coordinate::new(p[0], p[1])).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::test_helpers::test_data_path;

    // The fixture's stops, by NSR id and centroid.
    const NYDALEN: (&str, f64, f64) = ("NSR:StopPlace:59649", 59.951841, 10.771837);
    const NYLAND: (&str, f64, f64) = ("NSR:StopPlace:305", 59.943941, 10.871645);
    const ALTA: (&str, f64, f64) = ("NSR:StopPlace:59291", 69.977739, 23.346945);

    fn zones() -> FareZones {
        FareZones::load(&test_data_path("fareZones.xml")).unwrap()
    }

    fn ids_for<'a>(zones: &'a FareZones, stop: (&str, f64, f64)) -> Vec<&'a str> {
        zones.zones_for(stop.0, &Coordinate::new(stop.1, stop.2))
            .iter().map(|z| z.id.as_str()).collect()
    }

    #[test]
    fn parses_zones_with_authority_and_outline() {
        let zones = zones();
        assert_eq!(zones.zones.len(), 5);
        assert!(zones.zones.iter().all(|z| z.authority.is_some()));
        let rut4 = zones.zones.iter().find(|z| z.id == "RUT:FareZone:4").unwrap();
        assert_eq!(rut4.authority.as_deref(), Some("RUT:Authority:RUT"));
        // Pins lat/lon order and pairing, not just "parsed something".
        assert_eq!(rut4.outline, vec![
            Coordinate::new(59.85, 10.60),
            Coordinate::new(60.00, 10.60),
            Coordinate::new(60.00, 11.00),
            Coordinate::new(59.85, 11.00),
            Coordinate::new(59.85, 10.60),
        ]);
    }

    #[test]
    fn spatial_zone_covers_only_stops_inside_its_outline() {
        let zones = zones();
        assert_eq!(ids_for(&zones, NYLAND), ["RUT:FareZone:4"]);
        assert_eq!(ids_for(&zones, ALTA), ["FIN:FareZone:26"]);
    }

    #[test]
    fn explicit_stops_zone_reaches_members_only() {
        // Both Oslo stops sit inside RUT:FareZone:13's outline; only Nydalen is a member.
        let zones = zones();
        assert_eq!(ids_for(&zones, NYDALEN), ["RUT:FareZone:13", "RUT:FareZone:4"]);
        assert_eq!(ids_for(&zones, NYLAND), ["RUT:FareZone:4"]);
    }

    #[test]
    fn explicit_stops_zone_without_members_reaches_nothing() {
        // RUT:FareZone:20 has the Oslo outline and no members. Falling back to the outline
        // would hand it every stop in the zone.
        let zones = zones();
        assert!(!ids_for(&zones, NYDALEN).contains(&"RUT:FareZone:20"));
        assert!(!ids_for(&zones, NYLAND).contains(&"RUT:FareZone:20"));
    }

    #[test]
    fn quay_scoped_members_are_not_read_as_stop_places() {
        let xml = r#"<FareZone id="RUT:FareZone:1"><members>
            <ScheduledStopPointRef ref="NSR:ScheduledStopPoint:Q305"/>
            </members><ScopingMethod>explicitStops</ScopingMethod></FareZone>"#;
        assert!(parse_fare_zones(xml).unwrap()[0].members.is_empty());
    }

    #[test]
    fn three_dimensional_pos_list_is_rejected() {
        // srsDimension="3" (lat lon alt) would otherwise re-pair into an arbitrary ring.
        let xml = r#"<FareZone id="RUT:FareZone:1"><ns2:Polygon><ns2:exterior><ns2:LinearRing>
            <ns2:posList>59.85 10.60 5.0 60.00 10.60 5.0 60.00 11.00 5.0</ns2:posList>
            </ns2:LinearRing></ns2:exterior></ns2:Polygon></FareZone>"#;
        let err = parse_fare_zones(xml).unwrap_err().to_string();
        assert!(err.contains("expected lat/lon pairs"), "got: {err}");
    }

    #[test]
    fn unparsable_pos_list_is_rejected() {
        let xml = r#"<FareZone id="RUT:FareZone:1"><ns2:Polygon><ns2:exterior><ns2:LinearRing>
            <ns2:posList>59.85 10.60 not-a-number 60.00</ns2:posList>
            </ns2:LinearRing></ns2:exterior></ns2:Polygon></FareZone>"#;
        assert!(parse_fare_zones(xml).is_err());
    }

    #[test]
    fn second_outline_is_rejected() {
        let xml = r#"<FareZone id="RUT:FareZone:1"><ns2:Polygon><ns2:exterior><ns2:LinearRing>
            <ns2:posList>59.85 10.60 60.00 10.60 60.00 11.00</ns2:posList></ns2:LinearRing></ns2:exterior></ns2:Polygon>
            <ns2:Polygon><ns2:exterior><ns2:LinearRing>
            <ns2:posList>58.00 9.00 58.50 9.00 58.50 9.50</ns2:posList></ns2:LinearRing></ns2:exterior></ns2:Polygon></FareZone>"#;
        let err = parse_fare_zones(xml).unwrap_err().to_string();
        assert!(err.contains("more than one outline"), "got: {err}");
    }
}
