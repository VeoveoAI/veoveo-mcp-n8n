//! Typed reads and the workflow collection, available without the server runtime.
use super::{EditorView, N8nResource, WorkflowName, WorkflowSummary, WorkflowView};
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
