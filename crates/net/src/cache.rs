//! Disk cache for poster images, sharing a size cap with metadata blobs.

use std::{
    fmt::Write as _,
    fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use tokio::task::spawn_blocking;
use tracing::warn;

use crate::{Client, Error, HttpError, Method, Request};

/// Shared cap for `<cache>/posters/` and `<cache>/metadata/`: about 500 MB.
const DEFAULT_CAP_BYTES: u64 = 500 * 1024 * 1024;

/// A disk cache for poster images, keyed by the requested image URL.
///
/// The key is the requested URL, not the final URL after redirects: reqwest
/// does not expose the final URL once a response has been consumed.
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
    /// Returns [`Error::Cache`] if a cache directory cannot be created or read.
    pub fn new(cache_dir: impl AsRef<Path>) -> Result<Self, Error> {
        Self::with_cap(cache_dir, DEFAULT_CAP_BYTES)
    }

    /// Create the cache with a custom cap in bytes, shared by posters and metadata.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Cache`] if a cache directory cannot be created or read.
    pub fn with_cap(cache_dir: impl AsRef<Path>, cap: u64) -> Result<Self, Error> {
        let root = cache_dir.as_ref();
        let cache = Self {
            posters: root.join("posters"),
            metadata: root.join("metadata"),
            tmp: root.join("tmp"),
            cap,
        };
        for dir in [&cache.posters, &cache.metadata, &cache.tmp] {
            fs::create_dir_all(dir).map_err(|source| cache_error(dir, source))?;
        }
        // Clear leftovers from interrupted writes.
        for entry in entries(&cache.tmp)? {
            remove_file(&entry.path)?;
        }
        Ok(cache)
    }

    /// Fetch a poster, serving a cached copy without a network request.
    ///
    /// Disk work runs on a blocking pool. If eviction fails after the poster
    /// has been stored, the poster is still returned and the failure is logged.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the download or the cache write fails.
    pub async fn get(&self, client: &Client, url: &str) -> Result<Vec<u8>, Error> {
        let name = file_name(url);
        let read_path = self.posters.join(&name);
        let cached = spawn_blocking(move || fs::read(read_path))
            .await
            .map_err(join_error)?;
        if let Ok(bytes) = cached {
            return Ok(bytes);
        }

        let response = client.read(Request::new(Method::Get, url)).await?;
        let bytes = response.bytes().await?;

        let cache = self.clone();
        let bytes = spawn_blocking(move || {
            cache.store(&name, &bytes)?;
            if let Err(error) = cache.evict() {
                warn!(?error, "poster cache eviction failed");
            }
            Ok::<Vec<u8>, Error>(bytes)
        })
        .await
        .map_err(join_error)??;

        Ok(bytes)
    }

    /// Delete every cached poster. Metadata blobs are left alone.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Cache`] if a poster cannot be removed.
    pub async fn clear(&self) -> Result<(), Error> {
        let cache = self.clone();
        spawn_blocking(move || {
            for entry in entries(&cache.posters)? {
                remove_file(&entry.path)?;
            }
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    fn store(&self, name: &str, bytes: &[u8]) -> Result<(), Error> {
        let mut temp =
            NamedTempFile::new_in(&self.tmp).map_err(|source| cache_error(&self.tmp, source))?;
        temp.write_all(bytes)
            .map_err(|source| cache_error(&self.tmp, source))?;
        temp.as_file()
            .sync_all()
            .map_err(|source| cache_error(&self.tmp, source))?;
        let target = self.posters.join(name);
        temp.persist(&target)
            .map_err(|source| cache_error(&target, source.error))?;
        Ok(())
    }

    fn evict(&self) -> Result<(), Error> {
        let mut files = entries(&self.posters)?;
        files.extend(entries(&self.metadata)?);
        let mut total = files
            .iter()
            .fold(0u64, |sum, entry| sum.saturating_add(entry.len));
        if total <= self.cap {
            return Ok(());
        }
        files.sort_by_key(|entry| entry.modified);
        for entry in files {
            if total <= self.cap {
                break;
            }
            remove_file(&entry.path)?;
            total = total.saturating_sub(entry.len);
        }
        Ok(())
    }
}

fn join_error(source: tokio::task::JoinError) -> Error {
    Error::Task {
        source: HttpError::new(source),
    }
}

fn cache_error(path: &Path, source: impl std::error::Error + Send + Sync + 'static) -> Error {
    Error::Cache {
        path: path.to_path_buf(),
        source: HttpError::new(source),
    }
}

fn remove_file(path: &Path) -> Result<(), Error> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(cache_error(path, error)),
    }
}

fn file_name(url: &str) -> String {
    Sha256::digest(url.as_bytes())
        .iter()
        .fold(String::with_capacity(64), |mut name, byte| {
            let _ = write!(&mut name, "{byte:02x}");
            name
        })
}

fn entries(dir: &Path) -> Result<Vec<Entry>, Error> {
    let listing = fs::read_dir(dir).map_err(|source| cache_error(dir, source))?;
    let mut result = Vec::new();
    for item in listing {
        let item = item.map_err(|source| cache_error(dir, source))?;
        let metadata = item
            .metadata()
            .map_err(|source| cache_error(&item.path(), source))?;
        if !metadata.is_file() {
            continue;
        }
        result.push(Entry {
            path: item.path(),
            len: metadata.len(),
            modified: metadata
                .modified()
                .map_err(|source| cache_error(&item.path(), source))?,
        });
    }
    Ok(result)
}
