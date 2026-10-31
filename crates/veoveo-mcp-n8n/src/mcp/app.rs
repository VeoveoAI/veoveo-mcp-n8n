//! The editor's App, `ui://n8n/editor`: a self-contained launcher. It reads `n8n://editor`,
//! shows the editor's URL and opens the server's app origin top-level with MCP Apps'
//! `ui/open-link` when the person asks. It frames nothing: an App runs in a frame
//! sandboxed `allow-scripts` alone, which a nested frame inherits, so a framed app origin
//! would hold no session of its own.
use veoveo_hub_sdk::{
    apps::AppUri,
    hosting::{AppBuilder, RoutesBuilder},
};

use super::{N8nMcp, policy};
use crate::contract::N8nPermission;

/// The App's address.
pub const EDITOR_APP_URI: &str = "ui://n8n/editor";
const APP_NAME: &str = "editor";
const APP_TITLE: &str = "n8n";
const APP_DESCRIPTION: &str = "Opens n8n's workflow editor for this branch, which acts as n8n's owner. Changes stay in n8n until a workflow's file is pushed; `editor_changes` returns them.";

pub fn editor_app() -> AppUri {
    AppUri::parse(EDITOR_APP_URI).expect("the editor App's address")
}

/// Declare the App, admitted by `n8n.editor` as the app origin is. Its UI metadata
/// declares no remote origin: the view only reads its own server's resource.
pub(super) fn register(routes: RoutesBuilder<N8nMcp>) -> RoutesBuilder<N8nMcp> {
    routes.app(
        AppBuilder::new(editor_app(), APP_NAME)
            .title(APP_TITLE)
            .description(APP_DESCRIPTION)
            .policy(policy(N8nPermission::Editor))
            .reader(|domain: N8nMcp, _| async move { Ok(domain.app.clone()) }),
    )
}
