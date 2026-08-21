use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use fs2::FileExt;

const MIN_TTL_SECS: u64 = 60;

/// Canonical request descriptor. Requests with different parameters get
/// different cache files, so e.g. Waybar (`--hours 0`) and the Omarchy plugin
/// (`--hours 12 --units imperial`) never cross-serve each other's payloads.
/// The location component is the input as given (named location string, a
/// coords pair, or the auto/IP marker) — building the key never geocodes.
pub struct CacheKey {
    pub location: String,
    pub units: &'static str,
    pub days: u8,
    pub hours: u8,
}

impl CacheKey {
    pub fn canonical(&self) -> String {
        format!(
            "{}|units:{}|days:{}|hours:{}",
            self.location, self.units, self.days, self.hours
        )
    }

    /// 16-hex digest of the canonical descriptor (FNV-1a 64-bit; stable, no
    /// extra dependencies, not security-sensitive).
    pub fn digest(&self) -> String {
        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in self.canonical().as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        format!("{hash:016x}")
    }
}

/// How the returned payload relates to the network: when it was fetched, and
/// whether it is a stale fallback served because a fresh fetch failed.
#[derive(Debug)]
pub struct Freshness {
    pub fetched_at: Option<std::time::SystemTime>,
    pub stale: bool,
    pub stale_reason: Option<&'static str>,
}

pub struct Cache {
    dir: PathBuf,
    file_name: String,
    ttl: Duration,
}

impl Cache {
    /// Returns the modification time of the cache file (= last successful fetch).
    pub fn last_fetched(&self) -> Option<std::time::SystemTime> {
        fs::metadata(self.dir.join(&self.file_name))
            .ok()?
            .modified()
            .ok()
    }

    pub fn new(key: &CacheKey) -> Self {
        let dir = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("meteobar");
        fs::create_dir_all(&dir).ok();
        // Pre-keyed versions used a single shared weather.json; clean it up.
        fs::remove_file(dir.join("weather.json")).ok();
        Self::with_dir(dir, key, Duration::from_secs(MIN_TTL_SECS))
    }

    fn with_dir(dir: PathBuf, key: &CacheKey, ttl: Duration) -> Self {
        Self {
            dir,
            file_name: format!("weather-{}.json", key.digest()),
            ttl,
        }
    }

    /// Try to read fresh cached data. Returns None if cache is missing or stale.
    fn read_fresh(&self) -> Option<String> {
        let path = self.dir.join(&self.file_name);
        let meta = fs::metadata(&path).ok()?;
        let age = meta.modified().ok()?.elapsed().unwrap_or(Duration::MAX);
        if age < self.ttl {
            fs::read_to_string(&path).ok()
        } else {
            None
        }
    }

    /// Read stale cache as fallback (any age).
    fn read_stale(&self) -> Option<String> {
        fs::read_to_string(self.dir.join(&self.file_name)).ok()
    }

    /// Atomically write data to cache.
    fn write(&self, data: &str) {
        let tmp = self.dir.join(format!(".{}.tmp", self.file_name));
        let dest = self.dir.join(&self.file_name);
        if let Ok(mut f) = fs::File::create(&tmp) {
            if f.write_all(data.as_bytes()).is_ok() {
                fs::rename(&tmp, &dest).ok();
            }
        }
    }

    /// Run a fetch function with file-lock serialization and caching.
    /// Only one process fetches a given request at a time; others wait and
    /// read cache. Returns the payload plus its freshness metadata.
    pub fn fetch_or_cached<F>(&self, fetch_fn: F) -> Result<(String, Freshness), String>
    where
        F: FnOnce() -> Result<String, String>,
    {
        let lock_path = self.dir.join(format!(".{}.lock", self.file_name));
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

    fn fetch_inner<F>(&self, fetch_fn: F) -> Result<(String, Freshness), String>
    where
        F: FnOnce() -> Result<String, String>,
    {
        // Check cache inside lock (another instance may have just refreshed it)
        if let Some(cached) = self.read_fresh() {
            return Ok((
                cached,
                Freshness {
                    fetched_at: self.last_fetched(),
                    stale: false,
                    stale_reason: None,
                },
            ));
        }

        match fetch_fn() {
            Ok(data) => {
                self.write(&data);
                Ok((
                    data,
                    Freshness {
                        fetched_at: self.last_fetched().or(Some(std::time::SystemTime::now())),
                        stale: false,
                        stale_reason: None,
                    },
                ))
            }
            Err(e) => {
                // Stale fallback: serve the last payload we have, flagged as
                // such. "fetch_error" — the failure is not classified further.
                if let Some(stale) = self.read_stale() {
                    Ok((
                        stale,
                        Freshness {
                            fetched_at: self.last_fetched(),
                            stale: true,
                            stale_reason: Some("fetch_error"),
                        },
                    ))
                } else {
                    Err(e)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key(location: &str, units: &'static str, days: u8, hours: u8) -> CacheKey {
        CacheKey {
            location: location.to_string(),
            units,
            days,
            hours,
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("meteobar-test-{}-{}", tag, std::process::id()));
        fs::remove_dir_all(&dir).ok();
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn different_params_get_different_cache_files() {
        let base = test_key("loc:Berlin", "metric", 3, 0);
        let same = test_key("loc:Berlin", "metric", 3, 0);
        assert_eq!(base.digest(), same.digest());
        assert_eq!(base.digest().len(), 16);
        assert!(base.digest().chars().all(|c| c.is_ascii_hexdigit()));

        for other in [
            test_key("loc:Berlin", "imperial", 3, 0),
            test_key("loc:Berlin", "metric", 5, 0),
            test_key("loc:Berlin", "metric", 3, 12),
            test_key("loc:Paris", "metric", 3, 0),
            test_key("auto", "metric", 3, 0),
        ] {
            assert_ne!(base.digest(), other.digest());
        }
    }

    #[test]
    fn fresh_fetch_is_not_stale() {
        let dir = temp_dir("fresh");
        let cache = Cache::with_dir(
            dir.clone(),
            &test_key("auto", "metric", 3, 0),
            Duration::ZERO,
        );
        let (data, freshness) = cache.fetch_or_cached(|| Ok("payload-1".into())).unwrap();
        assert_eq!(data, "payload-1");
        assert!(!freshness.stale);
        assert_eq!(freshness.stale_reason, None);
        assert!(freshness.fetched_at.is_some());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn failed_fetch_serves_stale_with_reason() {
        let dir = temp_dir("stale");
        // TTL zero: every read is expired, forcing a re-fetch each call.
        let cache = Cache::with_dir(
            dir.clone(),
            &test_key("auto", "metric", 3, 0),
            Duration::ZERO,
        );
        cache.fetch_or_cached(|| Ok("payload-1".into())).unwrap();

        let (data, freshness) = cache.fetch_or_cached(|| Err("boom".into())).unwrap();
        assert_eq!(data, "payload-1");
        assert!(freshness.stale);
        assert_eq!(freshness.stale_reason, Some("fetch_error"));
        assert!(freshness.fetched_at.is_some());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn failed_fetch_without_cache_is_an_error() {
        let dir = temp_dir("nocache");
        let cache = Cache::with_dir(
            dir.clone(),
            &test_key("auto", "metric", 3, 0),
            Duration::ZERO,
        );
        let err = cache.fetch_or_cached(|| Err("boom".into())).unwrap_err();
        assert_eq!(err, "boom");
        fs::remove_dir_all(&dir).ok();
    }
}
