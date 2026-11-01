//! The teams the person reaches, as the Hub lists them to the owner's MCP client: each
//! runner of the n8n repository's links serves `n8n://{link}/status`, which names the
//! team's repository, its tracked branch and whether the person writes it.
use anyhow::{Context, Result};
use rmcp::model::ResourceContents;
use veoveo_hub_sdk::client::HubClient;
use veoveo_types::ResourceUri;

use crate::contract::StatusView;

/// One team: its link and what its runner's status names.
#[derive(Debug, Clone, PartialEq)]
pub struct Team {
    /// The link's name, which names the team's checkout under `$HOME/n8n`.
    pub link: String,
    pub status: StatusView,
}

impl Team {
    /// The team's Lore repository.
    pub fn repository(&self) -> Option<&str> {
        self.status.repository.as_deref()
    }
}

/// The link of `uri` when it is a runner's status, `n8n://{link}/status`.
pub fn status_link(uri: &str) -> Option<&str> {
    let link = uri.strip_prefix("n8n://")?.strip_suffix("/status")?;
    let label = !link.is_empty()
        && link.len() <= 63
        && link
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !link.starts_with('-')
        && !link.ends_with('-');
    label.then_some(link)
}

/// Every team the person reaches now, by link, with its status.
pub async fn teams(hub: &HubClient) -> Result<Vec<Team>> {
    let listed = hub
        .resources()
        .await
        .context("listing the Hub's resources")?;
    let mut teams = Vec::new();
    for resource in &listed.items {
        let Some(link) = status_link(&resource.uri) else {
            continue;
        };
        let uri = ResourceUri::new(resource.uri.clone())
            .with_context(|| format!("{} is no resource URI", resource.uri))?;
        let read = hub
            .read_resource(&uri)
            .await
            .with_context(|| format!("reading {uri}"))?;
        let text = read
            .contents
            .iter()
            .find_map(|contents| match contents {
                ResourceContents::TextResourceContents { text, .. } => Some(text),
                _ => None,
            })
            .with_context(|| format!("{uri} returned no text"))?;
        let status: StatusView =
            serde_json::from_str(text).with_context(|| format!("{uri} is no n8n status"))?;
        teams.push(Team {
            link: link.to_owned(),
            status,
        });
    }
    teams.sort_by(|a, b| a.link.cmp(&b.link));
    Ok(teams)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_runners_status_names_its_link() {
        assert_eq!(
            super::status_link("n8n://marketing/status"),
            Some("marketing")
        );
        for other in [
            "n8n://status",
            "n8n://marketing/workflows",
            "n8n://a/b/status",
            "n8n://-x/status",
            "time://marketing/status",
            "n8n://Marketing/status",
        ] {
            assert_eq!(super::status_link(other), None, "{other}");
        }
    }
}
