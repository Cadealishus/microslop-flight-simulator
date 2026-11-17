//! Disk-cached HTTP fetches for map tiles. (In the browser: through
//! `plat::http_get`, with the browser's own cache in place of the disk.)
use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use std::collections::HashSet;
use std::path::PathBuf;

pub struct Fetcher {
    #[cfg(not(target_arch = "wasm32"))]
    agent: ureq::Agent,
    pub root: PathBuf,
    missing: Mutex<HashSet<String>>,
    pub offline: bool,
}

impl Fetcher {
    pub fn new(root: PathBuf, offline: bool) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(20)))
            .user_agent("fablegame-flightsim/0.1")
            .build()
            .into();
        Self {
            #[cfg(not(target_arch = "wasm32"))]
            agent,
            root,
            missing: Mutex::new(HashSet::new()),
            offline,
        }
    }

    /// Plain GET with no caching.
    pub fn get(&self, url: &str) -> Result<Vec<u8>> {
        if self.offline {
            return Err(anyhow!("offline"));
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut res = self.agent.get(url).call()?;
            Ok(res.body_mut().with_config().limit(64 << 20).read_to_vec()?)
        }
        #[cfg(target_arch = "wasm32")]
        crate::plat::http_get(url)
    }

    /// GET with a disk cache at `<root>/<key>`. Missing tiles (404 etc.) are
    /// remembered for the session so they are not retried every frame.
    pub fn get_cached(&self, key: &str, url: &str) -> Result<Vec<u8>> {
        let path = self.root.join(key);
        #[cfg(not(target_arch = "wasm32"))]
        if let Ok(data) = std::fs::read(&path) {
            if !data.is_empty() {
                return Ok(data);
            }
        }
        if self.missing.lock().contains(key) {
            return Err(anyhow!("missing {key}"));
        }
        match self.get(url) {
            Ok(data) if !data.is_empty() => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    if let Some(dir) = path.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    // Write to a temp name first so a crash never leaves a half tile.
                    let tmp = path.with_extension(format!("tmp{:?}", std::thread::current().id()).replace(['(', ')'], ""));
                    if std::fs::write(&tmp, &data).is_ok() {
                        let _ = std::fs::rename(&tmp, &path);
                    }
                }
                let _ = &path;
                Ok(data)
            }
            Ok(_) => {
                self.missing.lock().insert(key.to_string());
                Err(anyhow!("empty {key}"))
            }
            Err(e) => {
                let s = e.to_string();
                // Only a real "not there" is permanent; timeouts get retried.
                if s.contains("404") || s.contains("403") || s.contains("204") {
                    self.missing.lock().insert(key.to_string());
                }
                Err(e)
            }
        }
    }
}
