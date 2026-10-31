//! A run's outcome, from the Veoveo Result node's callback or from n8n's execution, and
//! the status check and cancellation the Task runtime calls on n8n.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use veoveo_task::{CheckObservation, Operation, OperationCalls, OperationEventId};

use crate::{
    contract::{ExecutionId, ExecutionStatus},
    n8n::{N8nClient, N8nError, N8nExecution},
};

/// How a run's execution ended, the payload of its operation's final event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "outcome",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum RunOutcome {
    Succeeded {
        execution: ExecutionId,
        output: Vec<Value>,
    },
    Failed {
        execution: ExecutionId,
        status: ExecutionStatus,
        message: String,
    },
    /// Someone stopped the execution in n8n.
    Stopped { execution: ExecutionId },
}

impl RunOutcome {
    /// The outcome n8n's execution shows, once it ended.
    pub fn of(execution: &N8nExecution) -> Option<Self> {
        let id = execution.id.clone();
        match execution.status {
            ExecutionStatus::Success => Some(Self::Succeeded {
                execution: id,
                output: execution.output().unwrap_or_default(),
            }),
            ExecutionStatus::Error | ExecutionStatus::Crashed => Some(Self::Failed {
                execution: id,
                status: execution.status,
                message: execution
                    .error()
                    .unwrap_or_else(|| format!("the execution {}", execution.status)),
            }),
            ExecutionStatus::Canceled => Some(Self::Stopped { execution: id }),
            ExecutionStatus::New
            | ExecutionStatus::Running
            | ExecutionStatus::Waiting
            | ExecutionStatus::Unknown => None,
        }
    }
}

/// The status check and cancellation of a run's operation, through n8n's public API.
pub struct N8nOperations {
    n8n: N8nClient,
}

impl N8nOperations {
    pub fn new(n8n: N8nClient) -> Self {
        Self { n8n }
    }
}

fn execution_of(operation: &Operation) -> Result<ExecutionId, anyhow::Error> {
    let id = operation
        .external_id
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("the run's operation recorded no execution"))?;
    Ok(ExecutionId::parse(id.as_str())?)
}

impl OperationCalls for N8nOperations {
    type Error = anyhow::Error;

    async fn check(&self, operation: &Operation) -> Result<CheckObservation, Self::Error> {
        let execution = execution_of(operation)?;
        let Some(found) = self.n8n.execution(&execution, true).await? else {
            let outcome = RunOutcome::Failed {
                execution: execution.clone(),
                status: ExecutionStatus::Unknown,
                message: "n8n no longer holds the execution".to_owned(),
            };
            return Ok(CheckObservation {
                event_id: Some(OperationEventId::parse(format!(
                    "check-missing-{execution}"
                ))?),
                payload: serde_json::to_value(outcome)?,
                is_final: true,
            });
        };
        Ok(match RunOutcome::of(&found) {
            Some(outcome) => CheckObservation {
                event_id: Some(OperationEventId::parse(format!(
                    "check-{}-{execution}",
                    found.status
                ))?),
                payload: serde_json::to_value(outcome)?,
                is_final: true,
            },
            None => CheckObservation {
                event_id: None,
                payload: json!({"status": found.status}),
                is_final: false,
            },
        })
    }

    async fn cancel(&self, operation: &Operation) -> Result<Value, Self::Error> {
        let execution = execution_of(operation)?;
        match self.n8n.stop_execution(&execution).await {
            Ok(stopped) => Ok(json!({"execution": execution, "stopped": stopped})),
            // n8n refuses to stop an execution that already ended or that it no longer holds.
            Err(N8nError::Refused {
                status, message, ..
            }) if status.is_client_error() => Ok(json!({
                "execution": execution,
                "refused": {"status": status.as_u16(), "message": message},
            })),
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_execution_that_ended_is_an_outcome() {
        let execution = |status: &str| -> N8nExecution {
            serde_json::from_value(json!({"id": "5", "status": status, "mode": "webhook"})).unwrap()
        };
        assert_eq!(
            RunOutcome::of(&execution("success")),
            Some(RunOutcome::Succeeded {
                execution: ExecutionId::parse("5").unwrap(),
                output: vec![]
            })
        );
        assert!(matches!(
            RunOutcome::of(&execution("crashed")),
            Some(RunOutcome::Failed {
                status: ExecutionStatus::Crashed,
                ..
            })
        ));
        assert!(matches!(
            RunOutcome::of(&execution("canceled")),
            Some(RunOutcome::Stopped { .. })
        ));
        for running in ["new", "running", "waiting", "unknown"] {
            assert_eq!(RunOutcome::of(&execution(running)), None, "{running}");
        }
        let wire = serde_json::to_value(RunOutcome::Stopped {
            execution: ExecutionId::parse("5").unwrap(),
        })
        .unwrap();
        assert_eq!(wire, json!({"outcome": "stopped", "execution": "5"}));
    }
}
