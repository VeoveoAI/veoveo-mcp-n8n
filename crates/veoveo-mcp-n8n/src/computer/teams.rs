//! The teams' checkouts in a person's own Computer, `$HOME/n8n/{team}`, and their
//! workflows in the person's own n8n. A team's repository is cloned on demand at the
//! branch its runner tracks, with the Computer's Lore as the person. Its `workflows/` are
//! applied as drafts, with the server's apply, under the n8n tag `veoveo:{team}`, so its
//! triggers run only in its runner. A change saved in the editor is written back to its
//! file in the server's file form by polling n8n's public API; nothing is committed or
//! pushed: that stays with the person and their agent.
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use tokio::sync::Mutex;

use super::{hub::Team, records::TeamRecords};
use crate::{
    contract::WorkflowName,
    n8n::{N8nClient, N8nTag, N8nWorkflowId},
    store::AppliedWorkflow,
    workflows::{self, Activation, AppliedRecords, WorkflowFile},
};

/// How long one Lore clone may take.
const CLONE_TIMEOUT: Duration = Duration::from_secs(600);

/// The teams' checkouts and their workflows in n8n.
pub struct Checkouts {
    /// `$HOME/n8n`.
    root: PathBuf,
    /// Where the records of each team's applied workflows are kept.
    records: PathBuf,
    lore: PathBuf,
    lore_remote: String,
    n8n: N8nClient,
    /// One team operation at a time: an apply never meets a write-back.
    busy: Mutex<()>,
}

/// The n8n tag of `team`'s workflows.
pub fn tag_name(team: &str) -> String {
    format!("veoveo:{team}")
}

impl Checkouts {
    pub fn new(
        root: PathBuf,
        records: PathBuf,
        lore: PathBuf,
        lore_remote: String,
        n8n: N8nClient,
    ) -> Self {
        Self {
            root,
            records,
            lore,
            lore_remote,
            n8n,
            busy: Mutex::new(()),
        }
    }

    /// The team's checkout, `$HOME/n8n/{team}`.
    pub fn checkout_of(&self, team: &str) -> PathBuf {
        self.root.join(team)
    }

    /// Clone the team's repository at its tracked branch, unless its checkout exists.
    pub async fn checkout(&self, team: &Team) -> Result<PathBuf> {
        let directory = self.checkout_of(&team.link);
        if directory.join(".lore").exists() {
            return Ok(directory);
        }
        let repository = team
            .repository()
            .context("the team's runner names no linked repository")?;
        let branch = team
            .status
            .tracked_branch
            .as_deref()
            .context("the team's runner names no tracked branch to check out")?;
        std::fs::create_dir_all(&self.root)
            .with_context(|| format!("creating {}", self.root.display()))?;
        let remote = format!("{}/{repository}", self.lore_remote.trim_end_matches('/'));
        let output = tokio::time::timeout(
            CLONE_TIMEOUT,
            tokio::process::Command::new(&self.lore)
                .arg("clone")
                .arg(&remote)
                .arg(&directory)
                .arg("--branch")
                .arg(branch)
                .arg("--non-interactive")
                .stdin(std::process::Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .context("lore clone did not finish in time")?
        .with_context(|| format!("starting {}", self.lore.display()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "lore clone of {remote} at {branch} exited with {}: {}",
                output.status,
                stderr.trim().chars().take(400).collect::<String>()
            );
        }
        tracing::info!(team = team.link, %repository, %branch, "cloned the team's repository");
        Ok(directory)
    }

    /// Write back what was saved in n8n, then apply the team's files as drafts under its
    /// tag. Returns each workflow's n8n ID.
    pub async fn apply(&self, team: &str) -> Result<Vec<(WorkflowName, N8nWorkflowId)>> {
        let _busy = self.busy.lock().await;
        self.write_back_unlocked(team).await?;
        let directory = self.checkout_of(team);
        let files = workflows::read_directory(&directory.join("workflows"))?;
        let records = TeamRecords::new(&self.records, team);
        let decisions = workflows::apply_to(&self.n8n, &records, &files, Activation::Draft)
            .await
            .with_context(|| format!("applying the team {team}'s workflows"))?;
        let tag = self.n8n.tag(&tag_name(team)).await?;
        let applied = records.recorded().await?;
        for workflow in &applied {
            self.n8n.set_tags(&workflow.n8n_id, &[&tag]).await?;
        }
        tracing::info!(
            team,
            decisions = decisions.len(),
            "applied the team's workflows as drafts"
        );
        Ok(applied
            .into_iter()
            .map(|applied| (applied.workflow, applied.n8n_id))
            .collect())
    }

    /// Write back every team the Computer applied.
    pub async fn write_back_all(&self) -> Result<usize> {
        let mut written = 0;
        let teams = match std::fs::read_dir(&self.records) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(error.into()),
        };
        for entry in teams {
            let path = entry?.path();
            let Some(team) = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_suffix(".json"))
            else {
                continue;
            };
            let _busy = self.busy.lock().await;
            written += self.write_back_unlocked(team).await?;
        }
        Ok(written)
    }

    /// Write each of the team's workflows saved in n8n since it was applied or written
    /// back to its file, and each new workflow carrying the team's tag to a new file
    /// named after it, then record what the files now hold.
    async fn write_back_unlocked(&self, team: &str) -> Result<usize> {
        let records = TeamRecords::new(&self.records, team);
        if !records.exists() {
            return Ok(0);
        }
        let folder = self.checkout_of(team).join("workflows");
        let mut written = 0;
        let recorded = records.recorded().await?;
        for applied in &recorded {
            let Some(found) = self.n8n.workflow(&applied.n8n_id).await? else {
                continue;
            };
            if found.version_id == applied.version || found.is_archived {
                continue;
            }
            self.write_file(&records, &folder, &applied.workflow, &applied.n8n_id)
                .await?;
            written += 1;
        }
        let tag = tag_name(team);
        for tagged in self.n8n.tagged(&tag).await? {
            if recorded
                .iter()
                .any(|applied| applied.n8n_id == tagged.workflow.id)
            {
                continue;
            }
            let Some(workflow) = file_name_of(&tagged.name) else {
                tracing::warn!(
                    team,
                    name = tagged.name,
                    "a workflow of the team has no name a file can carry"
                );
                continue;
            };
            if folder.join(workflow.file_name()).exists() {
                tracing::warn!(
                    team,
                    file = workflow.file_name(),
                    "a new workflow of the team names a file that exists"
                );
                continue;
            }
            self.write_file(&records, &folder, &workflow, &tagged.workflow.id)
                .await?;
            written += 1;
        }
        Ok(written)
    }

    async fn write_file(
        &self,
        records: &TeamRecords,
        folder: &Path,
        workflow: &WorkflowName,
        n8n_id: &N8nWorkflowId,
    ) -> Result<()> {
        let document = self
            .n8n
            .workflow_document(n8n_id)
            .await?
            .with_context(|| format!("n8n no longer holds {workflow}"))?;
        let version = document
            .get("versionId")
            .and_then(serde_json::Value::as_str)
            .and_then(|version| uuid::Uuid::parse_str(version).ok())
            .with_context(|| format!("n8n names no version of {workflow}"))?;
        let mut bytes = serde_json::to_vec_pretty(&workflows::file_members(&document))?;
        bytes.push(b'\n');
        let file = WorkflowFile::parse(workflow.clone(), &bytes)
            .with_context(|| format!("{workflow} as n8n holds it is no workflow file"))?;
        std::fs::create_dir_all(folder)?;
        let path = folder.join(workflow.file_name());
        let staged = folder.join(format!(".{}.new", workflow.file_name()));
        std::fs::write(&staged, &bytes)?;
        std::fs::rename(&staged, &path)?;
        records
            .record(&AppliedWorkflow {
                workflow: workflow.clone(),
                n8n_id: n8n_id.clone(),
                digest: file.digest,
                version,
                runnable: file.runnable,
                applied: serde_json::Value::Null,
            })
            .await?;
        tracing::info!(file = %path.display(), %version, "wrote back a workflow saved in n8n");
        Ok(())
    }

    /// The tag of `team`, for a caller that names it.
    pub async fn tag(&self, team: &str) -> Result<N8nTag> {
        Ok(self.n8n.tag(&tag_name(team)).await?)
    }
}

/// The file name a new workflow called `name` takes: its name in lowercase, with every
/// run of other characters a hyphen.
pub fn file_name_of(name: &str) -> Option<WorkflowName> {
    let mut slug = String::new();
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    WorkflowName::parse(slug.trim_end_matches('-')).ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_new_workflow_takes_a_file_name_from_its_name() {
        let name = |value: &str| super::file_name_of(value).map(|name| name.to_string());
        assert_eq!(
            name("Invoice Intake (v2)").as_deref(),
            Some("invoice-intake-v2")
        );
        assert_eq!(name("hello-run").as_deref(), Some("hello-run"));
        assert_eq!(name("!!!"), None);
        assert_eq!(super::tag_name("marketing"), "veoveo:marketing");
    }
}
