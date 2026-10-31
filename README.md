# veoveo-mcp-n8n

A Veoveo MCP server that runs n8n workflows. A branch holds its workflows as JSON; its
factory sandbox runs n8n beside the server, which applies the workflows, runs them as
durable Tasks through the Hub and serves their executions and n8n's editor. n8n stays
contained: it reaches nothing except through OpenShell's proxy or through the server and
the Hub.

The design is in [crates/veoveo-mcp-n8n/DESIGN.md](crates/veoveo-mcp-n8n/DESIGN.md).

| Path | Holds |
|---|---|
| `crates/veoveo-mcp-n8n/` | The server |
| `nodes/n8n-nodes-veoveo/` | The Veoveo nodes for n8n: Veoveo Trigger, Veoveo Result and Veoveo Tool |
| `workflows/` | The branch's workflows |
| `n8n/instance.toml` | The tenant, the credential map and the admitted node and credential types |
| `openshell/` | The factory layout |

## License

Apache-2.0. See [LICENSE](LICENSE). n8n itself is licensed by n8n under its own terms;
this repository does not include it.
