//! Typed reads and the workflow collection, available without the server runtime.
use std::cmp::Ordering;

use super::{
    EditorView, ExecutionFilters, ExecutionId, ExecutionNumber, ExecutionView, N8nResource,
    WorkflowName, WorkflowSummary, WorkflowView,
};
use veoveo_hub_sdk::calls::ProviderContract;
use veoveo_types::{Collection, LiveKeyset, PageSize, ServerSlug};

pub struct N8nProvider;
impl ProviderContract for N8nProvider {
    fn slug() -> ServerSlug {
        crate::server()
    }
}

/// The branch's workflows in ascending name order.
pub struct Workflows;
impl Collection for Workflows {
    type Item = WorkflowSummary;
    type Filters = veoveo_hub_sdk::definition::NoFilters;
    type Key = WorkflowName;
    type Limit = PageSize<100>;
    type Policy = LiveKeyset;
    const NAME: &'static str = "n8n.workflows";
    fn key(item: &WorkflowSummary) -> WorkflowName {
        item.workflow.clone()
    }
}

#[derive(Debug, Clone)]
#[veoveo_hub_sdk::collection_resource(provider = N8nProvider, address_type = N8nResource,
    collection = Workflows, template = "n8n://workflows", address = N8nResource::Workflows,
    filters = veoveo_hub_sdk::definition::NoFilters {},
    parse = |address| matches!(address, N8nResource::Workflows).then_some(Self))]
pub struct ReadWorkflows;

#[derive(Debug, Clone)]
#[veoveo_hub_sdk::resource_read(provider = N8nProvider, address_type = N8nResource,
    content = WorkflowView, template = "n8n://workflow/{workflow}", address = N8nResource::Workflow(self.workflow.clone()),
    parse = |address| match address {N8nResource::Workflow(workflow) => Some(Self {workflow: workflow.clone()}), _ => None})]
pub struct ReadWorkflow {
    pub workflow: WorkflowName,
}

#[derive(Debug, Clone)]
#[veoveo_hub_sdk::resource_read(provider = N8nProvider, address_type = N8nResource,
    content = EditorView, address = N8nResource::Editor)]
pub struct ReadEditor;

/// n8n's executions, newest first.
pub struct Executions;
impl Collection for Executions {
    type Item = ExecutionView;
    type Filters = ExecutionFilters;
    type Key = ExecutionNumber;
    type Limit = PageSize<100>;
    type Policy = LiveKeyset;
    const NAME: &'static str = "n8n.executions";
    fn key(item: &ExecutionView) -> ExecutionNumber {
        ExecutionNumber::of(&item.execution).unwrap_or(ExecutionNumber(0))
    }
    fn compare(_: &ExecutionFilters, left: &ExecutionNumber, right: &ExecutionNumber) -> Ordering {
        right.cmp(left)
    }
}

#[derive(Debug, Clone)]
#[veoveo_hub_sdk::collection_resource(provider = N8nProvider, address_type = N8nResource,
    collection = Executions, template = "n8n://executions{?workflow,status}",
    address = N8nResource::Executions { workflow: self.workflow.clone(), status: self.status },
    filters = ExecutionFilters { workflow: self.workflow.clone(), status: self.status },
    parse = |address| match address {
        N8nResource::Executions { workflow, status } => Some(Self { workflow: workflow.clone(), status: *status }),
        _ => None,
    })]
pub struct ReadExecutions {
    pub workflow: Option<WorkflowName>,
    pub status: Option<super::ExecutionStatus>,
}

#[derive(Debug, Clone)]
#[veoveo_hub_sdk::resource_read(provider = N8nProvider, address_type = N8nResource,
    content = ExecutionView, template = "n8n://execution/{execution_id}", address = N8nResource::Execution(self.execution.clone()),
    parse = |address| match address {N8nResource::Execution(execution) => Some(Self {execution: execution.clone()}), _ => None})]
pub struct ReadExecution {
    pub execution: ExecutionId,
}
