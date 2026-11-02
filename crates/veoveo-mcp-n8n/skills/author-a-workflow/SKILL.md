---
name: author-a-workflow
description: Write or change a team's n8n workflow as a file in the team's Lore repository, with the Veoveo Trigger, Tool and Result nodes, admitted node types and credentials as provider placeholders, drafting it in the person's own n8n before pushing it to the team's runner. Use when a person asks to build, fix or automate something with n8n.
---

# Author a workflow

A team's workflows live in its Lore repository as files, `workflows/{name}.json`, in the
form n8n exports a workflow. The team's runner applies them to its own n8n at each start,
after every push to the branch it tracks. Lore owns the source: nothing edited in a
runner's n8n survives the next apply.

## The file

- `{name}` is the workflow's name everywhere: lowercase ASCII letters and digits in groups
  joined by single hyphens, at most 64 characters, such as `invoice-intake`.
- Members: `name`, `description`, `nodes`, `connections`, `settings`, `staticData`,
  `pinData` and `nodeGroups`. Leave out what n8n sets itself, `id`, `active`,
  `versionId`, `meta` and `tags`; any other member makes the runner refuse to start.
- Each node keeps a stable `id`; a webhook or Veoveo Trigger keeps a stable `webhookId`.

`references/hello-run.json` is a runnable workflow and `references/hello-veoveo.json` a
public webhook, both from the team template.

## A runnable workflow

A workflow is runnable through Veoveo when it starts with an enabled **Veoveo Trigger**,
type `CUSTOM.veoveoTrigger` with a `webhookId`:

- its `inputSchema` parameter, a JSON Schema object as a string, describes the run's input
  for the overview's run form (n8n does not check it);
- the run's input arrives as `$json.input`, and `$json.veoveo` is the run itself.

End it with a **Veoveo Result**, `CUSTOM.veoveoResult`: the JSON of the items it receives is
the run's result. Without one, the items of the last node that ran are.

## Calling Veoveo tools

A **Veoveo Tool** node, `CUSTOM.veoveoTool`, calls a tool of another Veoveo server for the
run's caller: `tool` is its name through the Hub, `{server}__{tool}` such as
`time__resolve_time`, and `arguments` a JSON object the tool's own schema checks. Leave
`run` at its default, the Veoveo Trigger's run. A runner admits only the tools in its
bridge table, today `time__resolve_time`, and only during a run; in the person's own n8n
the node calls any tool the Hub lists to them, as them. An execution a public webhook or a
schedule started belongs to no run and calls no tool.

## Node types and credentials

- The n8n repository's `n8n/admitted.toml` admits node and credential types; a team's
  `n8n/instance.toml` may narrow them with `nodes`. The command, SSH, Git, local-file and
  MCP nodes are refused whatever it says. A workflow using another type makes the runner
  refuse to start, and says which.
- Credentials are declared in `n8n/instance.toml` under `[[credentials]]`, with an `id`
  workflows name, a `type`, plain `data`, and `secrets` mapping each secret field to the
  environment name of the OpenShell provider whose placeholder fills it
  (`references/instance.toml`). Never write a credential value in a file: the real secret
  exists only at the endpoint its provider binds, and the installation, not the
  repository, attaches providers.

## Draft, then push

1. Open the team in the person's own n8n: `/apps/n8n/open?repository={id}&workflow={name}`
   in their Computer clones the team's repository at its tracked branch into
   `~/n8n/{team}` and opens the workflow in the editor.
2. Workflows there are drafts: never published, so their triggers run only in the team's
   runner. Every save is written back to `~/n8n/{team}/workflows/{name}.json`; a new
   workflow with the team's tag gets a new file.
3. Commit and push the checkout with Lore. The runner applies the push; read its
   `n8n://{link}/status` to confirm what it applied (skill `run-a-workflow`).
