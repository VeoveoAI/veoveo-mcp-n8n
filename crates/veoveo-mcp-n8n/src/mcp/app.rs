//! The overview App, `ui://n8n/overview`: a self-contained view of every team the person
//! reaches. The runners of the n8n repository's links share it; it reads each link's
//! resources at `n8n://{link}/…` and runs a workflow with `{link}__run`. It frames
//! nothing and declares no remote origin. Editing opens the person's own Computer with
//! `ui/open-link`.
use veoveo_hub_sdk::{
    apps::{AppUri, UiVisibility},
    hosting::{AppBuilder, RoutesBuilder},
};

use super::{N8nMcp, policy};
use crate::contract::N8nPermission;

/// The App's address.
pub const OVERVIEW_APP_URI: &str = "ui://n8n/overview";
const APP_NAME: &str = "overview";
const APP_TITLE: &str = "n8n";
const APP_DESCRIPTION: &str = "The teams' n8n workflows: what each runs, what needs attention, recent executions, a run form, and editing in your own Computer.";

pub fn overview_app() -> AppUri {
    AppUri::parse(OVERVIEW_APP_URI).expect("the overview App's address")
}

/// Declare the App, admitted as the workflows are, and link `run` to it for the model
/// and the App: the view's run form starts a run as an MCP Task.
pub(super) fn register(routes: RoutesBuilder<N8nMcp>) -> RoutesBuilder<N8nMcp> {
    let app = overview_app();
    routes
        .app(
            AppBuilder::new(app.clone(), APP_NAME)
                .title(APP_TITLE)
                .description(APP_DESCRIPTION)
                .policy(policy(N8nPermission::WorkflowsRead))
                .reader(|domain: N8nMcp, _| async move { Ok(domain.app.clone()) }),
        )
        .app_tools(&app, &["run"], &[UiVisibility::Model, UiVisibility::App])
}
