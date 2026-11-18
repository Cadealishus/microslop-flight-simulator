//! Multiplayer: flying together in a room picked by a short code.
//!
//! There is no game server. Every player connects to a free public MQTT
//! broker (a message relay) over a secure WebSocket, subscribes to the
//! room's topic and publishes their own aircraft's state to it ten times a
//! second. The desktop (Windows, Mac) and the browser speak the same bytes
//! to the same brokers, so they all meet in the same rooms. If one broker
//! does not answer, the next is tried; everyone in a room settles on the
//! first that works, as they all try them in the same order.
//!
//! Whoever made the room (the host) also sends the time of day and the
//! weather, which everyone else takes, so all see the same sky.
//!
//! What is sent is visible to anyone who listens on that topic of the public
//! broker: positions and a player name, nothing else.

use glam::{DQuat, DVec3};
use std::collections::BTreeMap;

/// Tried in this order (all public, no account; WebSocket over TLS).
const BROKERS: [&str; 3] = ["wss://broker.emqx.io:8084/mqtt", "wss://broker.hivemq.com:8884/mqtt", "wss://test.mosquitto.org:8081/mqtt"];
const TOPIC: &str = "microslop-fs/v1";
/// State messages a second.
const RATE: f64 = 10.0;
/// Give a broker this long to let us in before trying the next.
const CONNECT_S: f64 = 8.0;
/// A player not heard from for this long has left.
const GONE_S: f64 = 12.0;
const CODE_CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// The host's sky, as everyone takes it.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Sky {
    pub utc: f32,
    pub clouds: f32,
    pub cloud_base: f32,
    pub wind: f32,
    pub wind_from: f32,
    pub storms: f32,
    pub haze: f32,
}

/// One aircraft's state, as sent.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub id: u32,
    /// counts up with each report (the same one through two relays counts once)
    pub seq: u32,
    /// when it was true: the sender's own clock (seconds, real time)
    pub time: f64,
    pub name: String,
    /// index into `aircraft::ALL`
    pub spec: u8,
    pub host: bool,
    /// at the menus, or wrecked: not drawn
    pub away: bool,
    pub pos: DVec3,
    pub rot: DQuat,
    pub vel: DVec3,
    pub stick: [f32; 3],
    pub flaps: f32,
    pub slats: f32,
    pub spoilers: f32,
    pub gear: f32,
    pub gear_down: bool,
    pub throttle: f32,
    pub prop_rate: f32,
    pub burner: f32,
    pub n1: f32,
    pub sky: Sky,
}

impl State {
    fn encode(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(160);
        b.extend_from_slice(&[b'M', 3, 1]);
        b.extend_from_slice(&self.id.to_le_bytes());
        b.extend_from_slice(&self.seq.to_le_bytes());
        b.extend_from_slice(&self.time.to_le_bytes());
        b.push(self.spec);
        b.push(self.host as u8 | (self.away as u8) << 1 | (self.gear_down as u8) << 2);
        let name = &self.name.as_bytes()[..self.name.len().min(24)];
        b.push(name.len() as u8);
        b.extend_from_slice(name);
        for v in [self.pos.x, self.pos.y, self.pos.z] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        let r = self.rot.as_quat();
        let s = &self.sky;
        for v in [r.x, r.y, r.z, r.w, self.vel.x as f32, self.vel.y as f32, self.vel.z as f32, self.stick[0], self.stick[1], self.stick[2], self.flaps, self.slats, self.spoilers, self.gear, self.throttle, self.prop_rate, self.burner, self.n1, s.utc, s.clouds, s.cloud_base, s.wind, s.wind_from, s.storms, s.haze] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b
    }

    fn decode(b: &[u8]) -> Option<State> {
        if b.len() < 10 || b[0] != b'M' || b[1] != 3 || b[2] != 1 {
            return None;
        }
        let mut at = 3;
        let mut take = |n: usize| -> Option<&[u8]> {
            let s = b.get(at..at + n)?;
            at += n;
            Some(s)
        };
        let id = u32::from_le_bytes(take(4)?.try_into().ok()?);
        let seq = u32::from_le_bytes(take(4)?.try_into().ok()?);
        let time = f64::from_le_bytes(take(8)?.try_into().ok()?);
        let spec = take(1)?[0];
        let flags = take(1)?[0];
        let n = take(1)?[0] as usize;
        let name = String::from_utf8_lossy(take(n)?).into_owned();
        let mut d = [0f64; 3];
        for v in &mut d {
            *v = f64::from_le_bytes(take(8)?.try_into().ok()?);
        }
        let mut f = [0f32; 25];
        for v in &mut f {
            *v = f32::from_le_bytes(take(4)?.try_into().ok()?);
        }
        let rot = DQuat::from_xyzw(f[0] as f64, f[1] as f64, f[2] as f64, f[3] as f64);
        if !d.iter().all(|x| x.is_finite()) || !f.iter().all(|x| x.is_finite()) || rot.length_squared() < 0.5 {
            return None;
        }
        Some(State {
            id,
            seq,
            time,
            name,
            spec,
            host: flags & 1 != 0,
            away: flags & 2 != 0,
            gear_down: flags & 4 != 0,
            pos: DVec3::from(d),
            rot: rot.normalize(),
            vel: DVec3::new(f[4] as f64, f[5] as f64, f[6] as f64),
            stick: [f[7], f[8], f[9]],
            flaps: f[10],
            slats: f[11],
            spoilers: f[12],
            gear: f[13],
            throttle: f[14],
            prop_rate: f[15],
            burner: f[16],
            n1: f[17],
            sky: Sky { utc: f[18], clouds: f[19], cloud_base: f[20], wind: f[21], wind_from: f[22], storms: f[23], haze: f[24] },
        })
    }
}

/// Another player, as last heard, and where they are drawn.
///
/// Reports come in ten times a second, unevenly (through three relays, each
/// with its own delay). Jumping to each one as it arrives makes the aircraft
/// hop back and forth, so they are kept, by the time the sender stamped them,
/// and the aircraft is played back a little behind the newest: always between
/// two reports, gliding from one to the next.
pub struct Peer {
    /// the newest report
    pub last: State,
    /// seconds (real time) since a report came in
    pub age: f64,
    /// where and how they are drawn now
    pub shown: DVec3,
    pub shown_rot: DQuat,
    /// recent reports, oldest first
    reports: std::collections::VecDeque<State>,
    /// the moment (on the sender's clock) being played back
    play: f64,
}

/// How far behind the newest report the others are played back (s): enough
/// to have the next report in hand nearly always.
const DELAY: f64 = 0.25;

impl Peer {
    fn new(s: State) -> Peer {
        Peer { shown: s.pos, shown_rot: s.rot, play: s.time - DELAY, reports: std::iter::once(s.clone()).collect(), last: s, age: 0.0 }
    }

    fn add(&mut self, s: State) {
        if self.reports.iter().any(|r| r.time == s.time) {
            return;
        }
        let at = self.reports.iter().position(|r| r.time > s.time).unwrap_or(self.reports.len());
        self.reports.insert(at, s);
        while self.reports.len() > 40 {
            self.reports.pop_front();
        }
        self.last = self.reports.back().unwrap().clone();
        self.age = 0.0;
    }

    /// Moves the playback on by `dt` (real seconds) and places the aircraft there.
    fn step(&mut self, dt: f64) {
        self.age += dt;
        let newest = self.last.time;
        // The playback clock runs at real time, nudged to stay DELAY behind the
        // newest report (and set straight after a long gap).
        self.play += dt;
        let err = newest - DELAY - self.play;
        if err.abs() > 1.0 {
            self.play = newest - DELAY;
        } else {
            self.play += err * (dt * 1.5).min(1.0);
        }
        let t = self.play;
        while self.reports.len() > 2 && self.reports[1].time <= t {
            self.reports.pop_front();
        }
        let a = &self.reports[0];
        match self.reports.get(1) {
            Some(b) if a.time <= t => {
                let k = ((t - a.time) / (b.time - a.time).max(1e-6)).clamp(0.0, 1.0);
                // Along the way between the two, curving with their speeds (Hermite).
                let h = b.time - a.time;
                let (k2, k3) = (k * k, k * k * k);
                self.shown = a.pos * (2.0 * k3 - 3.0 * k2 + 1.0) + a.vel * h * (k3 - 2.0 * k2 + k) + b.pos * (-2.0 * k3 + 3.0 * k2) + b.vel * h * (k3 - k2);
                self.shown_rot = a.rot.slerp(b.rot, k).normalize();
            }
            // Ahead of the newest report (it is late): carry on along its velocity, briefly.
            _ => {
                let r = self.reports.back().unwrap();
                self.shown = r.pos + r.vel * (t - r.time).clamp(0.0, 0.5);
                self.shown_rot = r.rot;
            }
        }
    }
}

impl Peer {
    pub fn name(&self) -> &str {
        if self.last.name.is_empty() { "Pilot" } else { &self.last.name }
    }
}

/// Real seconds since the program started (the clock reports are stamped with).
fn wall() -> f64 {
    use std::sync::OnceLock;
    static T0: OnceLock<crate::plat::Instant> = OnceLock::new();
    T0.get_or_init(crate::plat::Instant::now).elapsed().as_secs_f64()
}

/// What happened, for the game to act on.
pub enum Event {
    /// someone came in (or a room was entered and they were already there)
    Joined(String),
    Left(String),
    /// the host's position, the first time it is heard after joining: start beside it
    FirstHost(State),
    /// someone crashed: everyone shows the blast, the fire and the wreck
    Crash(Boom),
}

/// A crash, as sent to the others.
#[derive(Clone, Debug, Default)]
pub struct Boom {
    pub name: String,
    pub spec: u8,
    pub site: DVec3,
    pub vel: DVec3,
    pub rot: DQuat,
    /// how hard (crash::Crash::power) and how big the aircraft (span / a light single's)
    pub power: f32,
    pub size: f32,
    pub water: bool,
}

impl Boom {
    fn encode(&self, id: u32, n: u16) -> Vec<u8> {
        let mut b = vec![b'M', 2, 3];
        b.extend_from_slice(&id.to_le_bytes());
        b.extend_from_slice(&n.to_le_bytes());
        b.push(self.spec);
        b.push(self.water as u8);
        for v in [self.site.x, self.site.y, self.site.z] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        let r = self.rot.as_quat();
        for v in [self.vel.x as f32, self.vel.y as f32, self.vel.z as f32, r.x, r.y, r.z, r.w, self.power, self.size] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b
    }
    fn decode(b: &[u8]) -> Option<(u32, u16, Boom)> {
        if b.len() != 3 + 4 + 2 + 2 + 24 + 36 || b[..3] != [b'M', 2, 3] {
            return None;
        }
        let f = |i: usize| f32::from_le_bytes(b[35 + 4 * i..39 + 4 * i].try_into().unwrap());
        let d = |i: usize| f64::from_le_bytes(b[11 + 8 * i..19 + 8 * i].try_into().unwrap());
        let boom = Boom {
            name: String::new(),
            spec: b[9],
            water: b[10] != 0,
            site: DVec3::new(d(0), d(1), d(2)),
            vel: DVec3::new(f(0) as f64, f(1) as f64, f(2) as f64),
            rot: DQuat::from_xyzw(f(3) as f64, f(4) as f64, f(5) as f64, f(6) as f64).normalize(),
            power: f(7).clamp(0.0, 4.0),
            size: f(8).clamp(0.5, 20.0),
        };
        boom.site.is_finite().then_some((u32::from_le_bytes(b[3..7].try_into().unwrap()), u16::from_le_bytes([b[7], b[8]]), boom))
    }
}

#[derive(PartialEq, Clone, Copy)]
enum Phase {
    /// socket opening or MQTT handshake under way, since then
    Connecting(f64),
    Live,
    /// waiting to try again, until then
    Retry(f64),
}

/// One relay: every player keeps one to each broker, sends to all and
/// takes from all (the same message from two of them counts once), so two
/// players meet as long as any one broker works for both.
struct Conn {
    broker: usize,
    link: Option<link::Link>,
    phase: Phase,
    buf: Vec<u8>,
    ping_at: f64,
}

pub struct Net {
    pub me: u32,
    pub name: String,
    /// the room (its code), and whether we made it
    pub room: Option<String>,
    pub hosting: bool,
    pub peers: BTreeMap<u32, Peer>,
    /// the host's sky, last heard (when not hosting)
    pub sky: Option<Sky>,
    conns: Vec<Conn>,
    t: f64,
    send_at: f64,
    seq: u32,
    host_seen: bool,
    /// real time at the last tick
    last_wall: Option<f64>,
    /// our last crash, sent a few times over (a lost message would mean a crash nobody saw): message, sends left
    boom: Option<(Vec<u8>, u8)>,
    booms: u16,
    /// the last crash heard from each player (it comes through every relay, and more than once)
    heard: BTreeMap<u32, u16>,
}

/// What this player sends; the game fills it in each frame.
pub type Mine = State;

impl Net {
    pub fn new(seed: u64) -> Self {
        let me = (seed ^ (seed >> 29) ^ 0x9e37_79b9) as u32 | 1;
        Self { me, name: format!("Pilot {}", me % 10_000), room: None, hosting: false, peers: BTreeMap::new(), sky: None, conns: Vec::new(), t: 0.0, send_at: 0.0, seq: 0, host_seen: false, boom: None, booms: 0, heard: BTreeMap::new(), last_wall: None }
    }

    /// Make a room; returns its code.
    pub fn host(&mut self) -> String {
        let mut x = (self.me as u64).wrapping_mul(0x2545_f491_4f6c_dd1d) ^ (crate::plat::SystemTime::now().duration_since(crate::plat::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64));
        let code: String = (0..5)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                CODE_CHARS[(x % CODE_CHARS.len() as u64) as usize] as char
            })
            .collect();
        self.enter(code.clone(), true);
        code
    }

    /// Join a room by its code (spaces and case don't matter).
    pub fn join(&mut self, code: &str) -> bool {
        let code: String = code.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect();
        if code.len() < 4 {
            return false;
        }
        self.enter(code, false);
        true
    }

    fn enter(&mut self, code: String, host: bool) {
        self.leave();
        self.room = Some(code);
        self.hosting = host;
        self.host_seen = host;
        self.conns = (0..BROKERS.len()).map(|broker| Conn { broker, link: None, phase: Phase::Retry(0.0), buf: Vec::new(), ping_at: 0.0 }).collect();
    }

    pub fn leave(&mut self) {
        if let Some(room) = &self.room {
            let bye = [b'M', 2, 2, self.me as u8, (self.me >> 8) as u8, (self.me >> 16) as u8, (self.me >> 24) as u8];
            for c in &self.conns {
                if let (Some(l), Phase::Live) = (&c.link, c.phase) {
                    l.send(mqtt::publish(&format!("{TOPIC}/{room}/{}", self.me), &bye));
                    l.send(vec![0xE0, 0]);
                }
            }
        }
        for c in &mut self.conns {
            if let Some(l) = c.link.take() {
                l.close();
            }
        }
        self.conns.clear();
        self.room = None;
        self.hosting = false;
        self.peers.clear();
        self.sky = None;
    }

    /// We crashed: tell the others.
    pub fn crash(&mut self, b: &Boom) {
        if self.room.is_some() {
            self.booms = self.booms.wrapping_add(1);
            self.boom = Some((b.encode(self.me, self.booms), 3));
        }
    }

    pub fn live(&self) -> bool {
        self.conns.iter().any(|c| c.phase == Phase::Live)
    }

    /// A line for the HUD and menus.
    pub fn status(&self) -> String {
        let Some(room) = &self.room else { return "Not in a room".into() };
        let n = self.peers.len();
        if self.live() {
            format!("Room {room} · {n} other{} here", if n == 1 { "" } else { "s" })
        } else {
            format!("Room {room} · connecting…")
        }
    }

    /// Each frame: keeps the connections, sends ours, takes theirs.
    pub fn tick(&mut self, dt: f64, mine: &Mine) -> Vec<Event> {
        let mut ev = Vec::new();
        self.t += dt;
        let Some(room) = self.room.clone() else { return ev };
        let topic = format!("{TOPIC}/{room}");
        let t = self.t;
        let send = t >= self.send_at;
        let boom = match (&mut self.boom, send) {
            (Some((b, left)), true) => {
                *left -= 1;
                Some(mqtt::publish(&format!("{topic}/{}", self.me), b))
            }
            _ => None,
        };
        if self.boom.as_ref().is_some_and(|b| b.1 == 0) {
            self.boom = None;
        }
        let packet = send.then(|| {
            self.send_at = t + 1.0 / RATE;
            self.seq = self.seq.wrapping_add(1);
            let mut m = mine.clone();
            (m.id, m.host, m.name, m.seq, m.time) = (self.me, self.hosting, self.name.clone(), self.seq, wall());
            mqtt::publish(&format!("{topic}/{}", self.me), &m.encode())
        });
        let mut got: Vec<Vec<u8>> = Vec::new();
        for c in &mut self.conns {
            let url = BROKERS[c.broker];
            let mut fail = |c: &mut Conn, why: &str| {
                log::warn!("multiplayer: {url} {why}");
                if let Some(l) = c.link.take() {
                    l.close();
                }
                c.buf.clear();
                c.phase = Phase::Retry(t + 5.0);
            };
            match c.phase {
                Phase::Retry(at) if t >= at => {
                    c.link = Some(link::Link::open(url));
                    c.buf.clear();
                    c.phase = Phase::Connecting(t);
                }
                Phase::Connecting(since) if t - since > CONNECT_S => fail(c, "did not answer"),
                _ => {}
            }
            let events = c.link.as_ref().map(|l| l.poll()).unwrap_or_default();
            for e in events {
                match e {
                    link::Ev::Open => {
                        if let Some(l) = &c.link {
                            l.send(mqtt::connect(&format!("msfs{:08x}{}", self.me, c.broker)));
                        }
                    }
                    link::Ev::Data(d) => c.buf.extend_from_slice(&d),
                    link::Ev::Closed(why) => {
                        fail(c, &why);
                        break;
                    }
                }
            }
            while let Some((kind, body, n)) = mqtt::next_packet(&c.buf).map(|(k, b, n)| (k, b.to_vec(), n)) {
                match kind >> 4 {
                    // CONNACK: in (return code 0), so listen to the room
                    2 if body.get(1) == Some(&0) => {
                        if let Some(l) = &c.link {
                            l.send(mqtt::subscribe(&format!("{topic}/+")));
                        }
                        c.phase = Phase::Live;
                        c.ping_at = t + 30.0;
                        log::info!("multiplayer: in room {room} via {url}");
                    }
                    2 => {
                        fail(c, "refused");
                        break;
                    }
                    3 => {
                        if let Some(p) = mqtt::publish_payload(kind, &body) {
                            got.push(p.to_vec());
                        }
                    }
                    _ => {}
                }
                c.buf.drain(..n);
            }
            if let (Some(l), Phase::Live) = (&c.link, c.phase) {
                if let Some(p) = &packet {
                    l.send(p.clone());
                }
                if let Some(b) = &boom {
                    l.send(b.clone());
                }
                if t >= c.ping_at {
                    c.ping_at = t + 30.0;
                    l.send(vec![0xC0, 0]);
                }
            }
        }
        for b in got {
            self.receive(&b, &mut ev);
        }
        // Theirs, played back between their reports (in real time: the game's
        // own clock slows when frames are slow).
        let now = wall();
        let wdt = (now - self.last_wall.unwrap_or(now)).clamp(0.0, 1.0);
        self.last_wall = Some(now);
        for p in self.peers.values_mut() {
            p.step(wdt);
        }
        let gone: Vec<u32> = self.peers.iter().filter(|(_, p)| p.age > GONE_S).map(|(id, _)| *id).collect();
        for id in gone {
            if let Some(p) = self.peers.remove(&id) {
                ev.push(Event::Left(p.name().to_string()));
            }
        }
        ev
    }

    fn receive(&mut self, b: &[u8], ev: &mut Vec<Event>) {
        if b.len() >= 7 && b[0] == b'M' && b[2] == 2 {
            let id = u32::from_le_bytes([b[3], b[4], b[5], b[6]]);
            if let Some(p) = self.peers.remove(&id) {
                ev.push(Event::Left(p.name().to_string()));
            }
            return;
        }
        if let Some((id, n, mut boom)) = Boom::decode(b) {
            if id != self.me && self.heard.get(&id) != Some(&n) {
                self.heard.insert(id, n);
                boom.name = self.peers.get(&id).map_or("Pilot".into(), |p| p.name().to_string());
                ev.push(Event::Crash(boom));
            }
            return;
        }
        let Some(s) = State::decode(b) else { return };
        if s.id == self.me {
            return;
        }
        // The same report through another relay (an older one arriving late is still kept, in its place).
        if self.peers.get(&s.id).is_some_and(|p| p.reports.iter().any(|r| r.seq == s.seq)) {
            return;
        }
        if s.host && !self.hosting {
            self.sky = Some(s.sky);
            if !self.host_seen && !s.away {
                self.host_seen = true;
                ev.push(Event::FirstHost(s.clone()));
            }
        }
        match self.peers.get_mut(&s.id) {
            Some(p) => p.add(s),
            None => {
                ev.push(Event::Joined(if s.name.is_empty() { "Pilot".into() } else { s.name.clone() }));
                self.peers.insert(s.id, Peer::new(s));
            }
        }
    }
}
/// Just enough MQTT 3.1.1: connect, subscribe, publish (QoS 0), ping.
mod mqtt {
    fn len(mut n: usize, out: &mut Vec<u8>) {
        loop {
            let mut b = (n % 128) as u8;
            n /= 128;
            if n > 0 {
                b |= 0x80;
            }
            out.push(b);
            if n == 0 {
                break;
            }
        }
    }
    fn packet(kind: u8, body: &[u8]) -> Vec<u8> {
        let mut out = vec![kind];
        len(body.len(), &mut out);
        out.extend_from_slice(body);
        out
    }
    fn string(s: &str, out: &mut Vec<u8>) {
        out.extend_from_slice(&(s.len() as u16).to_be_bytes());
        out.extend_from_slice(s.as_bytes());
    }
    pub fn connect(client: &str) -> Vec<u8> {
        let mut b = Vec::new();
        string("MQTT", &mut b);
        // level 4 (3.1.1), clean session, keep alive 60 s
        b.extend_from_slice(&[4, 0x02, 0, 60]);
        string(client, &mut b);
        packet(0x10, &b)
    }
    pub fn subscribe(filter: &str) -> Vec<u8> {
        let mut b = vec![0, 1];
        string(filter, &mut b);
        b.push(0);
        packet(0x82, &b)
    }
    pub fn publish(topic: &str, payload: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        string(topic, &mut b);
        b.extend_from_slice(payload);
        packet(0x30, &b)
    }
    /// The first whole packet in `buf`: its first byte, body, and total length.
    pub fn next_packet(buf: &[u8]) -> Option<(u8, &[u8], usize)> {
        let kind = *buf.first()?;
        let (mut n, mut mul, mut i) = (0usize, 1usize, 1usize);
        loop {
            let b = *buf.get(i)?;
            n += (b & 0x7f) as usize * mul;
            mul *= 128;
            i += 1;
            if b & 0x80 == 0 {
                break;
            }
            if i > 4 {
                return None;
            }
        }
        let body = buf.get(i..i + n)?;
        Some((kind, body, i + n))
    }
    pub fn publish_payload(kind: u8, body: &[u8]) -> Option<&[u8]> {
        let tl = u16::from_be_bytes([*body.first()?, *body.get(1)?]) as usize;
        let mut at = 2 + tl;
        if (kind >> 1) & 3 > 0 {
            at += 2;
        }
        body.get(at..)
    }
}

/// The WebSocket under it all: a thread with a blocking socket on the
/// desktop, the browser's own WebSocket in the browser.
mod link {
    pub enum Ev {
        Open,
        Data(Vec<u8>),
        Closed(String),
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub struct Link {
        tx: std::sync::mpsc::Sender<Option<Vec<u8>>>,
        rx: std::sync::mpsc::Receiver<Ev>,
    }

    #[cfg(not(target_arch = "wasm32"))]
    impl Link {
        pub fn open(url: &str) -> Link {
            use std::sync::mpsc::{channel, TryRecvError};
            use tungstenite::client::IntoClientRequest;
            let (tx, out_rx) = channel::<Option<Vec<u8>>>();
            let (ev_tx, rx) = channel();
            let url = url.to_string();
            std::thread::spawn(move || {
                let fail = |e: String| {
                    let _ = ev_tx.send(Ev::Closed(e));
                };
                let mut req = match url.as_str().into_client_request() {
                    Ok(r) => r,
                    Err(e) => return fail(e.to_string()),
                };
                req.headers_mut().insert("Sec-WebSocket-Protocol", "mqtt".parse().unwrap());
                let (mut ws, _) = match tungstenite::connect(req) {
                    Ok(x) => x,
                    Err(e) => return fail(e.to_string()),
                };
                // Short reads, so sending never waits long on a quiet socket.
                let tcp = match ws.get_mut() {
                    tungstenite::stream::MaybeTlsStream::Plain(s) => Some(&*s),
                    tungstenite::stream::MaybeTlsStream::Rustls(s) => Some(s.get_ref()),
                    _ => None,
                };
                if let Some(t) = tcp {
                    let _ = t.set_read_timeout(Some(std::time::Duration::from_millis(15)));
                }
                let _ = ev_tx.send(Ev::Open);
                loop {
                    loop {
                        match out_rx.try_recv() {
                            Ok(Some(b)) => {
                                if let Err(e) = ws.send(tungstenite::Message::Binary(b.into())) {
                                    return fail(e.to_string());
                                }
                            }
                            Ok(None) | Err(TryRecvError::Disconnected) => {
                                let _ = ws.close(None);
                                let _ = ws.flush();
                                return;
                            }
                            Err(TryRecvError::Empty) => break,
                        }
                    }
                    match ws.read() {
                        Ok(tungstenite::Message::Binary(b)) => {
                            if ev_tx.send(Ev::Data(b.to_vec())).is_err() {
                                return;
                            }
                        }
                        Ok(tungstenite::Message::Close(_)) => return fail("closed".into()),
                        Ok(_) => {}
                        Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                        Err(e) => return fail(e.to_string()),
                    }
                }
            });
            Link { tx, rx }
        }
        pub fn send(&self, b: Vec<u8>) {
            let _ = self.tx.send(Some(b));
        }
        pub fn poll(&self) -> Vec<Ev> {
            self.rx.try_iter().collect()
        }
        pub fn close(&self) {
            let _ = self.tx.send(None);
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub struct Link {
        ws: Option<web_sys::WebSocket>,
        got: std::rc::Rc<std::cell::RefCell<Vec<Ev>>>,
    }

    #[cfg(target_arch = "wasm32")]
    impl Link {
        pub fn open(url: &str) -> Link {
            use wasm_bindgen::prelude::*;
            use wasm_bindgen::JsCast;
            let got = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let ws = match web_sys::WebSocket::new_with_str(url, "mqtt") {
                Ok(ws) => ws,
                Err(e) => {
                    got.borrow_mut().push(Ev::Closed(format!("{e:?}")));
                    return Link { ws: None, got };
                }
            };
            ws.set_binary_type(web_sys::BinaryType::Arraybuffer);
            let g = got.clone();
            let on_open = Closure::<dyn FnMut()>::new(move || g.borrow_mut().push(Ev::Open));
            let g = got.clone();
            let on_msg = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
                if let Ok(buf) = e.data().dyn_into::<js_sys::ArrayBuffer>() {
                    g.borrow_mut().push(Ev::Data(js_sys::Uint8Array::new(&buf).to_vec()));
                }
            });
            let g = got.clone();
            let on_close = Closure::<dyn FnMut(web_sys::CloseEvent)>::new(move |e: web_sys::CloseEvent| g.borrow_mut().push(Ev::Closed(format!("closed ({})", e.code()))));
            ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
            ws.set_onmessage(Some(on_msg.as_ref().unchecked_ref()));
            ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
            on_open.forget();
            on_msg.forget();
            on_close.forget();
            Link { ws: Some(ws), got }
        }
        pub fn send(&self, b: Vec<u8>) {
            if let Some(ws) = &self.ws {
                if ws.ready_state() == web_sys::WebSocket::OPEN {
                    let _ = ws.send_with_u8_array(&b);
                }
            }
        }
        pub fn poll(&self) -> Vec<Ev> {
            std::mem::take(&mut *self.got.borrow_mut())
        }
        pub fn close(&self) {
            if let Some(ws) = &self.ws {
                ws.set_onclose(None);
                ws.set_onmessage(None);
                let _ = ws.close();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trip() {
        let s = State { id: 7, name: "Cade".into(), spec: 3, host: true, gear_down: true, pos: DVec3::new(1.0, -2.5, 6.4e6), rot: DQuat::from_rotation_y(0.4), vel: DVec3::new(50.0, 0.0, -3.0), flaps: 0.5, sky: Sky { utc: 13.5, clouds: 0.4, ..Default::default() }, ..Default::default() };
        let d = State::decode(&s.encode()).unwrap();
        assert_eq!((d.id, d.name.as_str(), d.spec, d.host, d.away, d.gear_down), (7, "Cade", 3, true, false, true));
        assert!((d.pos - s.pos).length() < 1e-9 && (d.vel - s.vel).length() < 1e-4);
        assert_eq!(d.sky, s.sky);
        assert!(State::decode(&[b'M', 3, 1, 0]).is_none());
    }

    /// Two players meet in a room through the real brokers:
    /// `cargo test --release net::tests::meet -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn meet() {
        let (mut a, mut b) = (Net::new(11), Net::new(22));
        let code = a.host();
        let mine = |x: f64| State { pos: DVec3::new(x, 0.0, 6.4e6), rot: DQuat::IDENTITY, sky: Sky { utc: 9.25, ..Default::default() }, ..Default::default() };
        let mut joined = false;
        for i in 0..400 {
            if i == 20 {
                assert!(b.join(&code));
            }
            a.tick(0.05, &mine(1.0));
            b.tick(0.05, &mine(2.0));
            if i % 40 == 0 {
                println!("{:5.1}s  a: {}  |  b: {}", i as f64 * 0.05, a.status(), b.status());
            }
            if a.peers.contains_key(&b.me) && b.peers.contains_key(&a.me) && b.sky.is_some() {
                joined = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(joined, "the two never saw each other");
        assert_eq!(b.sky.unwrap().utc, 9.25);
        assert!((b.peers[&a.me].last.pos.x - 1.0).abs() < 1e-9);
        println!("met in room {code}");
    }

    /// Reports arriving unevenly and out of order still play back smoothly:
    /// the drawn position only ever moves forward, at about the true speed.
    #[test]
    fn smooth_playback() {
        let at = |t: f64| State { time: t, seq: (t * 10.0) as u32, pos: DVec3::new(50.0 * t, 0.0, 0.0), vel: DVec3::new(50.0, 0.0, 0.0), rot: DQuat::IDENTITY, ..Default::default() };
        let mut p = Peer::new(at(0.0));
        let mut x = p.shown.x;
        let jitter = [0.0, 0.09, 0.02, 0.12, 0.0, 0.05, 0.11, 0.01];
        let mut next = 1usize;
        for step in 1..300 {
            let now = step as f64 / 60.0;
            // report n (made at n/10 s) arrives n/10 s + a jittery delay later
            while (next as f64) / 10.0 + 0.05 + jitter[next % jitter.len()] <= now {
                p.add(at(next as f64 / 10.0));
                next += 1;
            }
            p.step(1.0 / 60.0);
            if step > 60 {
                let v = (p.shown.x - x) * 60.0;
                assert!(v > 30.0 && v < 70.0, "speed {v} at step {step}");
            }
            x = p.shown.x;
        }
    }

    #[test]
    fn boom_round_trip() {
        let b = Boom { spec: 2, site: DVec3::new(1.0, 2.0, 6.4e6), vel: DVec3::new(-40.0, 1.0, 3.0), rot: DQuat::from_rotation_x(0.3), power: 1.5, size: 2.0, water: true, ..Default::default() };
        let (id, n, d) = Boom::decode(&b.encode(9, 4)).unwrap();
        assert_eq!((id, n, d.spec, d.water), (9, 4, 2, true));
        assert!((d.site - b.site).length() < 1e-9 && (d.power - 1.5).abs() < 1e-6);
    }

    #[test]
    fn mqtt_packets() {
        let p = mqtt::publish("a/b", &[1, 2, 3]);
        let (kind, body, n) = mqtt::next_packet(&p).unwrap();
        assert_eq!((kind, n), (0x30, p.len()));
        assert_eq!(mqtt::publish_payload(kind, body), Some(&[1u8, 2, 3][..]));
        // a body over 127 bytes takes two length bytes
        let big = mqtt::publish("t", &[0; 300]);
        assert_eq!(mqtt::next_packet(&big).unwrap().2, big.len());
        assert!(mqtt::next_packet(&big[..big.len() - 1]).is_none());
    }
}
