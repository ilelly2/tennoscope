use app_lib::{RewardObservation, RewardObserverState};

#[test]
fn observer_debounces_show_and_hide_to_prevent_overlay_flicker() {
    let choices = vec![
        RewardObservation::certain("A"),
        RewardObservation::certain("B"),
        RewardObservation::certain("C"),
        RewardObservation::certain("D"),
    ];
    let mut state = RewardObserverState::new(2, 2);

    assert!(!state.observe(choices.clone()).show);
    let shown = state.observe(choices);
    assert!(shown.show);
    assert_eq!(shown.choices.len(), 4);
    assert!(!state.miss().hide);
    assert!(state.miss().hide);
}

#[test]
fn observer_accepts_three_rewards_when_one_player_did_not_crack_a_relic() {
    let mut state = RewardObserverState::new(1, 1);
    let transition = state.observe(vec![
        RewardObservation::certain("Braton Prime Barrel"),
        RewardObservation::certain("Forma Blueprint"),
        RewardObservation::certain("Paris Prime String"),
    ]);

    assert!(transition.show);
    assert_eq!(transition.choices.len(), 3);
}

#[test]
fn observer_republishes_a_confirmed_set_even_when_it_thinks_overlay_is_visible() {
    let choices = vec![
        RewardObservation::certain("A"),
        RewardObservation::certain("B"),
        RewardObservation::certain("C"),
        RewardObservation::certain("D"),
    ];
    let mut state = RewardObserverState::new(1, 1);

    let first = state.observe(choices.clone());
    let repeated = state.observe(choices);

    assert!(first.publish);
    assert!(repeated.publish);
    assert!(!repeated.show);
}
