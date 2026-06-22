use std::collections::BTreeMap;

use crate::storage::{StoredFlowEdge, StoredFlowStep};

pub(crate) trait StepScheduler {
    fn order(&self, ready: Vec<StoredFlowStep>, edges: &[StoredFlowEdge]) -> Vec<StoredFlowStep>;
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RuleBasedStepScheduler;

impl StepScheduler for RuleBasedStepScheduler {
    fn order(
        &self,
        mut ready: Vec<StoredFlowStep>,
        edges: &[StoredFlowEdge],
    ) -> Vec<StoredFlowStep> {
        let declaration_order = ready
            .iter()
            .enumerate()
            .map(|(index, step)| (step.id.clone(), index))
            .collect::<BTreeMap<_, _>>();
        let downstream_counts = ready
            .iter()
            .map(|step| (step.id.clone(), downstream_unblock_count(step, edges)))
            .collect::<BTreeMap<_, _>>();

        ready.sort_by(|left, right| {
            downstream_counts[&right.id]
                .cmp(&downstream_counts[&left.id])
                .then_with(|| declaration_order[&left.id].cmp(&declaration_order[&right.id]))
        });
        ready
    }
}

fn downstream_unblock_count(step: &StoredFlowStep, edges: &[StoredFlowEdge]) -> usize {
    // P2.1a intentionally uses direct successors as a structural unblock approximation.
    edges
        .iter()
        .filter(|edge| edge.from_step_id == step.id)
        .count()
}
