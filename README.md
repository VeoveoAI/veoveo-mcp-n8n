# veoveo-mcp-n8n

A Veoveo MCP server that runs teams' n8n workflows. A branch of this repository links
each team's repository at its top level; the Hub runs one runner per link, whose factory
sandbox runs n8n beside the server. The server applies the team's workflows, runs them as
durable Tasks through the Hub and serves their executions, its status and an overview
App. n8n stays contained: it reaches nothing except through OpenShell's proxy or through
the server and the Hub, and a runner serves no editor; people edit in their own Computer.

The design is in [crates/veoveo-mcp-n8n/DESIGN.md](crates/veoveo-mcp-n8n/DESIGN.md).

| Path | Holds |
|---|---|
| `crates/veoveo-mcp-n8n/` | The server, and `veoveo-n8n-computer`, n8n in a person's own Computer |
| `nodes/n8n-nodes-veoveo/` | The Veoveo nodes for n8n: Veoveo Trigger, Veoveo Result and Veoveo Tool |
| `n8n/admitted.toml` | The node and credential types any team may use: `"all"` of each here, every type n8n ships but the always-excluded nodes, or a list |
| `templates/team/` | The template of a team's repository: `workflows/`, `n8n/instance.toml`, which names no node types and so uses every one admitted, `openshell/providers.yaml` and `openshell/policy.yaml` |
| `openshell/` | The factory layout |

## License

Apache-2.0. See [LICENSE](LICENSE). n8n itself is licensed by n8n under its own terms;
this repository does not include it.
