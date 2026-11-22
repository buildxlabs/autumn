//! Disk cache for poster images, sharing a size cap with metadata blobs.

use std::{
    fs::{self},
    io::Write,
    path::{Path, PathBuf},
    time::SystemTime,
};

use sha2::{Digest, Sha256};
use tracing::warn;

use crate::{Client, Error, HttpError, Method, Request};

/// Shared cap for `<cache>/posters/` and `<cache>/metadata/`: about 500 MB.
const DEFAULT_CAP_BYTES: u64 = 500 * 1024 * 1024;

/// A disk cache for poster images, keyed by the final image URL.
#[derive(Debug, Clone)]
pub struct PosterCache {
    posters: PathBuf,
    metadata: PathBuf,
    tmp: PathBuf,
    cap: u64,
}

#[derive(Debug)]
struct Entry {
    path: PathBuf,
    len: u64,
    modified: SystemTime,
}

impl PosterCache {
    /// Create the cache under an app cache directory with the default 500 MB cap.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Cache`] if a cache directory cannot be created.
    pub fn new(cache_dir: impl AsRef<Path>) -> Result<Self, Error> {
        Self::with_cap(cache_dir, DEFAULT_CAP_BYTES)
    }

    /// Create the cache with a custom cap in bytes, shared by posters and metadata.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Cache`] if a cache directory cannot be created.
    pub fn with_cap(cache_dir: impl AsRef<Path>, cap: u64) -> Result<Self, Error> {
        let root = cache_dir.as_ref();
        let cache = Self {
            posters: root.join("posters"),
            metadata: root.join("metadata"),
            tmp: root.join("tmp"),
            cap,
        };
        for dir in [&cache.posters, &cache.metadata, &cache.tmp] {
            fs::create_dir_all(dir).map_err(|source| Error::Cache {
                path: dir.clone(),
                source: HttpError::new(source),
            })?;
        }
        Ok(cache)
    }

    /// Fetch a poster, serving a cached copy without a network request.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the download fails or a cache write fails.
    pub async fn get(&self, client: &Client, url: &str) -> Result<Vec<u8>, Error> {
        let name = file_name(url);
        let path = self.posters.join(&name);
        if let Ok(bytes) = fs::read(&path) {
            return Ok(bytes);
        }
        let response = client.read(Request::new(Method::Get, url)).await?;
        let bytes = response.bytes().await?;
        self.store(&name, &bytes)?;
        self.evict()?;
        Ok(bytes)
    }

    /// Delete every cached poster. Metadata blobs are left alone.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Cache`] if a poster cannot be removed.
    pub fn clear(&self) -> Result<(), Error> {
        for entry in entries(&self.posters)? {
            fs::remove_file(&entry.path).map_err(|source| Error::Cache {
                path: entry.path,
                source: HttpError::new(source),
            })?;
        }
        Ok(())
    }

    fn store(&self, name: &str, bytes: &[u8]) -> Result<(), Error> {
        let target = self.posters.join(name);
        let temp = self.tmp.join(format!("{name}.tmp"));
        let written = (|| {
            let mut file = fs::File::create(&temp)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temp, &target)
        })();
        if let Err(error) = written {
            if let Err(cleanup) = fs::remove_file(&temp) {
                warn!(?cleanup, "could not remove an incomplete poster file");
            }
            return Err(Error::Cache {
                path: temp,
                source: HttpError::new(error),
            });
        }
        Ok(())
    }

    fn evict(&self) -> Result<(), Error> {
        let mut all = entries(&self.posters)?;
        all.extend(entries(&self.metadata)?);
        let mut total = all
            .iter()
            .fold(0u64, |sum, entry| sum.saturating_add(entry.len));
        if total <= self.cap {
            return Ok(());
        }
        all.sort_by_key(|entry| entry.modified);
        for entry in all {
            if total <= self.cap {
                break;
            }
            fs::remove_file(&entry.path).map_err(|source| Error::Cache {
                path: entry.path.clone(),
                source: HttpError::new(source),
            })?;
            total = total.saturating_sub(entry.len);
        }
        Ok(())
    }
}

fn file_name(url: &str) -> String {
    use std::fmt::Write;
    Sha256::digest(url.as_bytes())
        .iter()
        .fold(String::with_capacity(64), |mut name, byte| {
            let _ = write!(&mut name, "{byte:02x}");
            name
        })
}

fn entries(dir: &Path) -> Result<Vec<Entry>, Error> {
    let listing = fs::read_dir(dir).map_err(|source| Error::Cache {
        path: dir.to_path_buf(),
        source: HttpError::new(source),
    })?;
    let mut result = Vec::new();
    for item in listing {
        let item = item.map_err(|source| Error::Cache {
            path: dir.to_path_buf(),
            source: HttpError::new(source),
        })?;
        let metadata = item.metadata().map_err(|source| Error::Cache {
            path: item.path(),
            source: HttpError::new(source),
        })?;
        if !metadata.is_file() {
            continue;
        }
        result.push(Entry {
            path: item.path(),
            len: metadata.len(),
            modified: metadata.modified().map_err(|source| Error::Cache {
                path: item.path(),
                source: HttpError::new(source),
            })?,
        });
    }
    Ok(result)
}
