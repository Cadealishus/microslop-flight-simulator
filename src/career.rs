//! The pilot's logbook: flights, time in the air, landings, photos, jobs and
//! the best landing-challenge scores, kept in a small text file beside the game.

/// The landing challenges: airport, name, what makes it hard.
pub const CHALLENGES: [(&str, &str, &str); 5] = [
    ("KSFO", "San Francisco", "Warm-up: a long runway over the bay"),
    ("TNCM", "St Maarten", "Low over the beach"),
    ("LXGB", "Gibraltar", "A short runway beside the Rock"),
    ("NZQN", "Queenstown", "Down a mountain valley"),
    ("LFLJ", "Courchevel", "A steep altiport, one way in"),
];

/// Rank and the points it takes.
const RANKS: [(&str, f64); 6] = [("Student Pilot", 0.0), ("Private Pilot", 30.0), ("Commercial Pilot", 120.0), ("First Officer", 300.0), ("Captain", 600.0), ("Senior Captain", 1200.0)];

const FILE: &str = "career.txt";

/// A delivery flight on offer: airports (indices into the database), the
/// distance between them and what it pays in points.
#[derive(Copy, Clone)]
pub struct Job {
    pub from: u32,
    pub to: u32,
    pub km: f64,
    pub pay: u32,
}

#[derive(Default)]
pub struct Career {
    pub flights: u32,
    /// seconds flown
    pub secs: f64,
    pub landings: u32,
    pub photos: u32,
    /// best score in each challenge (0: not flown)
    pub best: [u32; CHALLENGES.len()],
    /// delivery jobs flown, and the points they paid
    pub jobs: u32,
    pub earned: u32,
    /// distance flown
    pub km: f64,
    pub crashes: u32,
    /// landings harder than 450 ft/min
    pub hard: u32,
    /// the softest touchdown yet, ft/min (0: none yet)
    pub soft: f64,
}

/// What a touchdown at this sink rate (ft/min) is called.
pub fn grade(fpm: f64) -> &'static str {
    match fpm {
        f if f < 90.0 => "BUTTER",
        f if f < 220.0 => "SMOOTH",
        f if f < 450.0 => "FIRM",
        _ => "HARD",
    }
}

impl Career {
    pub fn load() -> Self {
        let mut c = Self::default();
        for l in std::fs::read_to_string(FILE).unwrap_or_default().lines() {
            let Some((k, v)) = l.split_once(' ') else { continue };
            let n = v.parse::<f64>().unwrap_or(0.0);
            match k {
                "flights" => c.flights = n as u32,
                "secs" => c.secs = n,
                "landings" => c.landings = n as u32,
                "photos" => c.photos = n as u32,
                "jobs" => c.jobs = n as u32,
                "earned" => c.earned = n as u32,
                "km" => c.km = n,
                "crashes" => c.crashes = n as u32,
                "hard" => c.hard = n as u32,
                "soft" => c.soft = n,
                _ => {
                    if let Some(i) = CHALLENGES.iter().position(|x| x.0 == k) {
                        c.best[i] = n as u32;
                    }
                }
            }
        }
        c
    }

    pub fn save(&self) {
        let mut s = format!("flights {}\nsecs {:.0}\nlandings {}\nphotos {}\njobs {}\nearned {}\nkm {:.1}\ncrashes {}\nhard {}\nsoft {:.0}\n", self.flights, self.secs, self.landings, self.photos, self.jobs, self.earned, self.km, self.crashes, self.hard, self.soft);
        for (c, b) in CHALLENGES.iter().zip(self.best) {
            s += &format!("{} {b}\n", c.0);
        }
        if let Err(e) = std::fs::write(FILE, s) {
            log::warn!("could not save {FILE}: {e}");
        }
    }

    /// A point a minute in the air, ten a landing, two a photo, a twentieth
    /// of every challenge score, and whatever the jobs paid.
    pub fn points(&self) -> f64 {
        self.secs / 60.0 + self.landings as f64 * 10.0 + self.photos as f64 * 2.0 + self.best.iter().sum::<u32>() as f64 / 20.0 + self.earned as f64
    }

    /// The rank, how far along to the next (0..1), and the next one's name.
    pub fn rank(&self) -> (&'static str, f32, Option<&'static str>) {
        let p = self.points();
        let i = RANKS.iter().rposition(|r| p >= r.1).unwrap_or(0);
        match RANKS.get(i + 1) {
            Some(n) => (RANKS[i].0, ((p - RANKS[i].1) / (n.1 - RANKS[i].1)) as f32, Some(n.0)),
            None => (RANKS[i].0, 1.0, None),
        }
    }

    /// Name, how to earn it, earned yet.
    pub fn achievements(&self) -> [(&'static str, &'static str, bool); 10] {
        [
            ("First Flight", "Start a flight", self.flights >= 1),
            ("Wheels Down", "Land ten times", self.landings >= 10),
            ("Butter", "Touch down under 90 ft/min", self.soft > 0.0 && self.soft < 90.0),
            ("Courier", "Finish a delivery job", self.jobs >= 1),
            ("Freight Dog", "Finish ten delivery jobs", self.jobs >= 10),
            ("Shutterbug", "Take ten photos", self.photos >= 10),
            ("League Regular", "Fly all five challenges", self.best.iter().all(|b| *b > 0)),
            ("Top Gun", "Score 900 in a challenge", self.best.iter().any(|b| *b >= 900)),
            ("Globetrotter", "Fly 1000 km", self.km >= 1000.0),
            ("Old Hand", "Five hours in the air", self.secs >= 5.0 * 3600.0),
        ]
    }

    /// Three delivery jobs: each from one of the well-known airports to a
    /// field 50 to 350 km away. They change after every flight.
    pub fn offers(&self, apts: &[crate::airports::Apt]) -> Vec<Job> {
        let seed = self.flights as usize;
        let mut out = Vec::new();
        for j in 0..3 {
            let (ident, _) = crate::menu::AIRPORTS[(seed * 7 + j * 5) % crate::menu::AIRPORTS.len()];
            let Some(from) = apts.iter().position(|a| a.ident == ident) else { continue };
            let (la, lo) = (apts[from].lat.to_radians(), apts[from].lon.to_radians());
            let km = |a: &crate::airports::Apt| {
                let (lb, dl) = (a.lat.to_radians(), a.lon.to_radians() - lo);
                (la.sin() * lb.sin() + la.cos() * lb.cos() * dl.cos()).clamp(-1.0, 1.0).acos() * 6371.0
            };
            // Somewhere with a runway worth the trip: medium or large fields first.
            let mut near: Vec<(usize, f64)> = apts.iter().enumerate().filter(|(_, a)| a.kind >= 1 && !a.rwys.is_empty() && (a.lat - apts[from].lat).abs() < 4.0).map(|(i, a)| (i, km(a))).filter(|(_, d)| (50.0..350.0).contains(d)).collect();
            if near.is_empty() {
                near = apts.iter().enumerate().filter(|(_, a)| !a.rwys.is_empty() && (a.lat - apts[from].lat).abs() < 4.0).map(|(i, a)| (i, km(a))).filter(|(_, d)| (20.0..350.0).contains(d)).collect();
            }
            if near.is_empty() {
                continue;
            }
            let (to, d) = near[(seed * 13 + j * 31) % near.len()];
            out.push(Job { from: from as u32, to: to as u32, km: d, pay: 20 + (d * 0.4) as u32 });
        }
        out
    }
}
