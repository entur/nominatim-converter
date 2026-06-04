# Entrance-tie mapping worklist

Generated 2026-06-04 from `norway-latest.osm.pbf` (Geofabrik extract dated 2026-05-29) with the
production import config (`geocoder/photon/import/config/nominatim-converter.json`).

## What this is

For large `useEntrance` features, the converter substitutes the best perimeter gate/entrance for
the polygon centroid. When two or more candidates tie on the full ranking key, the pick among them
is arbitrary (smallest node id). This run found **33 of 79 substitutions (42%) were such arbitrary
picks**, listed below sorted by tie spread (max distance between the tied gates).

The fix is mapping, not code: identify the real main gate (vakta/hovedporten) on aerial imagery or
Mapillary and tag it in OSM with `routing:entrance=main` (or `entrance=main`, or give it its
`name`). The converter ranks an explicit main marker above everything, and a name above same-kind
candidates, so either resolves the tie. This improves OSM for every consumer, not just us.

## How to regenerate

```
cargo run --release -- osm -i norway-latest.osm.pbf -o /tmp/out.ndjson \
  -c <geocoder>/photon/import/config/nominatim-converter.json -f
```

The entrance-enrichment block in the log reports the tie count, spread distribution and the widest
ties with feature ids. The count dropping over time = mapping fixes landing.

## Worklist (33 ties, spread descending)

Priority: the > 250 m band (top 17) has real consequences for an arriving traveller. The
100-250 m band is mildly annoying. Below 100 m the pick is mostly immaterial -- listed for
completeness.

| Spread | Feature | Category | OSM |
|---:|---|---|---|
| 1749 m | Ørland Militærbase (Ørland) | military | <https://osm.org/way/600962632> |
| 1476 m | Bardufoss flystasjon (Målselv) | military | <https://osm.org/relation/8312685> |
| 886 m | Madlaleiren (Stavanger) | military | <https://osm.org/way/32207534> |
| 790 m | Forsvarets stasjon Ringerike (Ringerike) | military | <https://osm.org/way/602862481> |
| 635 m | Ullevål sykehus (Oslo) | hospital | <https://osm.org/way/4617195> |
| 589 m | Lutvann leir (Oslo) | military | <https://osm.org/way/97706911> |
| 514 m | Løten depot (Løten) | military | <https://osm.org/way/1037045864> |
| 467 m | Drevjamoen skyte- og øvingsfelt (Vefsn) | military | <https://osm.org/relation/10172015> |
| 467 m | Drevjamoen (Vefsn) | military | <https://osm.org/way/250805510> |
| 398 m | Linderud leir (Oslo) | military | <https://osm.org/way/115513581> |
| 389 m | Asker batteri (Asker) | military | <https://osm.org/way/1481149497> |
| 320 m | Camp Viking (Målselv) | military | <https://osm.org/way/1442618332> |
| 313 m | Bogen NATO-kai (Evenes) | military | <https://osm.org/way/306400541> |
| 312 m | Sola flystasjon (Sola) | military | <https://osm.org/relation/6983810> |
| 303 m | Terningmoen Leir (Elverum) | military | <https://osm.org/way/518127311> |
| 295 m | Hauerseter leir (Ullensaker) | military | <https://osm.org/way/104543411> |
| 254 m | Ytre festningsområde (Oslo) | military | <https://osm.org/relation/15093126> |
| 209 m | Bodin leir (Bodø) | military | <https://osm.org/way/24843736> |
| 182 m | Universitetshagen (Oslo) | park | <https://osm.org/way/111845758> |
| 176 m | Valhall stadion (Tromsø) | stadium | <https://osm.org/relation/17371572> |
| 152 m | Gulskogen gård (Drammen) | park/attraction | <https://osm.org/way/48307006> |
| 149 m | Sessvollmoen skyte- og øvingsfelt (Ullensaker) | military | <https://osm.org/relation/10172009> |
| 148 m | Strømmen stadion (Lillestrøm) | stadium | <https://osm.org/way/682923182> |
| 130 m | Marienlyst stadion (Drammen) | stadium | <https://osm.org/relation/13850962> |
| 127 m | Ledaalsparken (Stavanger) | park | <https://osm.org/relation/13877931> |
| 127 m | Atlanten stadion (Kristiansund) | stadium | <https://osm.org/way/177430960> |
| 118 m | Ulven T Park (Oslo) | park | <https://osm.org/way/1353234249> |
| 72 m | Marinen (Trondheim) | park | <https://osm.org/relation/6921140> |
| 68 m | Halden Stadion (Halden) | stadium | <https://osm.org/way/4481589> |
| 54 m | Havredalen (Oslo) | park | <https://osm.org/relation/20326074> |
| 33 m | Frognerparken (Oslo) | park | <https://osm.org/way/4334023> |
| 17 m | Somaleiren, teknisk verksted (Sola) | military | <https://osm.org/relation/7193796> |
| 4 m | Hunderfossen Familiepark (Lillehammer) | theme_park | <https://osm.org/relation/10678827> |

Notes:
- 19 of 33 are military: fenced perimeters with several identical gate nodes. These genuinely have
  one main gate, so they are the most mappable.
- Drevjamoen appears twice (co-named way + relation); one mapping fix resolves both rows.
- Run headline numbers for trend tracking: 760 eligible features (>= 150 m), 79 substitutions
  (10.4% have any mapped gate at all -- the other 90% keep their centroid; mapping gates on more
  of those is a separate, larger improvement opportunity), 33 ties, spread median=254 p90=790
  max=1749 m. The September 2025 extract gave 752 / 72 / 29 ties -- new bases (Ørland, Camp
  Viking, Asker batteri) entered the list as their fences/gates got mapped.
- If the tie count stays high as gate coverage grows, the planned alternative is a stop-proximity
  tie-breaker (prefer the gate nearest an NSR stop place); see session notes in the geocoder repo
  (`proxy/docs/entrances-wip.md`) for the broader entrance roadmap.
