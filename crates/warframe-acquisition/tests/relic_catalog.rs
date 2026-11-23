use warframe_acquisition::{CatalogIndex, RelicRewardIndex};

#[test]
fn loaded_projection_paths_resolve_to_deduplicated_reward_needles() {
    let relics = RelicRewardIndex::from_wfcd_json(
        br#"[
          {"uniqueName":"/Lotus/Types/Game/Projections/TestABronze","name":"Lith A1 Intact","rewards":[
            {"item":{"name":"Perigale Prime Receiver"}},
            {"item":{"name":"Forma Blueprint"}}
          ]},
          {"uniqueName":"/Lotus/Types/Game/Projections/TestBBronze","name":"Lith B1 Intact","rewards":[
            {"item":{"name":"Perigale Prime Receiver"}},
            {"item":{"name":"Burston Prime Receiver"}}
          ]}
        ]"#,
    )
    .unwrap();
    let catalog = CatalogIndex::from_wfcd_json(
        br#"[
          {"uniqueName":"/Lotus/Types/Recipes/PerigalePrimeReceiver","name":"Perigale Prime Receiver","type":"PrimePart","category":"PrimeParts"},
          {"uniqueName":"/Lotus/Types/Recipes/BurstonPrimeReceiver","name":"Burston Prime Receiver","type":"PrimePart","category":"PrimeParts"},
          {"uniqueName":"/Lotus/Types/Recipes/FormaBlueprint","name":"Forma Blueprint","type":"Blueprint","category":"Blueprints"}
        ]"#,
    )
    .unwrap();

    let candidates = relics.candidates_for_projection_paths(
        &[
            "/Lotus/Types/Game/Projections/TestABronze".into(),
            "/Lotus/Types/Game/Projections/TestBBronze".into(),
        ],
        &catalog,
    );

    assert_eq!(
        candidates
            .iter()
            .map(|candidate| candidate.choice_name())
            .collect::<Vec<_>>(),
        vec![
            "Burston Prime Receiver",
            "Forma Blueprint",
            "Perigale Prime Receiver"
        ]
    );
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.internal_paths().len() == 1)
    );
}

#[test]
fn relic_paths_absent_from_the_table_are_reported() {
    let relics = RelicRewardIndex::from_wfcd_json(
        br#"[{
          "uniqueName":"/Lotus/Types/Game/Projections/TestABronze",
          "rewards":[{"item":{"name":"Forma Blueprint"}}]
        }]"#,
    )
    .unwrap();
    let catalog = CatalogIndex::from_wfcd_json(
        br#"[{
          "uniqueName":"/Lotus/Types/Recipes/Components/FormaBlueprint",
          "name":"Forma Blueprint",
          "type":"Blueprint",
          "category":"Blueprints"
        }]"#,
    )
    .unwrap();
    let asked = [
        "/Lotus/Types/Game/Projections/TestABronze".to_owned(),
        "/Lotus/Types/Game/Projections/T5VoidProjectionCitrineAPlatinum".to_owned(),
    ];

    assert_eq!(
        relics.known_projection_path_count(&asked),
        1,
        "only the path the table lists is known"
    );
    let candidates = relics.candidates_for_projection_paths(&asked, &catalog);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].choice_name(), "Forma Blueprint");
}

#[test]
fn unrelated_projection_paths_produce_no_candidates() {
    let relics = RelicRewardIndex::from_wfcd_json(br#"[]"#).unwrap();
    let catalog = CatalogIndex::default();

    assert!(
        relics
            .candidates_for_projection_paths(&["/Lotus/Unknown".into()], &catalog)
            .is_empty()
    );
}

/// A relic the table lists with no rewards is a relic the table knows. Dropping it made the
/// two cases indistinguishable: "the table knows this relic and has no rewards for it" and "the
/// table has never heard of this relic" both left the pool empty, and only the second one means
/// the table is too old to name anything.
#[test]
fn a_relic_the_table_lists_without_rewards_is_still_a_relic_the_table_knows() {
    let relics = RelicRewardIndex::from_wfcd_json(
        br#"[{
          "uniqueName":"/Lotus/Types/Game/Projections/TestARewardless",
          "rewards":[]
        }]"#,
    )
    .unwrap();
    let known = "/Lotus/Types/Game/Projections/TestARewardless".to_owned();
    let unknown = "/Lotus/Types/Game/Projections/T5VoidProjectionCitrineAPlatinum".to_owned();

    assert!(
        relics.knows_projection_path(&known),
        "a relic the table lists with no rewards is still known"
    );
    assert!(
        !relics.knows_projection_path(&unknown),
        "a relic the table never listed is not known"
    );
    assert_eq!(
        relics.known_projection_path_count(&[known.clone(), unknown.clone()]),
        1
    );
    assert_eq!(
        relics.relic_count(),
        1,
        "the rewardless relic is counted as a relic"
    );
    assert!(
        relics.is_empty(),
        "a table with no rewards at all cannot name a card"
    );
}

#[test]
fn relic_blueprint_names_resolve_catalog_component_names_without_the_suffix() {
    let relics = RelicRewardIndex::from_wfcd_json(
        br#"[{
          "uniqueName":"/Lotus/Types/Game/Projections/WukongTestPlatinum",
          "rewards":[{"item":{"name":"Wukong Prime Neuroptics Blueprint"}}]
        }]"#,
    )
    .unwrap();
    let catalog = CatalogIndex::from_wfcd_json(
        br#"[{
          "uniqueName":"/Lotus/Powersuits/MonkeyKing/WukongPrime",
          "name":"Wukong Prime",
          "components":[{
            "uniqueName":"/Lotus/Types/Recipes/WarframeRecipes/WukongPrimeHelmetComponent",
            "name":"Neuroptics",
            "tradable":true,
            "ducats":15
          }]
        }]"#,
    )
    .unwrap();

    let candidates = relics.candidates_for_projection_paths(
        &["/Lotus/Types/Game/Projections/WukongTestPlatinum".into()],
        &catalog,
    );

    assert_eq!(candidates.len(), 1);
    assert_eq!(
        candidates[0].choice_name(),
        "Wukong Prime Neuroptics Blueprint"
    );
    assert_eq!(
        candidates[0].internal_paths(),
        [b"/Lotus/Types/Recipes/WarframeRecipes/WukongPrimeHelmetComponent".as_slice()]
    );
}

#[test]
fn relic_quantity_prefix_resolves_the_underlying_catalog_item() {
    let relics = RelicRewardIndex::from_wfcd_json(
        br#"[{
          "uniqueName":"/Lotus/Types/Game/Projections/FormaTestPlatinum",
          "rewards":[{"item":{"name":"2X Forma Blueprint"}}]
        }]"#,
    )
    .unwrap();
    let catalog = CatalogIndex::from_wfcd_json(
        br#"[{
          "uniqueName":"/Lotus/Types/Recipes/Components/FormaBlueprint",
          "name":"Forma Blueprint",
          "type":"Blueprint",
          "category":"Blueprints"
        }]"#,
    )
    .unwrap();

    let candidates = relics.candidates_for_projection_paths(
        &["/Lotus/Types/Game/Projections/FormaTestPlatinum".into()],
        &catalog,
    );

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].choice_name(), "2X Forma Blueprint");
    assert_eq!(
        candidates[0].internal_paths(),
        [b"/Lotus/Types/Recipes/Components/FormaBlueprint".as_slice()]
    );
}
