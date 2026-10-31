//! Registration uses the ready route table directly.
use super::setup::N8nContract;
use std::sync::LazyLock;
use veoveo_hub_sdk::{registration::AuthorizationDeclaration, server_contract::McpServerContract};
pub static AUTHORIZATION: LazyLock<AuthorizationDeclaration> = LazyLock::new(|| {
    let declaration = super::ROUTES.authorization();
    declaration
        .check(&crate::server(), &N8nContract::scheme())
        .expect("n8n route authorization");
    declaration
});

#[cfg(test)]
mod tests {
    use crate::contract::N8nPermission;
    use veoveo_hub_sdk::registration::RouteAccess;

    #[test]
    fn the_editor_permission_admits_a_person_to_the_app_origin() {
        let authorization = &*super::AUTHORIZATION;
        assert_eq!(
            authorization.requirements.app_origin,
            Some(RouteAccess::Permission(N8nPermission::Editor.name()))
        );
        for permission in N8nPermission::ALL {
            assert!(
                authorization.permission(&permission.name()).is_some(),
                "{}",
                permission.as_str()
            );
        }
        let uses: Vec<_> = authorization
            .uses
            .iter()
            .map(|use_| use_.as_str())
            .collect();
        assert_eq!(uses, ["time.authorities.read"]);
    }
}
