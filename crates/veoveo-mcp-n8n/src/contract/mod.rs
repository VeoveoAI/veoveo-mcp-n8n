//! The server's public types. Consumers select `default-features = false,
//! features = ["contract"]`, which builds without a database or runtime dependency.

mod calls;
pub use calls::{
    Executions, N8nProvider, ReadExecution, ReadExecutions, ReadStatus, ReadWorkflow,
    ReadWorkflows, Workflows,
};
mod ids;
mod permissions;
mod resource;
mod runs;
mod workflows;

pub use ids::{ExecutionId, MAX_WORKFLOW_NAME_CHARS, N8nIdError, WorkflowName};
pub use permissions::{N8nPermission, N8nScope};
pub use resource::{N8nDocument, N8nResource, N8nResourceError};
pub use runs::{
    ExecutionAccess, ExecutionFilters, ExecutionNumber, ExecutionStatus, ExecutionView,
    RetryExecutionRequest, RunRequest, RunResult, StopExecutionRequest, UnknownExecutionStatus,
};
pub use workflows::{
    AppliedFile, FileDigest, FileDigestError, StatusView, WebhookView, WorkflowSummary,
    WorkflowView,
};
