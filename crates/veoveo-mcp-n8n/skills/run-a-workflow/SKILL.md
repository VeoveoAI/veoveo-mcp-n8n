---
name: run-a-workflow
description: Find a team's n8n workflows, run one that starts with the Veoveo Trigger as a Task, read its execution, and retry or stop executions. Use when a person asks to run an automation or check how one went.
---

# Run a workflow

Each team has its own n8n, its runner's, reached through the Hub by the team's link: its
tools are `n8n__{link}__{tool}` and its resources `n8n://{link}/…`, where `{link}` names
the team. The overview App, `ui://n8n/overview`, shows every team the person reaches.

## Find it

- `n8n://{link}/workflows` pages the team's workflows: name, whether it is runnable and
  active, and the applied file's digest. A workflow is runnable when it starts with the
  Veoveo Trigger.
- `n8n://{link}/workflow/{workflow}` holds its JSON as applied, including the Veoveo
  Trigger's `inputSchema`, and the public URL of each of its webhooks.
- `n8n://{link}/status` says what the runner runs: n8n's version, when it applied its
  workflows, each file's digest, the linked repository and branch it tracks, and whether
  the person writes it.

## Run it

1. Draft the input from the Veoveo Trigger's `inputSchema`.
2. Call `n8n__{link}__run` with the workflow and its input, as a Task (call it with Task
   support). The run acts on outside services with the team's credentials.
3. The finished Task returns the execution, its address and the JSON of the Veoveo
   Result's items. A failed execution fails the Task with n8n's error; a run past 24 hours
   is stopped.

## Executions

- `n8n://{link}/executions` pages executions newest first, by workflow and status;
  `n8n://{link}/execution/{execution_id}` holds one: status, timing, the failing node and
  its error, and its data where the person may read it.
- `n8n__{link}__retry_execution` and `n8n__{link}__stop_execution` act on one execution.

A workflow that fails at start, or that the runner refused to apply, is fixed in its file
(skill `author-a-workflow`).
