//! What a start does to n8n for each workflow, decided from the team's files, what the
//! server recorded at its last apply and what n8n holds now. The decision is pure, so it
//! is tested apart from n8n.
use std::collections::BTreeMap;

use crate::{
    contract::{FileDigest, WorkflowName},
    n8n::{N8nWorkflow, N8nWorkflowId},
};

/// What the server recorded for a workflow at its last apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub n8n_id: N8nWorkflowId,
    pub digest: FileDigest,
    pub version: uuid::Uuid,
}

/// What a start does for one workflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// n8n holds no workflow for the file: create it from the file and publish it.
    Create { workflow: WorkflowName },
    /// Replace n8n's copy with the file and publish it. `replaced` is a version someone
    /// saved in n8n since the last apply, which n8n's history keeps; `unarchive` holds
    /// when someone archived the workflow in n8n.
    Update {
        workflow: WorkflowName,
        n8n_id: N8nWorkflowId,
        replaced: Option<uuid::Uuid>,
        unarchive: bool,
    },
    /// The file and n8n's version are the ones applied, but n8n has it unpublished.
    Publish {
        workflow: WorkflowName,
        n8n_id: N8nWorkflowId,
    },
    /// The file and n8n's version are the ones applied and published.
    Keep {
        workflow: WorkflowName,
        n8n_id: N8nWorkflowId,
    },
    /// The file left the team's files: archive n8n's copy and forget it.
    Archive {
        workflow: WorkflowName,
        n8n_id: N8nWorkflowId,
    },
    /// The file left the team's files and n8n's copy is gone or archived: forget it.
    Forget { workflow: WorkflowName },
}

impl Decision {
    pub fn workflow(&self) -> &WorkflowName {
        match self {
            Self::Create { workflow }
            | Self::Update { workflow, .. }
            | Self::Publish { workflow, .. }
            | Self::Keep { workflow, .. }
            | Self::Archive { workflow, .. }
            | Self::Forget { workflow } => workflow,
        }
    }
}

/// The decisions for `files`, each workflow and the digest of its file, given what the
/// server `recorded` and what n8n holds for each recorded workflow, `None` when n8n has
/// no such workflow. Files come first in name order, then the workflows whose files are
/// gone.
pub fn plan(
    files: &[(WorkflowName, FileDigest)],
    recorded: &BTreeMap<WorkflowName, Recorded>,
    current: &BTreeMap<WorkflowName, Option<N8nWorkflow>>,
) -> Vec<Decision> {
    let in_n8n = |workflow: &WorkflowName| current.get(workflow).and_then(Option::as_ref);
    let mut decisions: Vec<Decision> = files
        .iter()
        .map(|(workflow, digest)| {
            let workflow = workflow.clone();
            let (Some(recorded), Some(found)) = (recorded.get(&workflow), in_n8n(&workflow)) else {
                return Decision::Create { workflow };
            };
            let edited = (found.version_id != recorded.version).then_some(found.version_id);
            let n8n_id = recorded.n8n_id.clone();
            if found.is_archived || edited.is_some() || recorded.digest != *digest {
                Decision::Update {
                    workflow,
                    n8n_id,
                    replaced: edited.filter(|_| !found.is_archived),
                    unarchive: found.is_archived,
                }
            } else if found.is_active() {
                Decision::Keep { workflow, n8n_id }
            } else {
                Decision::Publish { workflow, n8n_id }
            }
        })
        .collect();
    decisions.extend(
        recorded
            .iter()
            .filter(|(workflow, _)| !files.iter().any(|(file, _)| file == *workflow))
            .map(|(workflow, recorded)| match in_n8n(workflow) {
                Some(found) if !found.is_archived => Decision::Archive {
                    workflow: workflow.clone(),
                    n8n_id: recorded.n8n_id.clone(),
                },
                _ => Decision::Forget {
                    workflow: workflow.clone(),
                },
            }),
    );
    decisions
}

#[cfg(test)]
mod tests {
    use super::*;

    const V1: uuid::Uuid = uuid::Uuid::from_u128(1);
    const V2: uuid::Uuid = uuid::Uuid::from_u128(2);

    fn name(value: &str) -> WorkflowName {
        WorkflowName::parse(value).unwrap()
    }
    fn id(value: &str) -> N8nWorkflowId {
        N8nWorkflowId::parse(value).unwrap()
    }
    fn digest(byte: u8) -> FileDigest {
        FileDigest::from_bytes([byte; 32])
    }
    fn recorded(n8n: &str, file: u8, version: uuid::Uuid) -> Recorded {
        Recorded {
            n8n_id: id(n8n),
            digest: digest(file),
            version,
        }
    }
    fn found(n8n: &str, version: uuid::Uuid, active: bool, archived: bool) -> Option<N8nWorkflow> {
        Some(N8nWorkflow {
            id: id(n8n),
            version_id: version,
            active_version_id: active.then_some(version),
            is_archived: archived,
        })
    }

    #[test]
    fn a_new_file_is_created() {
        let decisions = plan(
            &[(name("hello"), digest(1))],
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert_eq!(
            decisions,
            [Decision::Create {
                workflow: name("hello")
            }]
        );
    }

    #[test]
    fn an_unchanged_workflow_is_kept_or_published() {
        let files = [(name("hello"), digest(1))];
        let recorded = BTreeMap::from([(name("hello"), recorded("Wf1", 1, V1))]);
        let active = BTreeMap::from([(name("hello"), found("Wf1", V1, true, false))]);
        assert_eq!(
            plan(&files, &recorded, &active),
            [Decision::Keep {
                workflow: name("hello"),
                n8n_id: id("Wf1")
            }]
        );
        let unpublished = BTreeMap::from([(name("hello"), found("Wf1", V1, false, false))]);
        assert_eq!(
            plan(&files, &recorded, &unpublished),
            [Decision::Publish {
                workflow: name("hello"),
                n8n_id: id("Wf1")
            }]
        );
    }

    #[test]
    fn a_changed_file_updates_and_a_version_saved_in_n8n_is_replaced() {
        let recorded = BTreeMap::from([(name("hello"), recorded("Wf1", 1, V1))]);
        let current = BTreeMap::from([(name("hello"), found("Wf1", V1, true, false))]);
        assert_eq!(
            plan(&[(name("hello"), digest(2))], &recorded, &current),
            [Decision::Update {
                workflow: name("hello"),
                n8n_id: id("Wf1"),
                replaced: None,
                unarchive: false
            }]
        );
        // A version was saved in n8n since: the file's copy replaces it.
        let edited = BTreeMap::from([(name("hello"), found("Wf1", V2, true, false))]);
        assert_eq!(
            plan(&[(name("hello"), digest(1))], &recorded, &edited),
            [Decision::Update {
                workflow: name("hello"),
                n8n_id: id("Wf1"),
                replaced: Some(V2),
                unarchive: false
            }]
        );
        // Someone archived it in n8n: the file restores it.
        let archived = BTreeMap::from([(name("hello"), found("Wf1", V2, false, true))]);
        assert_eq!(
            plan(&[(name("hello"), digest(1))], &recorded, &archived),
            [Decision::Update {
                workflow: name("hello"),
                n8n_id: id("Wf1"),
                replaced: None,
                unarchive: true
            }]
        );
    }

    #[test]
    fn a_workflow_missing_from_n8n_is_created_again() {
        let recorded = BTreeMap::from([(name("hello"), recorded("Wf1", 1, V1))]);
        let current = BTreeMap::from([(name("hello"), None)]);
        assert_eq!(
            plan(&[(name("hello"), digest(1))], &recorded, &current),
            [Decision::Create {
                workflow: name("hello")
            }]
        );
    }

    #[test]
    fn a_removed_file_is_archived_then_forgotten() {
        let recorded = BTreeMap::from([
            (name("gone"), recorded("Wf1", 1, V1)),
            (name("kept"), recorded("Wf2", 2, V1)),
            (name("lost"), recorded("Wf3", 3, V1)),
            (name("shelved"), recorded("Wf4", 4, V1)),
        ]);
        let current = BTreeMap::from([
            (name("gone"), found("Wf1", V1, true, false)),
            (name("kept"), found("Wf2", V1, true, false)),
            (name("lost"), None),
            (name("shelved"), found("Wf4", V2, false, true)),
        ]);
        assert_eq!(
            plan(&[(name("kept"), digest(2))], &recorded, &current),
            [
                Decision::Keep {
                    workflow: name("kept"),
                    n8n_id: id("Wf2")
                },
                Decision::Archive {
                    workflow: name("gone"),
                    n8n_id: id("Wf1")
                },
                Decision::Forget {
                    workflow: name("lost")
                },
                Decision::Forget {
                    workflow: name("shelved")
                },
            ]
        );
    }
}
