use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use fs2::FileExt;

const MIN_TTL_SECS: u64 = 60;

pub struct Cache {
    dir: PathBuf,
    ttl: Duration,
}

impl Cache {
    /// Returns the modification time of the cache file (= last successful fetch).
    pub fn last_fetched(&self) -> Option<std::time::SystemTime> {
        std::fs::metadata(self.dir.join("weather.json"))
            .ok()?
            .modified()
            .ok()
    }

    pub fn new() -> Self {
        let dir = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("meteobar");
        fs::create_dir_all(&dir).ok();
        Self {
            dir,
            ttl: Duration::from_secs(MIN_TTL_SECS),
        }
    }

    /// Try to read fresh cached data. Returns None if cache is missing or stale.
    fn read_fresh(&self) -> Option<String> {
        let path = self.dir.join("weather.json");
        let meta = fs::metadata(&path).ok()?;
        let age = meta
            .modified()
            .ok()?
            .elapsed()
            .unwrap_or(Duration::MAX);
        if age < self.ttl {
            fs::read_to_string(&path).ok()
        } else {
            None
        }
    }

    /// Read stale cache as fallback (any age).
    fn read_stale(&self) -> Option<String> {
        fs::read_to_string(self.dir.join("weather.json")).ok()
    }

    /// Atomically write data to cache.
    fn write(&self, data: &str) {
        let tmp = self.dir.join(".weather.tmp");
        let dest = self.dir.join("weather.json");
        if let Ok(mut f) = fs::File::create(&tmp) {
            if f.write_all(data.as_bytes()).is_ok() {
                fs::rename(&tmp, &dest).ok();
            }
        }
    }

    /// Run a fetch function with file-lock serialization and caching.
    /// Only one process fetches at a time; others wait and read cache.
    pub fn fetch_or_cached<F>(&self, fetch_fn: F) -> Result<String, String>
    where
        F: FnOnce() -> Result<String, String>,
    {
        let lock_path = self.dir.join(".fetch.lock");
        let lock_file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|e| format!("lock open failed: {e}"))?;

        lock_file
            .lock_exclusive()
            .map_err(|e| format!("lock failed: {e}"))?;

        let result = self.fetch_inner(fetch_fn);

        lock_file.unlock().ok();
        result
    }

    fn fetch_inner<F>(&self, fetch_fn: F) -> Result<String, String>
    where
        F: FnOnce() -> Result<String, String>,
    {
        // Check cache inside lock (another instance may have just refreshed it)
        if let Some(cached) = self.read_fresh() {
            return Ok(cached);
        }

        match fetch_fn() {
            Ok(data) => {
                self.write(&data);
                Ok(data)
            }
            Err(e) => {
                // Stale fallback
                if let Some(stale) = self.read_stale() {
                    Ok(stale)
                } else {
                    Err(e)
                }
            }
        }
    }
}
