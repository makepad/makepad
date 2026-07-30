# Open data sources for map overlays

Survey of open datasources we can overlay on the Europe map, researched and
endpoint-verified 2026-07-27 (curl-probed where noted). Netherlands-first, Europe-wide
where possible. Companion to `gps.md` (interaction layer) and `map.md` (renderer).

> **Implementation**: `libs/geodata` (CLI: `geodata`) is the bulk-download +
> layer-database builder born from this survey — see `libs/geodata/README.md`
> for the per-layer database schemas and which layers are built vs planned.

License verdicts used below:
- **OPEN** — CC0 / public domain / no conditions. Bundle, redistribute, ship.
- **ATTR** — CC-BY-style. Fine, needs an attribution screen.
- **NC** — non-commercial only. Fine for the hobby build, blocks a commercial app.
- **RESTRICTED** — permission required / unusable. Listed to warn.

## How overlays plug into our stack

Five integration paths, roughly in order of how much new machinery each needs:

- **A. Tile-time attribute join** (import pipeline, `tools/map_tiles`): join external
  attributes onto OSM features while building our own tiles. Zero new renderer work —
  the style layer just gets new tags to color by.
- **B. Raster overlay layer** (new renderer feature): decode raster tiles (PNG/WebP/COG)
  onto textures, alpha-blend above the base map, optional time dimension for animation
  (radar frames). One feature, many layers: radar, hillshade, aerial, satellite, model
  fields. The tile scheduling/fade machinery in `view.rs` generalizes to this.
- **C. Live point/vector overlays** (existing `overlay.rs` machinery from the nav work):
  markers, polylines, colored road segments fed by a background poller. Transit
  vehicles, planes, ships, chargers, traffic coloring.
- **D. Routing-graph enrichment** (`libs/map_nav` + import): per-edge attributes
  (elevation profile, live travel times, bridge state) that change route costs, not
  pixels.
- **E. 3D import** (box3d/xr track): real building meshes and terrain meshes.

---

## 1. Weather

### OPERA / EUMETNET European rain radar — the headline find
The pan-European radar composite became genuinely open via the EU high-value-datasets
push. The **EUMETNET Open Radar Data (ORD) API** on MeteoGate serves OPERA composites:
max reflectivity at **1 km / 5 min** and surface rain rate + 1-h accumulation at
2 km / 15 min, all of Europe, as **ODIM HDF5 and cloud-optimized GeoTIFF**, rolling
24 h + archive to 2012. Anonymous tier verified live
(`https://api.meteogate.eu/eu-eumetnet-weather-radar/collections` returns 200); free
API key raises limits; MQTT push available. **ATTR (CC BY 4.0).**
Integration: B — COG → reproject to mercator → colormap → animated raster frames.

### KNMI Data Platform (NL detail + nowcast)
5-min NL reflectivity composite (`radar_reflectivity_composites/2.0`) plus
`radar_forecast/1.0` — a **+2 h nowcast in 5-min steps**, i.e. the "rain in the next
two hours" animation comes precomputed. HARMONIE-AROME forecast grids (GRIB1) too.
REST file API + MQTT push; free key (shared anonymous key exists). **ATTR (CC BY 4.0).**
Format: KNMI HDF5, polar-stereographic — needs decode + reprojection (or accept the
2 km OPERA rain-rate product for NL too and skip HDF5 entirely at first).

### Wind fields — the animated particle/arrow layer (verified 2026-07-28)
The windy.com / earth.nullschool effect needs a gridded **10 m u/v field** (not
speed/direction points): u/v becomes a small RG texture per forecast step, a shader
advects fading particles through it, steps crossfade for the time animation — the
same technique as Beccario's earth and Mapbox's WebGL wind demo; their
grib2json/leaflet-velocity JSON is literally this grid, we just decode GRIB2/NetCDF
ourselves. All sources below carry 10 m u/v (+ gusts); curl-probed 2026-07-28:

- **NOAA GFS via NOMADS** — global 0.25°, 4 runs/day, hourly steps to 120 h. The
  filter CGI subsets **by variable and bbox server-side**: 10 m U+V over a NL box
  returned a **939-byte** GRIB2
  (`filter_gfs_0p25.pl?...&var_UGRD=on&var_VGRD=on&lev_10_m_above_ground=on&subregion=&...`).
  **OPEN (US public domain — the only no-attribution source).** The bootstrap
  source for the shader work: bytes per frame, no key, and it's what
  windy/nullschool themselves run on.
- **ECMWF open data (IFS)** — global 0.25°, GRIB2, 4 runs/day, 3-h steps to 144 h
  (6-h to 240 h), no registration. Each file has an `.index` sidecar with byte
  offsets per param → one HTTP range request pulls a global 10u/10v field (~870 KB).
  Better European skill than GFS. `data.ecmwf.int` verified fine; the AWS mirror
  (`ecmwf-forecasts`, eu-central-1) throttles hard (SlowDown after a handful of
  rapid hits). **ATTR (CC BY 4.0).**
- **KNMI UWC-West HARMONIE — `uwcw-ha-det-nl-s1`** — NL at **2 km**, hourly runs,
  60 h horizon, one NetCDF per parameter: `wind-speed-components-hagl` (u/v at 10 m
  + boundary-layer levels, ~57 MB/run) or speed/direction/gust-only files
  (~27 MB each). Verified landing hourly. Free registered key; the shared anonymous
  key is 50 req/min shared and rate-limited repeatedly while probing — register.
  **ATTR (CC BY 4.0).** The NL-detail layer once the particle pass exists. Its
  predecessors (`harmonie_arome_cy43_p1` = 850 MB GRIB1 tars/run,
  `uwcw_extra_lv_ha43_nl_2km`) are deprecated; the Europe-wide 0.05° DINI domain
  sits in `harmonie_arome_cy43_p3` (GRIB1 tars).
- **DWD ICON-D2 / ICON-EU** — D2: 2.2 km covering Germany + all of Benelux, hourly
  `u_10m`/`v_10m`, **regular-lat-lon variants available** (~1 MB bz2 per step),
  plain HTTPS directory, no key
  (`opendata.dwd.de/weather/nwp/icon-d2/grib/<run>/u_10m/`). ICON-EU at 6.5 km
  likewise. **ATTR (GeoNutzV).** The middle tier between ECMWF and KNMI 2 km.
- **Live station wind (observed arrows)**: KNMI
  **`10-minute-in-situ-meteorological-observations`** — ~156 KB NetCDF every
  10 min, all NL automatic stations incl. wind poles, verified fresh at probe time.
  **ATTR.** Path C arrow markers + a "now" calibration for the model field.
  (Replaces `Actuele10mindataKNMIstations`, which stopped updating 2025-09-29.)
- Shortcut worth knowing: **Open-Meteo's AWS open-data bucket** republishes
  ECMWF/ICON/HARMONIE grids pre-merged in their custom `.om` chunk format —
  convenient, but the raw GRIB2/NetCDF above is plain enough to skip the extra
  format dependency.

Integration: path B's raster/time machinery + one new GPU particle pass (u/v → RG
texture per step, advect + fade, interpolate between steps); arrows are the cheap
first render of the same field. The identical u/v grid later feeds the EV headwind
term (EV thread, point 4).

### Forecast field overlays (cloud, snow, precip)
- **ECMWF open data** — IFS/AIFS at 0.25°, GRIB2, 4 runs/day, no registration,
  mirrors on AWS S3. **ATTR (CC BY 4.0).** Cloud cover, snow depth, and the other
  synoptic fields (wind: see above).
- **DWD ICON-EU** — 6.5 km Europe grid, GRIB2, plain HTTPS directory, no signup
  (`opendata.dwd.de`). **ATTR (GeoNutzV).** Sharper than ECMWF over central Europe;
  ICON-D2 (2.2 km) covers eastern NL fringe.
- **Open-Meteo** — JSON point/route forecasts; accepts coordinate lists →
  **forecast along a route in one call** (EV: wind + temperature per leg). Data is
  CC-BY; hosted API free for **non-commercial** (~10k calls/day); AGPL self-host
  option. **ATTR data / NC hosted API.**
- **MET Norway api.met.no** — CC-BY point forecasts, no key (mandatory User-Agent);
  solid backup.

### Satellite cloud layer
**EUMETSAT EUMETView** — WMS/WMTS at `view.eumetsat.int/geoserver`, no registration:
Meteosat MTG imagery, full disc every 10 min, rapid-scan Europe 5 min. **ATTR.**
Integration: B, trivially (it's already a tile service; cache politely).

### Flagged unsuitable
- **RainViewer** — free tier now personal/educational only, nowcast+satellite removed
  from the public endpoint, max zoom 7. Prototyping only. **NC.**
- **Buienradar** — imagery not georeferenced; apps require written permission.
  **RESTRICTED** (and redundant: KNMI is its upstream).
- **Blitzortung lightning** — network participants only, no commercial use.
  **RESTRICTED.** No open EU lightning alternative exists.

---

## 2. Buildings, elevation, imagery

### Building age (the NL building-age map) — nearly free for us
Dutch OSM buildings originate from the community BAG import and already carry
**`ref:bag`** (BAG pand id) and **`start_date`** (= bouwjaar) — the construction year
is **already in the Europe PBF on disk**. v1 building-age coloring is therefore pure
path A: keep `start_date` on buildings at tile build, add an age→color ramp to the
style. For authoritative/fresh data later: **BAG via PDOK** — `bag-light.gpkg`
(7.8 GB GeoPackage, monthly, **OPEN — CC0**), exact join on `ref:bag` =
`identificatie`. Prior art: Waag's "All 9.8M buildings" map (2013).

### Building heights + real 3D (NL)
**3D BAG** (TU Delft, release 2025.09, ~10.8M buildings): per-building height stats
(ground/roof percentiles, ridge) derived from LiDAR — join by the same BAG id at tile
build (path A) and extrude in the renderer; plus LoD2.2 roof-shape meshes as
**3D Tiles 1.1 / CityJSON / OBJ** per-tile downloads for the real-3D showcase
(path E). **ATTR (CC BY 4.0).** OSM's own `building:levels` covers only ~6% globally
and worse in NL — 3D BAG is the answer here.
Europe-wide fallback: **EUBUCCO v0.2** (322M buildings, height ~43%, age ~16%,
**ODbL share-alike**) or JRC **DBSM R2025** (harmonized EU stock, ODbL).

### Elevation (the EV-routing input)
- **AHN** (NL LiDAR): 0.5 m DTM/DSM GeoTIFF/COG via PDOK atom/WCS, AHN4 complete,
  AHN5 rolling in. **OPEN.** Best-in-class; also validates 3D BAG heights.
- **Copernicus DEM GLO-30** (30 m, Europe/global): anonymous COGs on AWS
  (`s3://copernicus-dem-30m/`). **ATTR** (credit notice). The pan-European base under
  AHN. (EU-DEM is discontinued; the 10 m EEA product is access-restricted — skip.)
- **Ready-made terrain tiles**: **Mapterhorn** — terrarium-encoded terrain tiles for
  Europe built from Copernicus + national LiDAR, distributed as a single **PMTiles**
  file (`download.mapterhorn.com/planet.pmtiles`), open data, offline-friendly —
  the fastest route to hillshade + 3D terrain. AWS/Mapzen terrain tiles still exist
  (**ATTR**) but carry stale EU-DEM-era data for Europe.

Uses: hillshade raster overlay (B), 3D terrain meshes (E), and **per-edge climb/descent
baked into `region.graph`** (D) — see the EV section.

### Road elevation / grade separation (overpasses in the 3D car-follow view)
Researched 2026-07-29: **OSM will not store road z, and there is no plan to.** The
data model is 2D lat/lon; `ele=*` is for summits/POIs ("OSM is not an elevation
database") and no active proposal changes that. Overture's transportation schema
keeps the same design — `level` is relative stacking only, explicitly "not a
precise indication of elevation". What road geometry does carry (all already in
our PBF): `layer=*` (relative −5..5), `bridge`/`tunnel`, `incline=*`, `maxheight`
(clearance *under*), occasionally `height` on bridge ways.

The ecosystem plan-of-record is **derive z at build time: DEM + OSM semantics +
constraint solving**. OSM2World is the reference implementation (init every road
vertex at terrain height → equal-z at shared nodes → minimum vertical clearance at
bridge-over-road crossings → tunnels below terrain → grade smoothing + per-class
incline clamps), and its Prototype-Fund **"OSM-3D-Terrain"** project (roadmap
June 2026) is productizing exactly this into 3D tiles with LOD and tile-boundary
continuity — worth watching. No open vector-map renderer draws true
grade-separated interchanges yet (MapLibre's terrain drape squashes bridges flat),
so doing it properly is a differentiator.

Our recipe (tile/nav build):
1. Init road vertices at DEM height (Mapterhorn/AHN, above).
2. Constraint pass; **never sample the DEM mid-bridge/-tunnel** — interpolate
   between abutments. (The classic artifact — elevation profile dipping into the
   valley under the viaduct — would also poison the EV climb numbers in the EV
   thread below.)
3. Bake per-vertex Δz-above-DEM into our tiles; the renderer lifts decks and sinks
   tunnels from the same terrain drape.
4. NL upgrade — measured deck heights instead of solved ones:
   - **RWS DTB** (PDOK, open): cm-grade surveyed x/y/z lines/points/areas covering
     exactly the RWS motorway network incl. interchanges; z now in the WFS too.
   - **AHN**: DSM keeps bridge decks, DTM removes them → DSM−DTM over bridge
     polygons ≈ deck height; the LAZ point clouds classify structures directly.
   - **Kadaster 3D Basisvoorziening** (open, 3dfier-generated): ready-made 3D
     terrain + LoD1 bridge decks, if ingesting meshes beats solving.

### Aerial & satellite imagery
- **PDOK Luchtfoto** (NL): 25 cm nationwide 2×/year + 8 cm HR flights, WMTS +
  **bulk GeoTIFF download** via the Beeldmateriaal dataroom. **ATTR (CC BY 4.0)** —
  offline self-tiling is explicitly legal. NL "satellite view" solved.
- **Sentinel-2**: EOX cloudless mosaics are **NC** for 2018–2024 (only the 2016 layer
  is CC-BY) — trap. The open path is compositing our own cloudless mosaic from raw
  Sentinel-2 L2A COGs on the Copernicus Data Space (**OPEN/ATTR**, one-off batch job).
- Germany: NRW 10 cm orthophotos are license-zero (no attribution!); Belgium via
  geo.be WMTS (ATTR). Per-country patchwork.
- **GHSL** (JRC): built-up surface 10 m, building height 100 m, population rasters.
  **ATTR.** Coarse Europe-wide fallbacks and a population-density shading layer.

---

## 3. Mobility & live transport

### OVapi / NDOV — all Dutch public transport, live, CC0
The backbone NL feed, verified live (sub-minute freshness): static GTFS (~223 MB,
daily) + GTFS-RT protobufs — `vehiclePositions.pb` (~218 KB!), `tripUpdates.pb`,
`trainUpdates.pb`, `alerts.pb` at `gtfs.ovapi.nl` / `gtfs.openov.nl`. **No key, no
registration, OPEN (CC0).** Every bus/tram/metro/train in the country as a live layer
for one small protobuf poll every ~30 s (path C: colored vehicle markers; tap for
line/delay). Volunteer-run — identify with a User-Agent, poll politely; NDOV loket
(free registration, ZeroMQ push) is the harder-core fallback. The NS API is redundant
for positions (proprietary terms; OVapi carries all rail already).

### NDW — national road traffic, minute cadence, no key
`https://opendata.ndw.nu/` — plain HTTPS directory of gzipped DATEX II XML refreshed
every minute, **OPEN**, verified live: point speeds/intensities from 24k+ sites
(`trafficspeed.xml.gz`), segment travel times, situation feed (jams, closures,
wrong-way drivers, **live bridge openings** — six bridges stood open at check time),
matrix-sign states, roadworks/events planning, temporary speed limits, emission
zones, truck parking, and a **national EV-charger dataset in OCPI JSON**. Effort sits
in DATEX II parsing + matching their location references onto our road segments; the
payoff is double: traffic coloring overlay (C) *and* live routing weights + bridge
penalties (D). Historical archive via Dexter.

### Europe-wide transit
No single EU GTFS-RT exists. Practical ladder: NL = OVapi; DE = **gtfs.de** mirrors
(CC-BY, no login, incl. a free Germany-wide GTFS-RT feed, verified); rest via the
**Transitous** project (1,800+ cleaned feeds, re-published openly; hosted MOTIS API is
non-commercial fair-use — self-host MOTIS with their feed list if it pinches) and the
`european-transport-feeds` / Mobility Database catalogs.

### Live vehicles for fun (path C, cheap to add)
- **OpenSky** — live aircraft state vectors, bbox REST query, OAuth2; registered free
  tier ≈ one NL-bbox poll/22 s. **NC free tier.** Feeding an ADS-B receiver doubles
  the quota.
- **aisstream.io** — free WebSocket AIS ship positions (good near NL coast/ports);
  thin license text — fine for hobby, clarify before commercial. Official NL inland
  AIS is deliberately closed — don't plan on it.
- **OV-fiets GBFS** — `gbfs.openov.nl/ovfiets/gbfs.json`, live rental-bike
  availability per station, 60 s TTL, **OPEN**. Perfect companion to transit routing.
- **Autobahn API** (DE) — no-key REST JSON: roadworks, closures, webcams, chargers
  per motorway. **OPEN.** Easiest German traffic entry point.

### EV charging
**NDW's OCPI feed** for NL (open, national, refreshed ~daily) + **OpenChargeMap** for
the rest of Europe (CC-BY, free key, rate-limited). Neither gives live occupancy for
free. OSM's `amenity=charging_station` lags reality in NL — use it only as fallback.

### Cycling
NL junction networks (knooppunten) are fully and actively mapped **in OSM** — already
in our data; render as an overlay style. The official Fietsplatform database is
**RESTRICTED** (purpose-limited license) — avoid.

### Water
**Rijkswaterstaat WaterWebservices** (new endpoint
`ddapi20-waterwebservices.rijkswaterstaat.nl` — old one is being switched off in
2026): water levels every ~10 min, JSON, no registration, **OPEN**. Plus
vaarweginformatie.nl fairway notices for lock/bridge operating times.

---

## 4. Environment & statistics

### Air quality
- **Luchtmeetnet** (RIVM, official NL): ~100 stations, hourly NO2/PM/O3 + LKI index +
  interpolated concentrations at arbitrary lat/lon. JSON API
  (`api.luchtmeetnet.nl/open_api`), no auth, 100 req/5 min; bulk CSV at
  `data.rivm.nl/data/luchtmeetnet/`. **ATTR (CC BY 4.0).** Primary NL live layer.
- **Sensor.Community**: crowdsourced PM, densest network in DE/NL; bbox-filterable
  JSON (`data.sensor.community`), daily CSV archive. **OPEN (ODbL-family, attr).**
  Noisy per-sensor — hex-bin into a heatmap. RIVM **Samen Meten** (SensorThings API)
  offers RIVM-calibrated versions of the same citizen sensors.
- **EEA** (Europe): the old file dumps are discontinued; current Download Service
  serves Parquet timeseries + modelled rasters (**ATTR**). Europe-wide
  historical/modelled; use Luchtmeetnet for NL live.

### Noise
**RIVM "Geluid in Nederland"** is the winner: whole-NL 10 m Lden/Lnight rasters per
source (highways, municipal roads, rail, aviation, industry, wind turbines) as
**CC0 GeoTIFF downloads** (`data.rivm.nl/data/alo/...`) + WMS. Bake our own raster
tiles (path B). Official END round-4 contour *polygons* (also CC0) on PDOK/haleconnect
if we want crisp vectors; EEA publishes the Europe-wide equivalents (**ATTR**, country
completeness varies).

### Land cover
**CORINE CLC2018** (100 m, Europe) — still the latest until CLC2024 lands mid-2026;
GeoTIFF via land.copernicus.eu (EU-Login) or a no-login EEA WMS mirror. **ATTR.**
**CLC+ Backbone 10 m** (2023 edition) is the modern high-res version — gorgeous
green/urban/water backdrop at z13+. Both EPSG:3035 → one ingest-time reprojection.

### Protected nature
**PDOK Natura 2000**: direct 10.7 MB GeoPackage, **CC0**, verified — ship it offline
as vector-tile polygons (path A). Also CC0 on PDOK: Wetlands/Ramsar (3.8 MB),
Nationale Parken (3.25 MB), Natuurnetwerk Nederland. Europe: EEA Natura 2000 bundle
(2.3 GB SHP/GPKG, **ATTR**). **WDPA/Protected Planet is RESTRICTED** (redistribution
ban incl. offline maps) — not needed, the open sets cover Europe.

### Flood risk (very Dutch, very good)
- **LIWO** (Rijkswaterstaat): public GeoServer, no login — max water depth per
  probability class, with **WCS** access to pull actual depth grids for offline
  baking. De-facto open, attribute RWS.
- **Klimaateffectatlas WMS**: the same national data with a clean **CC BY 4.0**
  citation surface; three probability classes as UI granularity.
- **PDOK ROR** flood risk/hazard zone polygons: **CC0**, tiny — the cheap
  "am I in a flood zone?" layer.
- **JRC Europe river flood hazard** v3.1.1: depth GeoTIFFs per return period,
  ~90 m, WGS84, anonymous FTP-style download (RP100 = 323 MB), **ATTR**. The
  Europe-wide layer.

### Demographics
- **CBS Wijk- en Buurtkaart** (2025 on PDOK): neighborhood polygons with population,
  age, income, density, housing — the canonical NL choropleth. GPKG ~261 MB.
  **ATTR (cite CBS + Kadaster).**
- **CBS 100 m / 500 m grids** (Vierkantstatistieken, direct zips from
  `download.cbs.nl/vierkant/`): per-cell population, age/sex, housing, WOZ value,
  energy consumption — **the best 3D-extrusion layer we could ship** (100 m at high
  zoom, 500 m as LOD). **ATTR.** Cells under 5 residents are suppressed.
- **Eurostat GISCO Census 2021 grid V3** (May 2026): 1 km grid, 30 countries,
  13 variables, GeoParquet/GPKG/GeoTIFF. **ATTR.** Pairs with the CBS grid inside NL.
- **CBS StatLine OData**: join any statistic onto wijk/buurt codes at ingest, no key.
- **GHSL** population/built-up rasters (see §2) as the global fallback.

### Historic maps
No official open Topotijdreis WMTS exists. The clean route: **RCE Bonnebladen ~1900**
via RCE's geo services (**ATTR**, pre-1930 imagery PD by age) — a "year slider"
overlay anchored on ~1900 vs today. The per-year Esri NL "tijdreis" tile services
work but are legally gray (unofficial). Pan-European: Arcanum is commercial, David
Rumsey is **NC** — skip both.

### Wikipedia / Wikidata ("what's around me")
Wikipedia geosearch API (no key, radius ≤10 km, descriptive User-Agent), Wikidata
SPARQL for bulk coordinates (**coordinates CC0**), Commons geosearch for geotagged
photos. Best value-per-effort in this whole document: article-card popups on map
points (path C), cacheable offline. NL Wikipedia + Rijksmonumenten together make a
genuinely good heritage layer.

### Fun extras (all verified, all NL unless noted)
- **Rijksmonumenten** (~63k national monuments): RCE WFS/WMS, nightly refresh,
  **ATTR**.
- **Kadastrale kaart (BRK)**: PDOK serves **native MVT vector tiles** — drop-in
  cadastral parcel lines at high zoom, **ATTR**. (The BAG OGC API also serves native
  MVT with bouwjaar — an alternative to our own join for quick experiments.)
- **Groningen earthquakes**: KNMI FDSN event API, **ATTR** — bubble map.
- **Trees**: Amsterdam serves ~300k trees with species/year as **native MVT**
  (`api.data.amsterdam.nl/v1/mvt/bomen`); Rotterdam etc. on data.overheid.nl.
- **Solar potential per roof**: Zonatlas-derived national dataset, Public Domain
  Mark, but dated — verify before building on it.
- **Energy labels** (EP-Online/RVO): monthly dump, free key, joins on BAG ids —
  **no open license: usable, don't redistribute.** A–G choropleth per building.
- **AEDs, windmills, etc.**: no open national registers — OSM/Overpass has them.

---

## The EV thread (elevation + chargers + weather, combined)

The pieces above compose into proper EV routing (extends `gps.md` M3):

1. **Energy-aware graph**: at `nav-build` time, sample AHN (NL) / GLO-30 (Europe)
   along every edge → per-edge climb/descent meters packed next to length/speed in
   `region.graph`. Cost function becomes physics: rolling + aero (speed²) + mass ×
   climb − regen × descent. Flat NL barely notices; the first Ardennen/Alpen trip does.
2. **Range visualization**: energy-cost Dijkstra from current position + state of
   charge → **reachable-area polygon** shaded on the map ("can I make it without
   charging?"), recomputed as you drive.
3. **Charger-aware planning**: NDW OCPI + OCM chargers as graph POIs; multi-stop
   route planning picks charging stops by detour cost + connector/power filtering.
4. **Weather correction**: Open-Meteo along-route wind + temperature → headwind term
   in the aero cost and a cold-battery efficiency factor. This is what makes highway
   range predictions honest.

## Licensing traps (one-screen summary)

| Source | Trap |
|---|---|
| RainViewer | personal/educational only now; free tier gutted |
| Buienradar | apps need written permission — use KNMI (its upstream) |
| Blitzortung | participants only, no commercial |
| EOX Sentinel-2 cloudless | 2018+ mosaics are CC-BY-**NC**; only 2016 is CC-BY |
| Copernicus EEA-10 DEM | access-restricted — use GLO-30 |
| EUBUCCO / DBSM | ODbL share-alike — mind derived-database obligations |
| Fietsplatform routes | purpose-limited license — OSM has the networks anyway |
| NS API | proprietary terms and redundant — OVapi is CC0 |
| Transitous hosted API | non-commercial fair-use — self-host MOTIS instead |
| OpenSky / aisstream / Open-Meteo hosted | free tiers are non-commercial |
| WDPA / Protected Planet | redistribution ban (incl. offline maps) — use Natura 2000/CDDA |
| David Rumsey / Arcanum historic maps | NC / commercial — use RCE Bonnebladen |
| EP-Online energy labels | no open license — use, don't redistribute |
| Esri NL "tijdreis" historic tiles | no stated terms — legally gray, unofficial |
| Everything NL-official (BAG, AHN, luchtfoto, NDW, OVapi, RWS, RIVM noise, PDOK nature/flood) | no traps — CC0/CC-BY, bundle away |

## Suggested build order

1. **Building age (path A)** — `start_date` is already in our PBF; a tag to keep +
   a color ramp. One day of work for the most beautiful quick win; the Waag map from
   OSM data we already have.
2. **Raster overlay layer in the renderer (path B)** — the enabling feature. First
   consumer: hillshade from Mapterhorn PMTiles (static, offline, no cadence
   pressure). Second: rain radar animation (OPERA COG → colormapped frames + the
   KNMI 2 h nowcast for NL) — the "amazing" one. Third: the wind particle layer
   (§1 wind fields) — reuses the same time-dimension machinery plus one GPU
   particle pass; bootstrap on GFS bbox subsets (bytes per frame, no key).
3. **Live transit + traffic (path C)** — OVapi vehicle positions as moving markers;
   NDW travel-time coloring on trunk roads. Both are single-file pollers feeding the
   existing overlay machinery.
4. **Elevation into the graph (path D)** — EV/bike energy-aware routing + range
   polygon; quiet infrastructure, big payoff once routes leave the polder.
5. **3D BAG + AHN terrain (path E)** — the showcase: LoD2.2 roofs + real terrain in
   the xr/3D track.

Once paths A–C exist, §4 provides cheap follow-on layers that reuse them: Natura 2000
polygons (10.7 MB CC0 GPKG → path A), RIVM noise + JRC flood rasters (CC0/CC-BY
GeoTIFFs → path B), Wikipedia/Rijksmonumenten points (path C), and the CBS 100 m grid
as the first 3D-extrusion statistics layer (path E).
