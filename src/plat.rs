//! What differs between the desktop and the browser build: background work,
//! downloads, where things are saved, and the clock.
//!
//! On the desktop, background work runs on threads and downloads block them.
//! A browser page has one thread and may not block it, so there:
//! - `task` work runs between frames on the page's thread (`pump`), within a
//!   time budget, most urgent first;
//! - a download (`http_get`) that has not arrived yet starts in the
//!   background and reports "pending"; the task that asked is run again from
//!   the start once something new has arrived, and by then finds it waiting.
//!   So the game's own loading code stays as it is, written as if it blocks.

pub use web_time::{Instant, SystemTime, UNIX_EPOCH};

#[cfg(not(target_arch = "wasm32"))]
pub fn cache_dir() -> Option<std::path::PathBuf> {
    dirs::cache_dir()
}
#[cfg(target_arch = "wasm32")]
pub fn cache_dir() -> Option<std::path::PathBuf> {
    None
}
#[cfg(not(target_arch = "wasm32"))]
pub fn config_dir() -> Option<std::path::PathBuf> {
    dirs::config_dir()
}
#[cfg(target_arch = "wasm32")]
pub fn config_dir() -> Option<std::path::PathBuf> {
    None
}

/// Background work: `work` (run again from the start in the browser while it
/// waits on downloads), then `done` with what it made. `prio`: lower is sooner
/// (only the browser orders them).
#[cfg(not(target_arch = "wasm32"))]
pub fn task<T: Send + 'static>(_prio: f32, mut work: impl FnMut() -> T + Send + 'static, done: impl FnOnce(T) + Send + 'static) {
    std::thread::spawn(move || done(work()));
}

/// Run waiting background work for up to `ms` (the browser; nothing to do on the desktop).
#[cfg(not(target_arch = "wasm32"))]
pub fn pump(_ms: f64) {}

/// Whether a download this work asked for has not arrived yet (only ever in the browser).
#[cfg(not(target_arch = "wasm32"))]
pub fn pending() -> bool {
    false
}

/// A small value kept between visits (the logbook): a file beside the game,
/// or the browser's local storage.
#[cfg(not(target_arch = "wasm32"))]
pub fn load_text(name: &str) -> Option<String> {
    std::fs::read_to_string(name).ok()
}
#[cfg(not(target_arch = "wasm32"))]
pub fn save_text(name: &str, s: &str) -> Result<(), String> {
    std::fs::write(name, s).map_err(|e| e.to_string())
}

/// Work split over threads that all finish before it returns (one after
/// another in the browser).
#[cfg(not(target_arch = "wasm32"))]
pub use std::thread::scope;

#[cfg(target_arch = "wasm32")]
pub fn scope<R>(f: impl FnOnce(&Scope) -> R) -> R {
    f(&Scope)
}
#[cfg(target_arch = "wasm32")]
pub struct Scope;
#[cfg(target_arch = "wasm32")]
pub struct Joined<T>(T);
#[cfg(target_arch = "wasm32")]
impl Scope {
    pub fn spawn<T>(&self, f: impl FnOnce() -> T) -> Joined<T> {
        Joined(f())
    }
}
#[cfg(target_arch = "wasm32")]
impl<T> Joined<T> {
    pub fn join(self) -> Result<T, ()> {
        Ok(self.0)
    }
}

/// The command-line options (in the browser, from the page address:
/// `?airport=KSFO&runway=28R&no-menu` is `--airport KSFO --runway 28R --no-menu`).
#[cfg(not(target_arch = "wasm32"))]
pub fn args() -> Vec<String> {
    std::env::args().skip(1).collect()
}

#[cfg(target_arch = "wasm32")]
pub use web::*;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::{Cell, RefCell};
    use std::collections::{HashMap, VecDeque};
    use std::rc::Rc;
    use wasm_bindgen::JsCast;

    struct Task {
        prio: f32,
        /// the downloads it is waiting on (it runs again once they are all in)
        waiting: Vec<String>,
        tries: u32,
        /// runs the work; true once it is finished
        run: Box<dyn FnMut() -> bool>,
    }

    thread_local! {
        static TASKS: RefCell<Vec<Task>> = const { RefCell::new(Vec::new()) };
        static PENDING: Cell<bool> = const { Cell::new(false) };
        /// the downloads the work running now found missing
        static ASKED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        /// the priority of the work running now (its downloads queue by it)
        static PRIO: Cell<f32> = const { Cell::new(0.0) };
        static NET: RefCell<Net> = RefCell::new(Net::default());
    }

    pub fn task<T: 'static>(prio: f32, mut work: impl FnMut() -> T + 'static, done: impl FnOnce(T) + 'static) {
        let mut done = Some(done);
        let run = Box::new(move || {
            let r = work();
            if PENDING.with(|p| p.replace(false)) {
                return false;
            }
            if let Some(d) = done.take() {
                d(r);
            }
            true
        });
        TASKS.with(|t| t.borrow_mut().push(Task { prio, waiting: Vec::new(), tries: 0, run }));
    }

    pub fn pending() -> bool {
        PENDING.with(|p| p.get())
    }

    thread_local! {
        static STATS: Cell<(f64, u32, u32)> = const { Cell::new((0.0, 0, 0)) };
        /// no background work before then (ms): a long job is followed by as
        /// long a rest, so the frames keep coming while the world loads
        static REST: Cell<f64> = const { Cell::new(0.0) };
    }

    pub fn pump(ms: f64) {
        let t0 = now_ms();
        if t0 < REST.with(|r| r.get()) {
            return;
        }
        let (last, runs, done) = STATS.with(|s| s.get());
        if t0 - last > 5000.0 {
            let (n, w) = TASKS.with(|t| { let t = t.borrow(); (t.len(), t.iter().filter(|k| !k.waiting.is_empty()).count()) });
            let (running, queued, have) = NET.with(|n| { let n = n.borrow(); (n.running, n.waiting.len(), n.got.len()) });
            log::debug!("jobs: {n} queued ({w} waiting on downloads), {runs} runs and {done} finished in 5 s; downloads: {running} running, {queued} queued, {have} known");
            STATS.with(|s| s.set((t0, 0, 0)));
        }
        loop {
            // The most urgent task that can get further now.
            let pick = NET.with(|n| {
                let n = n.borrow();
                TASKS.with(|t| {
                    let mut t = t.borrow_mut();
                    let i = t.iter().enumerate().filter(|(_, k)| !k.waiting.iter().any(|u| matches!(n.got.get(u), Some(Got::Loading)))).min_by(|a, b| a.1.prio.total_cmp(&b.1.prio)).map(|(i, _)| i)?;
                    Some(t.swap_remove(i))
                })
            });
            let Some(mut k) = pick else { break };
            PENDING.with(|p| p.set(false));
            ASKED.with(|a| a.borrow_mut().clear());
            PRIO.with(|p| p.set(k.prio));
            let t1 = now_ms();
            let finished = (k.run)();
            STATS.with(|s| { let (a, b, c) = s.get(); s.set((a, b + 1, c + finished as u32)) });
            let took = now_ms() - t1;
            k.tries += 1;
            if took > 60.0 {
                log::debug!("background job took {took:.0} ms (prio {}, try {})", k.prio, k.tries);
            }
            if !finished {
                k.waiting = ASKED.with(|a| std::mem::take(&mut *a.borrow_mut()));
                TASKS.with(|t| t.borrow_mut().push(k));
            }
            if now_ms() - t0 > ms {
                break;
            }
        }
        let t1 = now_ms();
        if t1 - t0 > ms * 2.0 {
            REST.with(|r| r.set(t1 + (t1 - t0)));
        }
    }

    pub fn args() -> Vec<String> {
        let q = web_sys::window().and_then(|w| w.location().search().ok()).unwrap_or_default();
        let Ok(p) = web_sys::UrlSearchParams::new_with_str(&q) else { return Vec::new() };
        let mut out = Vec::new();
        for e in js_sys::try_iter(&p.entries()).ok().flatten().into_iter().flatten().flatten() {
            let kv = js_sys::Array::from(&e);
            let (k, v) = (kv.get(0).as_string().unwrap_or_default(), kv.get(1).as_string().unwrap_or_default());
            out.push(format!("--{k}"));
            if !v.is_empty() {
                out.push(v);
            }
        }
        out
    }

    pub fn now_ms() -> f64 {
        web_sys::window().and_then(|w| w.performance()).map_or(0.0, |p| p.now())
    }

    // ------------------------------------------------------------ downloads

    enum Got {
        Loading,
        Done(Rc<Vec<u8>>),
        Failed(String),
    }

    #[derive(Default)]
    struct Net {
        got: HashMap<String, Got>,
        /// finished downloads, oldest first, and their bytes (the oldest are let go past a limit)
        order: VecDeque<String>,
        bytes: usize,
        /// how many have arrived (or failed) so far
        arrived: u64,
        running: usize,
        /// how many of those running are background extras (see LOW)
        running_low: usize,
        /// waiting to start: priority, url
        waiting: Vec<(f32, String)>,
    }

    const MAX_RUNNING: usize = 16;
    /// Work this far down the list (the ground photos: several MB each) only
    /// gets a couple of downloads at a time, so it never holds up the terrain.
    const LOW: f32 = 1e5;
    const MAX_LOW: usize = 2;
    const MAX_BYTES: usize = 256 << 20;

    /// GET `url`: its bytes if they are here, else "pending" (and the
    /// download is queued, by the asking work's priority) or why it failed.
    pub fn http_get(url: &str) -> anyhow::Result<Vec<u8>> {
        let here = NET.with(|n| {
            let mut n = n.borrow_mut();
            match n.got.get(url) {
                Some(Got::Done(d)) => return Some(Ok(d.to_vec())),
                Some(Got::Failed(e)) => return Some(Err(anyhow::anyhow!("{e}"))),
                Some(Got::Loading) => return None,
                None => {}
            }
            n.got.insert(url.to_string(), Got::Loading);
            let prio = PRIO.with(|p| p.get());
            n.waiting.push((prio, url.to_string()));
            None
        });
        if let Some(r) = here {
            return r;
        }
        ASKED.with(|a| a.borrow_mut().push(url.to_string()));
        PENDING.with(|p| p.set(true));
        start_next();
        Err(anyhow::anyhow!("pending"))
    }

    /// A download asked for again and again (a live feed): what came back,
    /// once (and it is forgotten, so the next call goes out afresh), or None
    /// while it is on its way (starting it if need be).
    pub fn http_take(url: &str) -> Option<anyhow::Result<Vec<u8>>> {
        let r = NET.with(|n| {
            let mut n = n.borrow_mut();
            match n.got.get(url) {
                Some(Got::Loading) => return None,
                None => {
                    n.got.insert(url.to_string(), Got::Loading);
                    n.waiting.push((0.0, url.to_string()));
                    return None;
                }
                Some(_) => {}
            }
            if let Some(i) = n.order.iter().position(|u| u == url) {
                n.order.remove(i);
            }
            match n.got.remove(url) {
                Some(Got::Done(d)) => {
                    n.bytes -= d.len();
                    Some(Ok(d.to_vec()))
                }
                Some(Got::Failed(e)) => Some(Err(anyhow::anyhow!("{e}"))),
                _ => None,
            }
        });
        start_next();
        r
    }

    /// Starts queued downloads, most urgent first, while there is room.
    fn start_next() {
        loop {
            let next = NET.with(|n| {
                let mut n = n.borrow_mut();
                if n.running >= MAX_RUNNING {
                    return None;
                }
                let low_ok = n.running_low < MAX_LOW;
                let i = n.waiting.iter().enumerate().filter(|(_, w)| low_ok || w.0 < LOW).min_by(|a, b| a.1 .0.total_cmp(&b.1 .0)).map(|(i, _)| i)?;
                let (prio, url) = n.waiting.swap_remove(i);
                let low = prio >= LOW;
                n.running += 1;
                n.running_low += low as usize;
                Some((url, low))
            });
            let Some((url, low)) = next else { return };
            begin(url, low);
        }
    }

    fn begin(url: String, low: bool) {
        wasm_bindgen_futures::spawn_local(async move {
            let r = get(&url).await;
            NET.with(|n| {
                let mut n = n.borrow_mut();
                n.arrived += 1;
                n.running -= 1;
                n.running_low -= low as usize;
                match r {
                    Ok(d) => {
                        n.bytes += d.len();
                        n.order.push_back(url.clone());
                        n.got.insert(url, Got::Done(Rc::new(d)));
                        while n.bytes > MAX_BYTES {
                            let Some(old) = n.order.pop_front() else { break };
                            if let Some(Got::Done(d)) = n.got.remove(&old) {
                                n.bytes -= d.len();
                            }
                        }
                    }
                    Err(e) => {
                        n.got.insert(url, Got::Failed(e));
                    }
                }
            });
            start_next();
        });
    }

    async fn get(url: &str) -> Result<Vec<u8>, String> {
        let w = web_sys::window().ok_or("no window")?;
        let res = wasm_bindgen_futures::JsFuture::from(w.fetch_with_str(url)).await.map_err(|e| format!("network: {e:?}"))?;
        let res: web_sys::Response = res.dyn_into().map_err(|_| "bad response")?;
        if !res.ok() {
            return Err(format!("HTTP {}", res.status()));
        }
        if res.status() == 204 {
            return Err("HTTP 204".into());
        }
        let buf = wasm_bindgen_futures::JsFuture::from(res.array_buffer().map_err(|e| format!("{e:?}"))?).await.map_err(|e| format!("{e:?}"))?;
        Ok(js_sys::Uint8Array::new(&buf).to_vec())
    }

    // ------------------------------------------------------------ storage

    fn storage() -> Option<web_sys::Storage> {
        web_sys::window()?.local_storage().ok()?
    }
    pub fn load_text(name: &str) -> Option<String> {
        storage()?.get_item(name).ok()?
    }
    pub fn save_text(name: &str, s: &str) -> Result<(), String> {
        storage().ok_or("no local storage")?.set_item(name, s).map_err(|e| format!("{e:?}"))
    }
}
