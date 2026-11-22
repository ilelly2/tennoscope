use std::{cell::Cell, fs};
use tempfile::tempdir;
use warframe_acquisition::{
    CatalogCache, CatalogCacheError, CatalogFetch, CatalogLoadSource, CatalogSource,
    RELIC_CATALOG_MAX_AGE_SECS, RelicCatalogCache, RelicCatalogSource, WFCD_ALL_JSON_URL,
    WFCD_RELICS_JSON_URL,
};

const VALID: &[u8] = br#"[{"uniqueName":"/Lotus/Powersuits/Test/Test","name":"Test Frame","type":"Warframe","category":"Warframes","masterable":true}]"#;
const VALID_RELICS: &[u8] = br#"[{"uniqueName":"/Lotus/Types/Game/Projections/TestABronze","rewards":[{"item":{"name":"Forma Blueprint"}}]}]"#;
const REFRESHED_RELICS: &[u8] = br#"[{"uniqueName":"/Lotus/Types/Game/Projections/T5VoidProjectionCitrineAPlatinum","rewards":[{"item":{"name":"Citrine Prime Chassis Blueprint"}}]}]"#;

#[test]
fn item_catalog_uses_the_supported_aggregate_endpoint() {
    assert_eq!(WFCD_ALL_JSON_URL, "https://api.warframestat.us/items");
}

#[test]
fn the_relic_catalog_is_followed_from_upstream_not_a_commit() {
    assert_eq!(
        WFCD_RELICS_JSON_URL,
        "https://raw.githubusercontent.com/WFCD/warframe-items/master/data/json/Relics.json"
    );
}

struct Source {
    result: Result<Vec<u8>, CatalogFetch>,
    calls: Cell<usize>,
}
impl CatalogSource for Source {
    fn fetch(&self) -> Result<Vec<u8>, CatalogFetch> {
        self.calls.set(self.calls.get() + 1);
        self.result.clone()
    }
}

impl RelicCatalogSource for Source {
    fn fetch(&self) -> Result<Vec<u8>, CatalogFetch> {
        CatalogSource::fetch(self)
    }
}

#[test]
fn relic_catalog_has_an_independent_atomic_cache_generation() {
    let dir = tempdir().unwrap();
    let cache = RelicCatalogCache::new(dir.path().join("catalog"));
    let loaded = cache
        .load(
            &Source {
                result: Ok(VALID_RELICS.to_vec()),
                calls: Cell::new(0),
            },
            100,
        )
        .unwrap();

    assert_eq!(loaded.source(), CatalogLoadSource::Network);
    assert_eq!(loaded.fetched_unix(), 100);
    assert!(dir.path().join("catalog/relic-generation.json").is_file());

    let stale = cache
        .load(
            &Source {
                result: Err(CatalogFetch::Unavailable),
                calls: Cell::new(0),
            },
            200,
        )
        .unwrap();
    assert_eq!(stale.source(), CatalogLoadSource::StaleCache);
    assert_eq!(stale.fetched_unix(), 100);
}

#[test]
fn valid_download_is_atomically_cached_with_freshness_metadata() {
    let dir = tempdir().unwrap();
    let cache = CatalogCache::new(dir.path().join("catalog"));
    let loaded = cache
        .load(
            &Source {
                result: Ok(VALID.to_vec()),
                calls: Cell::new(0),
            },
            100,
        )
        .unwrap();
    assert_eq!(loaded.source(), CatalogLoadSource::Network);
    assert_eq!(loaded.fetched_unix(), 100);
    assert!(
        loaded
            .index()
            .resolve("/Lotus/Powersuits/Test/Test")
            .is_some()
    );
    assert!(dir.path().join("catalog/catalog-generation.json").is_file());
}

#[test]
fn stale_complete_cache_is_used_when_network_fails() {
    let dir = tempdir().unwrap();
    let cache = CatalogCache::new(dir.path().join("catalog"));
    cache
        .load(
            &Source {
                result: Ok(VALID.to_vec()),
                calls: Cell::new(0),
            },
            100,
        )
        .unwrap();
    let loaded = cache
        .load(
            &Source {
                result: Err(CatalogFetch::Unavailable),
                calls: Cell::new(0),
            },
            200,
        )
        .unwrap();
    assert_eq!(loaded.source(), CatalogLoadSource::StaleCache);
    assert_eq!(loaded.fetched_unix(), 100);
}

#[test]
fn invalid_download_never_replaces_the_last_complete_catalog() {
    let dir = tempdir().unwrap();
    let cache = CatalogCache::new(dir.path().join("catalog"));
    cache
        .load(
            &Source {
                result: Ok(VALID.to_vec()),
                calls: Cell::new(0),
            },
            100,
        )
        .unwrap();
    let loaded = cache
        .load(
            &Source {
                result: Ok(b"not json".to_vec()),
                calls: Cell::new(0),
            },
            200,
        )
        .unwrap();
    assert_eq!(loaded.source(), CatalogLoadSource::StaleCache);
    assert!(
        loaded
            .index()
            .resolve("/Lotus/Powersuits/Test/Test")
            .is_some()
    );
}

#[test]
fn interrupted_temporary_generation_is_never_loaded() {
    let dir = tempdir().unwrap();
    let cache = CatalogCache::new(dir.path().join("catalog"));
    fs::create_dir_all(dir.path().join("catalog")).unwrap();
    fs::write(
        dir.path().join("catalog/catalog-generation.tmp-99"),
        b"partial",
    )
    .unwrap();
    assert!(
        cache
            .load(
                &Source {
                    result: Err(CatalogFetch::Unavailable),
                    calls: Cell::new(0)
                },
                100
            )
            .is_err()
    );
}

#[test]
fn mismatched_content_hash_rejects_the_entire_generation() {
    let dir = tempdir().unwrap();
    let cache = CatalogCache::new(dir.path().join("catalog"));
    cache
        .load(
            &Source {
                result: Ok(VALID.to_vec()),
                calls: Cell::new(0),
            },
            100,
        )
        .unwrap();
    let path = dir.path().join("catalog/catalog-generation.json");
    let mut envelope: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    envelope["fetched_unix"] = 999.into();
    fs::write(path, serde_json::to_vec(&envelope).unwrap()).unwrap();
    assert!(
        cache
            .load(
                &Source {
                    result: Err(CatalogFetch::Unavailable),
                    calls: Cell::new(0)
                },
                200
            )
            .is_err()
    );
}

#[test]
fn concurrent_writers_leave_one_whole_valid_generation() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("catalog");
    std::thread::scope(|scope| {
        for now in [100, 200] {
            let path = path.clone();
            scope.spawn(move || {
                let outcome = CatalogCache::new(path).load(
                    &Source {
                        result: Ok(VALID.to_vec()),
                        calls: Cell::new(0),
                    },
                    now,
                );
                // Either writer may lose the race to replace the generation file. On Windows
                // the replace of a destination being replaced at the same moment fails
                // transiently (MoveFileExW), and the loser reporting CacheWrite is a
                // legitimate outcome, since the invariant under test is that one whole valid
                // generation survives, asserted below. Any other failure is a bug.
                if let Err(error) = outcome
                    && !matches!(error, CatalogCacheError::CacheWrite)
                {
                    panic!("unexpected cache error under concurrent writers: {error}");
                }
            });
        }
    });
    let loaded = CatalogCache::new(path)
        .load(
            &Source {
                result: Err(CatalogFetch::Unavailable),
                calls: Cell::new(0),
            },
            300,
        )
        .unwrap();
    assert!(matches!(loaded.fetched_unix(), 100 | 200));
    assert!(
        loaded
            .index()
            .resolve("/Lotus/Powersuits/Test/Test")
            .is_some()
    );
}

#[test]
fn fails_honestly_without_network_or_a_valid_cache() {
    let dir = tempdir().unwrap();
    let cache = CatalogCache::new(dir.path().join("catalog"));
    assert!(
        cache
            .load(
                &Source {
                    result: Err(CatalogFetch::Unavailable),
                    calls: Cell::new(0)
                },
                100
            )
            .is_err()
    );
}

#[test]
fn the_relic_catalog_fails_honestly_without_network_or_a_valid_cache() {
    let dir = tempdir().unwrap();
    let cache = RelicCatalogCache::new(dir.path().join("catalog"));
    assert!(
        cache
            .load_current(
                &Source {
                    result: Err(CatalogFetch::Unavailable),
                    calls: Cell::new(0)
                },
                100
            )
            .is_err()
    );
}

#[test]
fn a_relic_generation_younger_than_the_max_age_answers_without_a_fetch() {
    let dir = tempdir().unwrap();
    let cache = RelicCatalogCache::new(dir.path().join("catalog"));
    cache
        .load(
            &Source {
                result: Ok(VALID_RELICS.to_vec()),
                calls: Cell::new(0),
            },
            100,
        )
        .unwrap();

    let offline = Source {
        result: Err(CatalogFetch::Unavailable),
        calls: Cell::new(0),
    };
    let loaded = cache
        .load_current(&offline, 100 + RELIC_CATALOG_MAX_AGE_SECS - 1)
        .unwrap();

    // A young generation answered without a fetch is current, not a stale fallback: the monitor
    // logs this source, and `StaleCache` there is a false triage signal.
    assert_eq!(loaded.source(), CatalogLoadSource::FreshCache);
    assert_eq!(loaded.fetched_unix(), 100);
    assert_eq!(offline.calls.get(), 0);
}

/// A stamp ahead of the clock is not evidence of freshness. Clamping it to age zero would make
/// one fetch under a forward-set clock look fresh forever, which is the "stale indefinitely"
/// failure the age check exists to remove.
#[test]
fn a_relic_generation_stamped_past_the_current_clock_is_refetched() {
    let dir = tempdir().unwrap();
    let cache = RelicCatalogCache::new(dir.path().join("catalog"));
    cache
        .load(
            &Source {
                result: Ok(VALID_RELICS.to_vec()),
                calls: Cell::new(0),
            },
            100,
        )
        .unwrap();

    let refreshed = Source {
        result: Ok(REFRESHED_RELICS.to_vec()),
        calls: Cell::new(0),
    };
    let loaded = cache.load_current(&refreshed, 50).unwrap();

    assert_eq!(loaded.source(), CatalogLoadSource::Network);
    assert_eq!(loaded.fetched_unix(), 50);
    assert_eq!(refreshed.calls.get(), 1);
    assert!(
        loaded
            .index()
            .reward_names()
            .any(|name| name == "Citrine Prime Chassis Blueprint")
    );
}

#[test]
fn a_relic_generation_written_by_an_older_schema_is_refetched_while_young() {
    let dir = tempdir().unwrap();
    let cache = RelicCatalogCache::new(dir.path().join("catalog"));
    cache
        .load(
            &Source {
                result: Ok(VALID_RELICS.to_vec()),
                calls: Cell::new(0),
            },
            100,
        )
        .unwrap();

    // A build before the schema version existed wrote the same generation without the key.
    let path = dir.path().join("catalog/relic-generation.json");
    let mut envelope: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    envelope.as_object_mut().unwrap().remove("schema");
    fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();

    let refreshed = Source {
        result: Ok(REFRESHED_RELICS.to_vec()),
        calls: Cell::new(0),
    };
    let loaded = cache.load_current(&refreshed, 150).unwrap();

    assert_eq!(loaded.source(), CatalogLoadSource::Network);
    assert_eq!(loaded.fetched_unix(), 150);
    assert_eq!(refreshed.calls.get(), 1);
    assert!(
        loaded
            .index()
            .reward_names()
            .any(|name| name == "Citrine Prime Chassis Blueprint")
    );

    let rewritten: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        rewritten["schema"], 1,
        "the refetch rewrites the current schema"
    );
    assert_eq!(rewritten["fetched_unix"], 150);
}

#[test]
fn a_failed_cache_write_still_answers_with_the_fetched_table() {
    let dir = tempdir().unwrap();
    // A file where the cache directory should be makes every store attempt fail, which is what a
    // full disk or a read-only app-data directory does.
    let blocked = dir.path().join("catalog");
    fs::write(&blocked, b"").unwrap();
    let cache = RelicCatalogCache::new(&blocked);

    let loaded = cache
        .load(
            &Source {
                result: Ok(VALID_RELICS.to_vec()),
                calls: Cell::new(0),
            },
            100,
        )
        .unwrap();

    assert_eq!(loaded.source(), CatalogLoadSource::Network);
    assert_eq!(loaded.fetched_unix(), 100);
    assert!(
        loaded
            .index()
            .reward_names()
            .any(|name| name == "Forma Blueprint")
    );
    assert!(!dir.path().join("catalog").is_dir());
}

#[test]
fn an_expired_relic_generation_is_refetched() {
    let dir = tempdir().unwrap();
    let cache = RelicCatalogCache::new(dir.path().join("catalog"));
    cache
        .load(
            &Source {
                result: Ok(VALID_RELICS.to_vec()),
                calls: Cell::new(0),
            },
            100,
        )
        .unwrap();

    let now = 100 + RELIC_CATALOG_MAX_AGE_SECS;
    let refreshed = Source {
        result: Ok(br#"[{"uniqueName":"/Lotus/Types/Game/Projections/T5VoidProjectionCitrineAPlatinum","rewards":[{"item":{"name":"Citrine Prime Chassis Blueprint"}}]}]"#.to_vec()),
        calls: Cell::new(0),
    };
    let loaded = cache.load_current(&refreshed, now).unwrap();

    assert_eq!(loaded.source(), CatalogLoadSource::Network);
    assert_eq!(loaded.fetched_unix(), now);
    assert_eq!(refreshed.calls.get(), 1);
    assert!(
        loaded
            .index()
            .reward_names()
            .any(|name| name == "Citrine Prime Chassis Blueprint")
    );
}

#[test]
fn an_expired_generation_still_answers_when_the_refetch_fails() {
    let dir = tempdir().unwrap();
    let cache = RelicCatalogCache::new(dir.path().join("catalog"));
    cache
        .load(
            &Source {
                result: Ok(VALID_RELICS.to_vec()),
                calls: Cell::new(0),
            },
            100,
        )
        .unwrap();

    let loaded = cache
        .load_current(
            &Source {
                result: Err(CatalogFetch::Unavailable),
                calls: Cell::new(0),
            },
            100 + RELIC_CATALOG_MAX_AGE_SECS,
        )
        .unwrap();

    assert_eq!(loaded.source(), CatalogLoadSource::StaleCache);
    assert_eq!(loaded.fetched_unix(), 100);
    assert!(
        loaded
            .index()
            .reward_names()
            .any(|name| name == "Forma Blueprint")
    );
}

#[test]
fn a_relic_payload_with_no_rewards_never_replaces_a_complete_generation() {
    let dir = tempdir().unwrap();
    let cache = RelicCatalogCache::new(dir.path().join("catalog"));
    cache
        .load(
            &Source {
                result: Ok(VALID_RELICS.to_vec()),
                calls: Cell::new(0),
            },
            100,
        )
        .unwrap();

    let loaded = cache
        .load_current(
            &Source {
                result: Ok(
                    br#"[{"uniqueName":"/Lotus/Types/Game/Projections/TestABronze","rewards":[]}]"#
                        .to_vec(),
                ),
                calls: Cell::new(0),
            },
            100 + RELIC_CATALOG_MAX_AGE_SECS,
        )
        .unwrap();

    assert_eq!(loaded.source(), CatalogLoadSource::StaleCache);
    assert_eq!(loaded.fetched_unix(), 100);
    assert!(
        loaded
            .index()
            .reward_names()
            .any(|name| name == "Forma Blueprint")
    );

    let envelope: serde_json::Value = serde_json::from_slice(
        &fs::read(dir.path().join("catalog/relic-generation.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(envelope["fetched_unix"], 100);
}
