//! The real world, live: the weather the nearest airport is reporting now
//! (METAR, from aviationweather.gov) and the aircraft really flying around
//! here now (ADS-B, from adsb.fi), fetched on a thread of their own. The
//! aircraft are drawn where they report being, carried on between reports
//! along their track.
use crate::airfield::{Light, L_GREEN, L_RED, L_STROBE, L_WHITE};
use crate::airmodels::{self as am, TYPES};
use crate::geo;
use crate::render::Inst;
use crate::weather::Weather;
use glam::{DVec3, Vec3};
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use crate::plat::Instant;
use std::time::Duration;

/// What a station reports.
#[derive(Clone, Debug)]
pub struct Metar {
    pub station: String,
    pub clouds: f32,
    /// lowest cloud base, m above the station
    pub base: Option<f32>,
    /// m/s, and where from (degrees; None when variable)
    pub wind: f64,
    pub wind_from: Option<f64>,
    pub gusts: f64,
    /// km
    pub vis: f32,
    pub storm: bool,
}

/// One aircraft's report.
struct Report {
    hex: String,
    lat: f64,
    lon: f64,
    /// m above sea level, or on the ground
    alt: Option<f64>,
    gs: f64,
    track: f64,
    vs: f64,
    ty: u8,
    /// seconds since it was there
    age: f64,
}

struct Shared {
    /// where the player is (lat, lon, degrees), and whether each feed is wanted
    at: (f64, f64),
    want: (bool, bool),
    metar: Option<Metar>,
    reports: Option<Vec<Report>>,
    /// a new cloud map of the whole Earth, until the renderer takes it
    clouds: Option<Vec<u8>>,
}

struct Plane {
    /// where it reported being, carried on since
    lat: f64,
    lon: f64,
    alt: Option<f64>,
    gs: f64,
    track: f64,
    vs: f64,
    ty: u8,
    /// where it is drawn (catching up with the above), once it has been
    shown: Option<DVec3>,
    /// the ground under it, looked up now and then
    ground: f64,
    ground_in: f32,
    quiet: f64,
    paint: (u8, u8),
}

pub struct Live {
    shared: Arc<Mutex<Shared>>,
    planes: HashMap<String, Plane>,
    /// both feeds, as the player wants them
    pub weather: bool,
    pub traffic: bool,
    /// the station whose weather is in use
    pub station: String,
    #[allow(dead_code)]
    polls: Polls,
}

/// The browser's downloads of the feeds: when each last went out (seconds),
/// and the one on its way.
#[derive(Default)]
#[allow(dead_code)]
struct Polls {
    clouds_next: f64,
    wx_at: Option<(f64, (f64, f64))>,
    wx_url: Option<String>,
    ac_at: Option<f64>,
    ac_url: Option<String>,
    ac_alt: bool,
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim_end_matches('+').parse().ok()))
}

fn metar_url(lat: f64, lon: f64) -> String {
    format!("https://aviationweather.gov/api/data/metar?bbox={:.2},{:.2},{:.2},{:.2}&format=json", lat - 0.7, lon - 0.9, lat + 0.7, lon + 0.9)
}

fn metar(body: &[u8], lat: f64, lon: f64) -> Option<Metar> {
    let v: Value = serde_json::from_slice(&body).ok()?;
    let near = |m: &&Value| {
        let (a, o) = (num(&m["lat"]).unwrap_or(99.0) - lat, (num(&m["lon"]).unwrap_or(999.0) - lon) * lat.to_radians().cos());
        a * a + o * o
    };
    let m = v.as_array()?.iter().min_by(|a, b| near(a).total_cmp(&near(b)))?;
    let mut clouds = 0.0f32;
    let mut base: Option<f32> = None;
    for c in m["clouds"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        let cover = match c["cover"].as_str().unwrap_or("") {
            "FEW" => 0.2,
            "SCT" => 0.4,
            "BKN" => 0.7,
            "OVC" | "OVX" => 0.95,
            _ => 0.0,
        };
        if cover > 0.0 {
            clouds = clouds.max(cover);
            if let Some(b) = num(&c["base"]) {
                base = Some(base.map_or(b as f32 * 0.3048, |x| x.min(b as f32 * 0.3048)));
            }
        }
    }
    let wind = num(&m["wspd"]).unwrap_or(0.0) / 1.94384;
    let wx = m["wxString"].as_str().unwrap_or("");
    Some(Metar {
        station: m["icaoId"].as_str().unwrap_or("?").to_string(),
        clouds,
        base,
        wind,
        wind_from: m["wdir"].as_f64(),
        gusts: num(&m["wgst"]).map_or(0.0, |g| ((g / 1.94384 - wind) / 10.0).clamp(0.0, 1.0)),
        vis: num(&m["visib"]).map_or(16.0, |sm| sm as f32 * 1.609),
        storm: wx.contains("TS"),
    })
}

/// Which of the traffic models an aircraft is drawn as, from its type and wake category.
fn model(t: &str, cat: &str) -> u8 {
    let has = |p: &[&str]| p.iter().any(|p| t.starts_with(p));
    if has(&["B74", "B77", "B78", "B76", "A33", "A34", "A35", "A38", "MD1", "DC10", "IL9", "A30", "A31"]) || cat == "A5" {
        4
    } else if has(&["CRJ", "E1", "E7", "E4", "E29", "AT", "DH8", "SF3", "BCS", "A22"]) {
        2
    } else if cat == "A1" || cat == "A7" || cat.starts_with('B') {
        0
    } else if cat == "A2" {
        1
    } else {
        3
    }
}

/// adsb.fi first, adsb.lol should it not answer: the same reports from both
fn report_urls(lat: f64, lon: f64) -> [String; 2] {
    [format!("https://opendata.adsb.fi/api/v2/lat/{lat:.3}/lon/{lon:.3}/dist/45"), format!("https://api.adsb.lol/v2/point/{lat:.3}/{lon:.3}/45")]
}

const CLOUD_MAP: &str = "https://clouds.matteason.co.uk/images/2048x1024/clouds.jpg";

fn cloud_map(b: &[u8]) -> Option<Vec<u8>> {
    image::load_from_memory(b).ok().map(|i| i.to_luma8()).filter(|i| i.width() == crate::render::WX_W && i.height() == crate::render::WX_H).map(|m| m.into_raw())
}

fn reports(body: &[u8]) -> Option<Vec<Report>> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let mut out = Vec::new();
    for a in v["aircraft"].as_array().or(v["ac"].as_array())? {
        let (Some(lat), Some(lon), Some(hex)) = (a["lat"].as_f64(), a["lon"].as_f64(), a["hex"].as_str()) else { continue };
        let ground = a["alt_baro"].as_str() == Some("ground");
        let alt = if ground { None } else { a["alt_baro"].as_f64().or(a["alt_geom"].as_f64()).map(|f| f * 0.3048) };
        if !ground && alt.is_none() {
            continue;
        }
        out.push(Report {
            hex: hex.to_string(),
            lat: lat.to_radians(),
            lon: lon.to_radians(),
            alt,
            gs: a["gs"].as_f64().unwrap_or(0.0) / 1.94384,
            track: a["track"].as_f64().or(a["true_heading"].as_f64()).unwrap_or(0.0).to_radians(),
            vs: if ground { 0.0 } else { a["baro_rate"].as_f64().or(a["geom_rate"].as_f64()).unwrap_or(0.0) * 0.00508 },
            ty: model(a["t"].as_str().unwrap_or(""), a["category"].as_str().unwrap_or("")),
            age: a["seen_pos"].as_f64().unwrap_or(0.0).min(30.0),
        });
    }
    Some(out)
}

impl Live {
    /// A freshly fetched cloud map of the Earth, once.
    pub fn take_clouds(&self) -> Option<Vec<u8>> {
        self.shared.lock().clouds.take()
    }

    /// Starts the fetching thread. Nothing is fetched while a feed is off.
    pub fn new(weather: bool, traffic: bool) -> Self {
        let shared = Arc::new(Mutex::new(Shared { at: (0.0, 0.0), want: (false, false), metar: None, reports: None, clouds: None }));
        #[cfg(not(target_arch = "wasm32"))]
        Self::spawn(&shared, weather);
        Self { shared, planes: HashMap::new(), weather, traffic, station: String::new(), polls: Polls::default() }
    }

    /// In the browser there is no thread to wait on downloads: each frame
    /// looks whether one has come in, and starts the next when it is due.
    #[cfg(target_arch = "wasm32")]
    fn poll(&mut self) {
        use crate::plat::{http_take, now_ms};
        let now = now_ms() / 1000.0;
        let (at, want) = {
            let s = self.shared.lock();
            (s.at, s.want)
        };
        let p = &mut self.polls;
        if self.weather && now >= p.clouds_next {
            match http_take(CLOUD_MAP) {
                Some(Ok(b)) => {
                    let m = cloud_map(&b);
                    p.clouds_next = now + if m.is_some() { 3.0 * 3600.0 } else { 300.0 };
                    if let Some(m) = m {
                        log::info!("live cloud map of the Earth loaded");
                        self.shared.lock().clouds = Some(m);
                    }
                }
                Some(Err(_)) => p.clouds_next = now + 300.0,
                None => {}
            }
        }
        if at == (0.0, 0.0) {
            return;
        }
        let moved = p.wx_at.is_none_or(|(_, q)| (q.0 - at.0).abs() + (q.1 - at.1).abs() > 0.4);
        if let Some(url) = p.wx_url.clone() {
            match http_take(&url) {
                Some(r) => {
                    p.wx_url = None;
                    if let Some(m) = r.ok().and_then(|b| metar(&b, at.0, at.1)) {
                        log::info!("live weather from {}: {:?}", m.station, m);
                        self.shared.lock().metar = Some(m);
                    }
                }
                None => {}
            }
        } else if want.0 && (moved || p.wx_at.is_none_or(|(t, _)| now - t > 600.0)) {
            p.wx_at = Some((now, at));
            let u = metar_url(at.0, at.1);
            let _ = http_take(&u);
            p.wx_url = Some(u);
        }
        if let Some(url) = p.ac_url.clone() {
            match http_take(&url) {
                Some(r) => {
                    p.ac_url = None;
                    match r.ok().and_then(|b| reports(&b)) {
                        Some(r) => self.shared.lock().reports = Some(r),
                        // the other feed next time
                        None => p.ac_alt = !p.ac_alt,
                    }
                }
                None => {}
            }
        } else if want.1 && p.ac_at.is_none_or(|t| now - t > 6.0) {
            p.ac_at = Some(now);
            let u = report_urls(at.0, at.1)[p.ac_alt as usize].clone();
            let _ = http_take(&u);
            p.ac_url = Some(u);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn spawn(shared: &Arc<Mutex<Shared>>, weather: bool) {
        let get = |agent: &ureq::Agent, url: &str| -> Option<Vec<u8>> { agent.get(url).call().ok()?.body_mut().read_to_vec().ok() };
        if weather {
            // The whole Earth's cloud cover for the planet in the menus: a
            // satellite composite that is redone every three hours.
            let sh = shared.clone();
            let _ = std::thread::Builder::new().name("live clouds".into()).spawn(move || {
                let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(30))).user_agent("openflight-flightsim/0.1").build().into();
                loop {
                    let map = get(&agent, CLOUD_MAP).and_then(|b| cloud_map(&b));
                    let ok = map.is_some();
                    if let Some(m) = map {
                        log::info!("live cloud map of the Earth loaded");
                        sh.lock().clouds = Some(m);
                    }
                    std::thread::sleep(Duration::from_secs(if ok { 3 * 3600 } else { 300 }));
                }
            });
        }
        let sh = shared.clone();
        let _ = std::thread::Builder::new().name("live".into()).spawn(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(12))).user_agent("openflight-flightsim/0.1").build().into();
            let mut wx_at: Option<(Instant, (f64, f64))> = None;
            let mut ac_at: Option<Instant> = None;
            loop {
                std::thread::sleep(Duration::from_millis(500));
                let (at, want) = {
                    let s = sh.lock();
                    (s.at, s.want)
                };
                if at == (0.0, 0.0) {
                    continue;
                }
                // The weather: every ten minutes, or sooner having flown somewhere else.
                let moved = wx_at.is_none_or(|(_, p)| (p.0 - at.0).abs() + (p.1 - at.1).abs() > 0.4);
                if want.0 && (moved || wx_at.is_none_or(|(t, _)| t.elapsed() > Duration::from_secs(600))) {
                    wx_at = Some((Instant::now(), at));
                    if let Some(m) = get(&agent, &metar_url(at.0, at.1)).and_then(|b| metar(&b, at.0, at.1)) {
                        log::info!("live weather from {}: {:?}", m.station, m);
                        sh.lock().metar = Some(m);
                    }
                }
                if want.1 && ac_at.is_none_or(|t| t.elapsed() > Duration::from_secs(6)) {
                    let first = ac_at.is_none();
                    ac_at = Some(Instant::now());
                    let got = report_urls(at.0, at.1).iter().find_map(|u| get(&agent, u).and_then(|b| reports(&b)));
                    if first {
                        log::info!("live traffic: {}", got.as_ref().map_or("no answer".to_string(), |r| format!("{} aircraft reporting within 45 nm", r.len())));
                    }
                    if let Some(r) = got {
                        sh.lock().reports = Some(r);
                    }
                }
            }
        });
    }

    /// How many real aircraft are around.
    pub fn count(&self) -> usize {
        if self.traffic { self.planes.len() } else { 0 }
    }

    /// Each frame: takes what has arrived (new weather goes into `wx`) and
    /// carries the aircraft on. `ground` is the height of the ground at a lat/lon.
    pub fn update(&mut self, dt: f64, player: DVec3, wx: &mut Weather, ground: &dyn Fn(f64, f64) -> Option<f64>) {
        #[cfg(target_arch = "wasm32")]
        self.poll();
        let (lat, lon, _) = geo::pos_to_lla(player);
        let (metar, reports) = {
            let mut s = self.shared.lock();
            s.at = (lat.to_degrees(), lon.to_degrees());
            s.want = (self.weather, self.traffic);
            (s.metar.take(), s.reports.take())
        };
        if let (Some(m), true) = (metar, self.weather) {
            wx.clouds = m.clouds;
            if let Some(b) = m.base {
                wx.cloud_base = b.clamp(200.0, 6000.0);
            }
            wx.wind = m.wind;
            if let Some(d) = m.wind_from {
                wx.wind_from = d;
            }
            wx.gusts = m.gusts;
            wx.haze = (16.0 / m.vis.max(0.4)).clamp(1.0, 40.0);
            wx.storms = if m.storm { 0.6 } else { 0.0 };
            self.station = m.station;
        }
        if !self.traffic {
            self.planes.clear();
            return;
        }
        if let Some(rs) = reports {
            for r in rs {
                let p = self.planes.entry(r.hex.clone()).or_insert_with(|| {
                    let h = r.hex.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
                    Plane { lat: 0.0, lon: 0.0, alt: None, gs: 0.0, track: 0.0, vs: 0.0, ty: 0, shown: None, ground: 0.0, ground_in: 0.0, quiet: 0.0, paint: ((h % 8) as u8, ((h >> 8) % 12) as u8) }
                });
                // from where it was then to where it is now
                let d = r.gs * r.age;
                p.lat = r.lat + d * r.track.cos() / geo::R;
                p.lon = r.lon + d * r.track.sin() / (geo::R * r.lat.cos().max(0.01));
                p.alt = r.alt.map(|a| a + r.vs * r.age);
                (p.gs, p.track, p.vs, p.ty, p.quiet) = (r.gs, r.track, r.vs, r.ty, 0.0);
            }
        }
        for p in self.planes.values_mut() {
            p.quiet += dt;
            let d = p.gs * dt;
            p.lat += d * p.track.cos() / geo::R;
            p.lon += d * p.track.sin() / (geo::R * p.lat.cos().max(0.01));
            p.alt = p.alt.map(|a| a + p.vs * dt);
            p.ground_in -= dt as f32;
            if p.ground_in <= 0.0 {
                p.ground_in = if p.alt.is_none() { 0.5 } else { 3.0 };
                if let Some(g) = ground(p.lat, p.lon) {
                    p.ground = g;
                }
            }
            let h = p.alt.map_or(p.ground, |a| a.max(p.ground));
            let want = geo::lla_to_pos(p.lat, p.lon, h);
            // shown where it is, closing any jump from a new report over a few seconds
            let near = p.shown.is_some_and(|s| (s - want).length() < 3000.0);
            p.shown = Some(if near {
                let s = p.shown.unwrap();
                let (e, n, u) = geo::enu(s);
                let v = (n * p.track.cos() + e * p.track.sin()) * p.gs + u * p.vs;
                let s = s + v * dt;
                s + (want - s) * (1.0 - (-dt / 2.5).exp())
            } else {
                want
            });
        }
        self.planes.retain(|_, p| p.quiet < 45.0);
    }

    pub fn draw(&self, cam: DVec3, out: &mut Vec<(u32, Inst)>, lights: &mut Vec<Light>) {
        if !self.traffic {
            return;
        }
        for (k, p) in self.planes.values().enumerate() {
            let Some(origin) = p.shown else { continue };
            let o = (origin - cam).as_vec3();
            let dist = o.length();
            // not the one the player is flying, should it be reporting too
            if !(60.0..60_000.0).contains(&dist) {
                continue;
            }
            let (e, n, u) = geo::enu(origin);
            let f = (n * p.track.cos() + e * p.track.sin()).as_vec3();
            let up = u.as_vec3();
            let pitch = if p.gs > 20.0 { (p.vs / p.gs).atan() as f32 + if p.alt.is_some() { 0.04 } else { 0.0 } } else { 0.0 };
            let (sp, cp) = pitch.sin_cos();
            let fwd = f * cp + up * sp;
            let y = up * cp - f * sp;
            let (x, z) = (fwd.cross(y), -fwd);
            let ty = TYPES[p.ty as usize];
            let mut push = |mesh: u32, tint: [f32; 3]| {
                out.push((mesh, Inst { m: [[x.x, y.x, z.x, o.x], [x.y, y.y, z.y, o.y], [x.z, y.z, z.z, o.z]], tint: [tint[0], tint[1], tint[2], 0.0], ext: [0.0; 4] }));
            };
            let base = am::M_BODY + 3 * p.ty as u32;
            push(base, crate::traffic::BODY[p.paint.0 as usize]);
            push(base + 1, crate::traffic::FIN[p.paint.1 as usize]);
            let low = p.alt.is_none_or(|a| a - p.ground < 600.0);
            if low {
                push(base + 2, [0.5; 3]);
            }
            let at = |q: Vec3| -> [f32; 3] { (o + x * q.x + y * q.y - z * q.z).into() };
            let mut l = |q: Vec3, inten: f32, col: [u8; 3], fx: [u8; 4]| {
                if lights.len() < crate::render::MAX_DYN_LIGHTS {
                    lights.push(Light { pos: at(q), inten, dir: [0.0; 3], spread: 0.0, col: [col[0], col[1], col[2], 0], fx });
                }
            };
            let ph = (k as u32 * 97 % 256) as u8;
            let steady = [0, 0, 255, 1];
            l(Vec3::new(-ty.tip.x, ty.tip.y, -ty.tip.z), 40.0, L_RED, steady);
            l(Vec3::new(ty.tip.x, ty.tip.y, -ty.tip.z), 40.0, L_GREEN, steady);
            l(Vec3::new(0.0, ty.tail.y * 0.35, -ty.tail.z), 25.0, L_WHITE, steady);
            l(Vec3::new(0.0, ty.cy + ty.rf + 0.1, 0.0), 120.0, L_RED, [12, ph, 25, 1]);
            if p.alt.is_some() {
                for sx in [-1.0, 1.0] {
                    l(Vec3::new(sx * ty.tip.x, ty.tip.y, -ty.tip.z), 1600.0, L_STROBE, [11, ph.wrapping_add(60), 7, 1]);
                }
            }
        }
    }
}
