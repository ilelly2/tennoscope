use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RewardObservation {
    pub name: String,
    pub confidence: f32,
}

impl RewardObservation {
    pub fn certain(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            confidence: 1.0,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ObserverTransition {
    pub publish: bool,
    pub show: bool,
    pub hide: bool,
    pub choices: Vec<RewardObservation>,
}

pub struct RewardObserverState {
    hits_required: u8,
    misses_required: u8,
    hits: u8,
    misses: u8,
    visible: bool,
    candidate: Vec<RewardObservation>,
}

impl RewardObserverState {
    pub fn new(hits_required: u8, misses_required: u8) -> Self {
        Self {
            hits_required: hits_required.max(1),
            misses_required: misses_required.max(1),
            hits: 0,
            misses: 0,
            visible: false,
            candidate: Vec::new(),
        }
    }

    pub fn observe(&mut self, choices: Vec<RewardObservation>) -> ObserverTransition {
        if !(2..=4).contains(&choices.len()) {
            return self.miss();
        }
        self.misses = 0;
        if same_choices(&self.candidate, &choices) {
            self.hits = self.hits.saturating_add(1);
        } else {
            self.candidate = choices.clone();
            self.hits = 1;
        }
        let show = !self.visible && self.hits >= self.hits_required;
        if show {
            self.visible = true;
        }
        ObserverTransition {
            publish: self.hits >= self.hits_required,
            show,
            hide: false,
            choices,
        }
    }

    pub fn miss(&mut self) -> ObserverTransition {
        self.hits = 0;
        self.candidate.clear();
        self.misses = self.misses.saturating_add(1);
        let hide = self.visible && self.misses >= self.misses_required;
        if hide {
            self.visible = false;
        }
        ObserverTransition {
            publish: false,
            show: false,
            hide,
            choices: Vec::new(),
        }
    }
}

fn same_choices(left: &[RewardObservation], right: &[RewardObservation]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| left.name == right.name)
}
