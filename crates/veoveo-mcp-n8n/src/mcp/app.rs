//! The editor's App, `ui://n8n/editor`: a view that reads `n8n://editor` and frames n8n's
//! editor from the server's app origin, which its UI metadata names as the one frame
//! domain. It asks the host for `fullscreen`.
use veoveo_hub_sdk::{
    apps::{AppUri, ResourceUiMeta, UiCsp},
    hosting::{AppBuilder, RoutesBuilder},
    registration::AppOrigin,
};

use super::{N8nMcp, policy};
use crate::contract::N8nPermission;

/// The App's address.
pub const EDITOR_APP_URI: &str = "ui://n8n/editor";
const APP_NAME: &str = "editor";
const APP_TITLE: &str = "n8n";
const APP_DESCRIPTION: &str = "n8n's workflow editor for this branch, acting as n8n's owner. Changes stay in n8n until a workflow's file is pushed; `editor_changes` returns them.";

pub fn editor_app() -> AppUri {
    AppUri::parse(EDITOR_APP_URI).expect("the editor App's address")
}

/// Declare the App, admitted by `n8n.editor` as the app origin is. `editor_origin` is
/// the app origin it frames, once the server registered.
pub(super) fn register(
    routes: RoutesBuilder<N8nMcp>,
    editor_origin: Option<&AppOrigin>,
) -> RoutesBuilder<N8nMcp> {
    let ui = ResourceUiMeta {
        csp: editor_origin.map(|origin| UiCsp {
            frame_domains: vec![origin.as_str().to_owned()],
            ..UiCsp::default()
        }),
        ..ResourceUiMeta::default()
    };
    routes.app(
        AppBuilder::new(editor_app(), APP_NAME)
            .title(APP_TITLE)
            .description(APP_DESCRIPTION)
            .ui(ui)
            .policy(policy(N8nPermission::Editor))
            .reader(|domain: N8nMcp, _| async move { Ok(domain.app.clone()) }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_frames_only_the_app_origin() {
        let origin = AppOrigin::parse("https://n8n.apps.veoveo.example").unwrap();
        let routes = super::super::routes_for(Some(&origin));
        let authorization = routes.authorization();
        assert!(
            serde_json::to_string(&authorization)
                .unwrap()
                .contains(EDITOR_APP_URI)
        );
        assert_eq!(
            super::super::routes_for(None).authorization(),
            authorization,
            "the declaration does not depend on the app origin"
        );
    }
}
