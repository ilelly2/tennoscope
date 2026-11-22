use std::{
    fs,
    io::{Read, Write},
    path::PathBuf,
    time::Duration,
};

use atomicwrites::{AtomicFile, OverwriteBehavior};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{CatalogIndex, RelicRewardIndex};

pub const RELIC_CATALOG_MAX_AGE_SECS: u64 = 24 * 60 * 60;
/// Version of the stored relic generation's format. A generation stamped with any other version
/// is refetched regardless of its age, so a cache written by an older build can never be served
/// as current after an upgrade.
pub const RELIC_CATALOG_SCHEMA: u32 = 1;
pub const WFCD_ALL_JSON_URL: &str = "https://api.warframestat.us/items";
pub const WFCD_RELICS_JSON_URL: &str =
    "https://raw.githubusercontent.com/WFCD/warframe-items/master/data/json/Relics.json";
// Raised from 64 MiB: the WFCD items catalog passed 64 MiB in October 2026.
const MAX_CATALOG_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CatalogFetch {
    Unavailable,
    TooLarge,
}
pub trait CatalogSource {
    fn fetch(&self) -> Result<Vec<u8>, CatalogFetch>;
}

pub trait RelicCatalogSource {
    fn fetch(&self) -> Result<Vec<u8>, CatalogFetch>;
}

pub struct WfcdCatalogHttp {
    client: Client,
}
impl WfcdCatalogHttp {
    pub fn new() -> Result<Self, CatalogFetch> {
        Ok(Self {
            client: Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(|_| CatalogFetch::Unavailable)?,
        })
    }
}
impl CatalogSource for WfcdCatalogHttp {
    fn fetch(&self) -> Result<Vec<u8>, CatalogFetch> {
        let response = self
            .client
            .get(WFCD_ALL_JSON_URL)
            .send()
            .map_err(|_| CatalogFetch::Unavailable)?;
        if !response.status().is_success() {
            return Err(CatalogFetch::Unavailable);
        }
        read_bounded(response)
    }
}

pub struct WfcdRelicCatalogHttp {
    client: Client,
}

impl WfcdRelicCatalogHttp {
    pub fn new() -> Result<Self, CatalogFetch> {
        Ok(Self {
            client: Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(|_| CatalogFetch::Unavailable)?,
        })
    }
}

impl RelicCatalogSource for WfcdRelicCatalogHttp {
    fn fetch(&self) -> Result<Vec<u8>, CatalogFetch> {
        fetch_bounded(&self.client, WFCD_RELICS_JSON_URL)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CatalogLoadSource {
    Network,
    /// A stored generation answered without a fetch and is inside the max age. Only
    /// [`RelicCatalogCache::load_current`] reports this; [`CatalogCache::load`] has no age check,
    /// so every cached item catalog is [`Self::StaleCache`].
    FreshCache,
    StaleCache,
}
pub struct CatalogLoad {
    index: CatalogIndex,
    source: CatalogLoadSource,
    fetched_unix: u64,
}
impl CatalogLoad {
    pub const fn index(&self) -> &CatalogIndex {
        &self.index
    }
    pub const fn source(&self) -> CatalogLoadSource {
        self.source
    }
    pub const fn fetched_unix(&self) -> u64 {
        self.fetched_unix
    }
}

#[derive(Debug, Error)]
pub enum CatalogCacheError {
    #[error("no valid Warframe catalog is available")]
    Unavailable,
    #[error("catalog cache could not be updated")]
    CacheWrite,
}

#[derive(Serialize, Deserialize)]
struct Generation {
    /// Format version of the envelope. `serde(default)` reads a file written before the field
    /// existed as version `0`, which no build ever wrote, so it is refetched rather than trusted.
    #[serde(default)]
    schema: u32,
    fetched_unix: u64,
    catalog: serde_json::Value,
    content_hash: String,
}

/// Only the retrieval time and the envelope version. Deserialising [`Generation`] builds a
/// `serde_json::Value` of the whole relic table, which a freshness check must not pay on every
/// launch.
#[derive(Deserialize)]
struct GenerationStamp {
    #[serde(default)]
    schema: u32,
    fetched_unix: u64,
}

pub struct RelicCatalogLoad {
    index: RelicRewardIndex,
    source: CatalogLoadSource,
    fetched_unix: u64,
}

impl RelicCatalogLoad {
    pub const fn index(&self) -> &RelicRewardIndex {
        &self.index
    }

    pub const fn source(&self) -> CatalogLoadSource {
        self.source
    }

    pub const fn fetched_unix(&self) -> u64 {
        self.fetched_unix
    }

    pub const fn age_secs(&self, now_unix: u64) -> u64 {
        now_unix.saturating_sub(self.fetched_unix)
    }
}

pub struct CatalogCache {
    directory: PathBuf,
}
impl CatalogCache {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }
    pub fn load(
        &self,
        source: &dyn CatalogSource,
        now_unix: u64,
    ) -> Result<CatalogLoad, CatalogCacheError> {
        if let Ok(bytes) = source.fetch() {
            if let (Ok(index), Ok(catalog)) = (
                CatalogIndex::from_wfcd_json(&bytes),
                serde_json::from_slice(&bytes),
            ) {
                self.store(catalog, now_unix)?;
                return Ok(CatalogLoad {
                    index,
                    source: CatalogLoadSource::Network,
                    fetched_unix: now_unix,
                });
            }
        }
        self.load_cached()
    }

    fn store(
        &self,
        catalog: serde_json::Value,
        fetched_unix: u64,
    ) -> Result<(), CatalogCacheError> {
        fs::create_dir_all(&self.directory).map_err(|_| CatalogCacheError::CacheWrite)?;
        let content_hash = generation_hash(fetched_unix, &catalog)?;
        // The item catalog validates by content hash alone and has no format version of its own.
        let bytes = serde_json::to_vec(&Generation {
            schema: 0,
            fetched_unix,
            catalog,
            content_hash,
        })
        .map_err(|_| CatalogCacheError::CacheWrite)?;
        let final_path = self.directory.join("catalog-generation.json");
        AtomicFile::new(final_path, OverwriteBehavior::AllowOverwrite)
            .write(|file| file.write_all(&bytes).and_then(|_| file.sync_all()))
            .map_err(|_| CatalogCacheError::CacheWrite)
    }

    pub fn load_cached(&self) -> Result<CatalogLoad, CatalogCacheError> {
        let bytes = fs::read(self.directory.join("catalog-generation.json"))
            .map_err(|_| CatalogCacheError::Unavailable)?;
        let generation: Generation =
            serde_json::from_slice(&bytes).map_err(|_| CatalogCacheError::Unavailable)?;
        if generation.content_hash
            != generation_hash(generation.fetched_unix, &generation.catalog)
                .map_err(|_| CatalogCacheError::Unavailable)?
        {
            return Err(CatalogCacheError::Unavailable);
        }
        let catalog_bytes =
            serde_json::to_vec(&generation.catalog).map_err(|_| CatalogCacheError::Unavailable)?;
        let index = CatalogIndex::from_wfcd_json(&catalog_bytes)
            .map_err(|_| CatalogCacheError::Unavailable)?;
        Ok(CatalogLoad {
            index,
            source: CatalogLoadSource::StaleCache,
            fetched_unix: generation.fetched_unix,
        })
    }
}

pub struct RelicCatalogCache {
    directory: PathBuf,
}

impl RelicCatalogCache {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    /// Answers from the stored generation while it was written by this build's schema and is
    /// younger than `RELIC_CATALOG_MAX_AGE_SECS`, and otherwise refetches through [`Self::load`],
    /// which keeps answering from the stale generation if the network is unreachable.
    pub fn load_current(
        &self,
        source: &dyn RelicCatalogSource,
        now_unix: u64,
    ) -> Result<RelicCatalogLoad, CatalogCacheError> {
        if let Some(stamp) = self.cached_stamp()
            && stamp.schema == RELIC_CATALOG_SCHEMA
            && now_unix
                .checked_sub(stamp.fetched_unix)
                .is_some_and(|age| age < RELIC_CATALOG_MAX_AGE_SECS)
            && let Ok(cached) = self.load_cached()
        {
            return Ok(RelicCatalogLoad {
                index: cached.index,
                source: CatalogLoadSource::FreshCache,
                fetched_unix: cached.fetched_unix,
            });
        }
        self.load(source, now_unix)
    }

    /// The stored generation's envelope version and retrieval time, read without building the
    /// reward index so a freshness check stays cheap on a table of a few thousand relics.
    fn cached_stamp(&self) -> Option<GenerationStamp> {
        let bytes = fs::read(self.directory.join("relic-generation.json")).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    pub fn load(
        &self,
        source: &dyn RelicCatalogSource,
        now_unix: u64,
    ) -> Result<RelicCatalogLoad, CatalogCacheError> {
        if let Ok(bytes) = source.fetch() {
            if let (Ok(index), Ok(catalog)) = (
                RelicRewardIndex::from_wfcd_json(&bytes),
                serde_json::from_slice(&bytes),
            ) && !index.is_empty()
            {
                // A failed write costs the next launch a refetch; it must not cost this session
                // the table that was just validated.
                if let Err(error) = self.store(catalog, now_unix) {
                    log::warn!("relic catalog: cache write failed: {error}");
                }
                return Ok(RelicCatalogLoad {
                    index,
                    source: CatalogLoadSource::Network,
                    fetched_unix: now_unix,
                });
            }
        }
        self.load_cached()
    }

    fn store(
        &self,
        catalog: serde_json::Value,
        fetched_unix: u64,
    ) -> Result<(), CatalogCacheError> {
        fs::create_dir_all(&self.directory).map_err(|_| CatalogCacheError::CacheWrite)?;
        let content_hash = generation_hash(fetched_unix, &catalog)?;
        let bytes = serde_json::to_vec(&Generation {
            schema: RELIC_CATALOG_SCHEMA,
            fetched_unix,
            catalog,
            content_hash,
        })
        .map_err(|_| CatalogCacheError::CacheWrite)?;
        AtomicFile::new(
            self.directory.join("relic-generation.json"),
            OverwriteBehavior::AllowOverwrite,
        )
        .write(|file| file.write_all(&bytes).and_then(|_| file.sync_all()))
        .map_err(|_| CatalogCacheError::CacheWrite)
    }

    pub fn load_cached(&self) -> Result<RelicCatalogLoad, CatalogCacheError> {
        let bytes = fs::read(self.directory.join("relic-generation.json"))
            .map_err(|_| CatalogCacheError::Unavailable)?;
        let generation: Generation =
            serde_json::from_slice(&bytes).map_err(|_| CatalogCacheError::Unavailable)?;
        if generation.content_hash
            != generation_hash(generation.fetched_unix, &generation.catalog)
                .map_err(|_| CatalogCacheError::Unavailable)?
        {
            return Err(CatalogCacheError::Unavailable);
        }
        let catalog_bytes =
            serde_json::to_vec(&generation.catalog).map_err(|_| CatalogCacheError::Unavailable)?;
        let index = RelicRewardIndex::from_wfcd_json(&catalog_bytes)
            .map_err(|_| CatalogCacheError::Unavailable)?;
        Ok(RelicCatalogLoad {
            index,
            source: CatalogLoadSource::StaleCache,
            fetched_unix: generation.fetched_unix,
        })
    }
}

fn fetch_bounded(client: &Client, url: &str) -> Result<Vec<u8>, CatalogFetch> {
    let response = client
        .get(url)
        .send()
        .map_err(|_| CatalogFetch::Unavailable)?;
    if !response.status().is_success() {
        return Err(CatalogFetch::Unavailable);
    }
    read_bounded(response)
}

/// Read a response body up to `MAX_CATALOG_BYTES`, refusing anything longer.
///
/// Reads one byte past the limit rather than the limit itself so an oversize body is told apart
/// from one that is exactly the limit, and it is read through `Take` so an oversize body never
/// costs more than the limit plus one byte of memory before it is refused.
fn read_bounded(body: impl Read) -> Result<Vec<u8>, CatalogFetch> {
    let mut bytes = Vec::new();
    body.take((MAX_CATALOG_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| CatalogFetch::Unavailable)?;
    if bytes.len() > MAX_CATALOG_BYTES {
        return Err(CatalogFetch::TooLarge);
    }
    Ok(bytes)
}

fn generation_hash(
    fetched_unix: u64,
    catalog: &serde_json::Value,
) -> Result<String, CatalogCacheError> {
    let mut digest = Sha256::new();
    digest.update(fetched_unix.to_le_bytes());
    digest.update(serde_json::to_vec(catalog).map_err(|_| CatalogCacheError::CacheWrite)?);
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::{CatalogFetch, MAX_CATALOG_BYTES, read_bounded};

    /// `https://api.warframestat.us/items` as measured on 2026-10-10: 17,672 entries. It passed
    /// the old 64 MiB limit (67,108,864) at some point before then, after which every fetch was
    /// refused as `TooLarge` and a fresh install had no catalog at all.
    const LIVE_WFCD_ITEMS_BYTES: usize = 68_640_011;

    fn body(length: usize) -> impl Read {
        std::io::repeat(b' ').take(length as u64)
    }

    #[test]
    fn the_live_wfcd_items_catalog_is_within_the_limit() {
        let bytes =
            read_bounded(body(LIVE_WFCD_ITEMS_BYTES)).expect("the live catalog size is accepted");
        assert_eq!(bytes.len(), LIVE_WFCD_ITEMS_BYTES);
    }

    #[test]
    fn a_body_at_the_limit_is_accepted_and_one_byte_over_is_refused() {
        assert_eq!(
            read_bounded(body(MAX_CATALOG_BYTES)).map(|bytes| bytes.len()),
            Ok(MAX_CATALOG_BYTES)
        );
        assert_eq!(
            read_bounded(body(MAX_CATALOG_BYTES + 1)).map(|bytes| bytes.len()),
            Err(CatalogFetch::TooLarge)
        );
    }
}
