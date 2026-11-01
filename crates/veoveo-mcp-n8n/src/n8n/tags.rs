//! n8n's tags and the workflows that carry one, as a person's own Computer keeps each
//! team's workflows together under a tag of its own.
use reqwest::Method;
use serde::Deserialize;
use serde_json::json;

use super::{N8nClient, N8nError, N8nWorkflow, N8nWorkflowId};

/// The most items one page of n8n's public API returns.
const PAGE: u32 = 250;
/// The most pages a listing reads.
const MAX_PAGES: usize = 40;

/// An n8n tag.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct N8nTag {
    pub id: String,
    pub name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page<T> {
    data: Vec<T>,
    #[serde(default)]
    next_cursor: Option<String>,
}

impl N8nClient {
    /// Every page of `path`, a listing of n8n's public API.
    async fn every<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<Vec<T>, N8nError> {
        let separator = if path.contains('?') { '&' } else { '?' };
        let mut items = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut page_path = format!("{path}{separator}limit={PAGE}");
            if let Some(cursor) = &cursor {
                page_path.push_str("&cursor=");
                page_path.push_str(
                    &url::form_urlencoded::byte_serialize(cursor.as_bytes()).collect::<String>(),
                );
            }
            let page: Page<T> = self.call(Method::GET, &page_path, None).await?;
            items.extend(page.data);
            cursor = page.next_cursor.filter(|cursor| !cursor.is_empty());
            if cursor.is_none() {
                break;
            }
        }
        Ok(items)
    }

    /// The tag named `name`, created when n8n holds none.
    pub async fn tag(&self, name: &str) -> Result<N8nTag, N8nError> {
        let tags: Vec<N8nTag> = self.every("api/v1/tags").await?;
        if let Some(tag) = tags.into_iter().find(|tag| tag.name == name) {
            return Ok(tag);
        }
        self.call(Method::POST, "api/v1/tags", Some(&json!({"name": name})))
            .await
    }

    /// Give workflow `id` the tags `tags`, in place of its own.
    pub async fn set_tags(&self, id: &N8nWorkflowId, tags: &[&N8nTag]) -> Result<(), N8nError> {
        let body = json!(
            tags.iter()
                .map(|tag| json!({"id": tag.id}))
                .collect::<Vec<_>>()
        );
        let _: serde_json::Value = self
            .call(
                Method::PUT,
                &format!("api/v1/workflows/{id}/tags"),
                Some(&body),
            )
            .await?;
        Ok(())
    }

    /// The workflows that carry the tag `name`, archived ones left out.
    pub async fn tagged(&self, name: &str) -> Result<Vec<TaggedWorkflow>, N8nError> {
        let name: String = url::form_urlencoded::byte_serialize(name.as_bytes()).collect();
        let workflows: Vec<TaggedWorkflow> = self
            .every(&format!(
                "api/v1/workflows?tags={name}&excludePinnedData=true"
            ))
            .await?;
        Ok(workflows
            .into_iter()
            .filter(|workflow| !workflow.workflow.is_archived)
            .collect())
    }
}

/// A workflow of a listing, with its name.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TaggedWorkflow {
    #[serde(flatten)]
    pub workflow: N8nWorkflow,
    pub name: String,
}
