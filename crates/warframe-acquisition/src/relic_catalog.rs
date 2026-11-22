use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::{CatalogError, CatalogIndex, RewardNeedle};

#[derive(Clone, Debug, Default)]
pub struct RelicRewardIndex {
    rewards: BTreeMap<String, BTreeSet<String>>,
}

impl RelicRewardIndex {
    pub fn from_wfcd_json(bytes: &[u8]) -> Result<Self, CatalogError> {
        let records: Vec<RelicRecord> =
            serde_json::from_slice(bytes).map_err(|_| CatalogError::InvalidJson)?;
        let mut rewards = BTreeMap::new();
        for record in records {
            if !record
                .unique_name
                .starts_with("/Lotus/Types/Game/Projections/")
            {
                continue;
            }
            let names = record
                .rewards
                .into_iter()
                .map(|reward| reward.item.name.trim().to_owned())
                .filter(|name| !name.is_empty())
                .collect::<BTreeSet<_>>();
            // A relic the table lists with no rewards is still a relic the table knows. Dropping
            // it made "the table knows this relic and has no rewards for it" indistinguishable
            // from "the table has never heard of this relic", and the two need opposite
            // handling: the first may fall back to the reward catalogue, the second may not,
            // because there is nothing to constrain the catalogue against.
            rewards.insert(record.unique_name, names);
        }
        Ok(Self { rewards })
    }

    /// Every reward a relic in the index can drop, spelled as the relic tables and the reward
    /// screen spell it: "2X Forma Blueprint", "Lavos Prime Chassis Blueprint". A reward several
    /// relics share comes once per relic.
    pub fn reward_names(&self) -> impl Iterator<Item = &str> {
        self.rewards.values().flatten().map(String::as_str)
    }

    /// Whether the table lists no rewards at all. A table of relics the upstream lists with no
    /// rewards is empty in this sense, and cannot be used to name a card.
    pub fn is_empty(&self) -> bool {
        self.reward_names().next().is_none()
    }

    /// How many relics the table lists, whether or not it lists rewards for them.
    pub fn relic_count(&self) -> usize {
        self.rewards.len()
    }

    /// Whether the table lists this projection path at all, whether or not it lists rewards for
    /// it. A path the table does not list is a relic it has never heard of.
    pub fn knows_projection_path(&self, projection_path: &str) -> bool {
        self.rewards.contains_key(projection_path)
    }

    /// How many of the given projection paths the table lists.
    pub fn known_projection_path_count(&self, projection_paths: &[String]) -> usize {
        projection_paths
            .iter()
            .filter(|path| self.rewards.contains_key(path.as_str()))
            .count()
    }

    pub fn candidates_for_projection_paths(
        &self,
        projection_paths: &[String],
        catalog: &CatalogIndex,
    ) -> Vec<RewardNeedle> {
        projection_paths
            .iter()
            .filter_map(|path| self.rewards.get(path))
            .flatten()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter_map(|name| {
                RewardNeedle::from_paths(name.clone(), reward_catalog_paths(name, catalog)).ok()
            })
            .collect()
    }
}

fn reward_catalog_paths(name: &str, catalog: &CatalogIndex) -> Vec<String> {
    // Relic rewards conventionally add ` Blueprint` to component names. Prefer the component
    // identity even when the catalog also has an exact inventory-recipe alias for that wording.
    if let Some(component_name) = name.strip_suffix(" Blueprint") {
        let paths = catalog.paths_for_name(component_name);
        if !paths.is_empty() {
            return paths;
        }
    }
    let exact = catalog.paths_for_name(name);
    if !exact.is_empty() {
        return exact;
    }

    for alias in [
        crate::catalog::without_quantity(name),
        name.strip_suffix(" Blueprint"),
    ]
    .into_iter()
    .flatten()
    {
        let paths = catalog.paths_for_name(alias);
        if !paths.is_empty() {
            return paths;
        }
        if let Some(component_name) = alias.strip_suffix(" Blueprint") {
            let paths = catalog.paths_for_name(component_name);
            if !paths.is_empty() {
                return paths;
            }
        }
    }
    Vec::new()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RelicRecord {
    unique_name: String,
    #[serde(default)]
    rewards: Vec<RelicReward>,
}

#[derive(Deserialize)]
struct RelicReward {
    item: RelicRewardItem,
}

#[derive(Deserialize)]
struct RelicRewardItem {
    name: String,
}
