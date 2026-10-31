//! The credentials `n8n/instance.toml` maps, as the plaintext document
//! `n8n import:credentials` reads: each secret field holds the placeholder its provider
//! sets in the environment, and n8n encrypts the document's values on import.
use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};

use crate::instance::{CredentialMapping, FieldPath};

/// The import document for `credentials`, with each secret field's value read by
/// `placeholder` from the provider's environment variable.
pub fn import_document(
    credentials: &[CredentialMapping],
    placeholder: impl Fn(&str) -> Option<String>,
) -> Result<Value> {
    let mut document = Vec::with_capacity(credentials.len());
    for credential in credentials {
        let mut data = match serde_json::to_value(&credential.data)
            .context("a credential's data converts to JSON")?
        {
            Value::Object(data) => data,
            _ => bail!("credential {}'s data is not a table", credential.id),
        };
        for (field, variable) in &credential.secrets {
            let value = placeholder(variable.as_str()).with_context(|| {
                format!(
                    "credential {} needs {variable}, which no attached provider sets",
                    credential.id
                )
            })?;
            insert(&mut data, field, value)
                .with_context(|| format!("credential {} field {field}", credential.id))?;
        }
        document.push(serde_json::json!({
            "id": credential.id.as_str(),
            "name": credential.name,
            "type": credential.credential_type.as_str(),
            "data": data,
        }));
    }
    Ok(Value::Array(document))
}

fn insert(data: &mut Map<String, Value>, field: &FieldPath, value: String) -> Result<()> {
    let (last, parents) = field
        .keys()
        .split_last()
        .context("a field has at least one key")?;
    let mut object = data;
    for key in parents {
        object = match object
            .entry(key.clone())
            .or_insert_with(|| Value::Object(Map::new()))
        {
            Value::Object(object) => object,
            _ => bail!("{key} is not an object"),
        };
    }
    object.insert(last.clone(), Value::String(value));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::Instance;

    #[test]
    fn secret_fields_hold_the_providers_placeholders() {
        let instance = Instance::parse(
            r#"
            tenant = "reference"
            nodes = ["n8n-nodes-base.httpRequest"]
            [[credentials]]
            id = "slackOauth00001"
            name = "Slack"
            type = "slackOAuth2Api"
            data = { grantType = "authorizationCode", oauthTokenData = { token_type = "Bearer" } }
            secrets = { "oauthTokenData.access_token" = "SLACK_ACCESS_TOKEN", clientSecret = "SLACK_CLIENT_SECRET" }
            "#,
        )
        .unwrap();
        let document = import_document(&instance.credentials, |name| {
            Some(format!("openshell:resolve:env:{name}"))
        })
        .unwrap();
        assert_eq!(
            document,
            serde_json::json!([{
                "id": "slackOauth00001",
                "name": "Slack",
                "type": "slackOAuth2Api",
                "data": {
                    "grantType": "authorizationCode",
                    "clientSecret": "openshell:resolve:env:SLACK_CLIENT_SECRET",
                    "oauthTokenData": {
                        "token_type": "Bearer",
                        "access_token": "openshell:resolve:env:SLACK_ACCESS_TOKEN"
                    }
                }
            }])
        );
        let missing = import_document(&instance.credentials, |_| None).unwrap_err();
        assert!(
            format!("{missing:#}").contains("no attached provider sets"),
            "{missing:#}"
        );
        assert_eq!(
            import_document(&[], |_| None).unwrap(),
            serde_json::json!([])
        );
    }
}
