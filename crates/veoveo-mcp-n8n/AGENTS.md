# n8n MCP Server — Agent Manual

Delta over the repository root `AGENTS.md`. [`DESIGN.md`](DESIGN.md) is the design and
operational contract.

## Purpose

Runs a team's n8n workflows as a Veoveo server, as the runner of the team's link: `run`
starts one as a Task, `n8n://executions` shows what happened, `n8n://status` what the
runner runs, and `ui://n8n/overview` shows every team the person reaches. n8n runs beside
the server in the same sandbox and reaches nothing except through OpenShell's proxy or
through the server and the Hub. A runner serves no editor.

## Invariants

- Owns the `n8n://` URI scheme and the PostgreSQL schema `n8n`. The server's SQL touches
  only its own tables there; n8n's `n8n_*` tables belong to n8n's migrations. Identity:
  slug `n8n`, MCP `/n8n/mcp`, port 8811.
- n8n listens only on loopback. Only the server talks to it, through n8n's public API
  and its webhook listener, and only the server uses n8n's owner account.
- n8n holds no real credential. Credentials are providers' placeholders, imported with
  `n8n import:credentials`. A workflow file, `n8n/instance.toml` or `n8n/admitted.toml`
  never holds a secret.
- Workflows are named by their file under the team repository's `workflows/`, at
  `/sandbox/link`. Lore owns them; n8n's copy is applied from the team's files at every
  start. The server never runs anything from `/sandbox/link`.
- A team uses only the node and credential types `n8n/admitted.toml` admits.
- An execution's data goes only to the caller of the run that started it or, for one no
  run started, to a writer of the linked repository; everyone else reads its summary.
- n8n calls into Veoveo only through the bridge, for tools in its typed table and only
  within a running run.
- A runner serves the one tenant its team's `n8n/instance.toml` names.
- `veoveo-n8n-computer` is the only program of a Computer that calls the Hub and the
  model for n8n; n8n's process gets none of the Computer's placeholders, and its
  Assistant's sandbox commands get none either. It applies a team's workflows
  as drafts, never published, and writes saved changes back to the team's checkout;
  it never commits or pushes.
- n8n comes from the VeoveoAI fork at a pinned revision. Configure it here; change it only
  in the fork.

## Build And Test

- `cargo test -p veoveo-mcp-n8n` with PostgreSQL 18 and the pinned n8n on `PATH`, as the
  `dev` image provides them. `tests/workflows.rs` starts the n8n program that
  `VEOVEO_N8N_PROGRAM` names, or `n8n`, and needs port 5678 on loopback; n8n reaches the
  test's loopback listener at `VEOVEO_N8N_TEST_LOOPBACK_HOST`, `127.0.0.1` unless n8n runs
  in a container. `tests/runs.rs`, `tests/bridge.rs` and `tests/mcp.rs` use the fake n8n
  in `tests/support`; `tests/team.rs` reads the template and the teams in
  `tests/fixtures/`; `tests/computer.rs` drives `veoveo-n8n-computer`'s routes against
  a fake n8n, a fake Hub MCP endpoint, a fake Lore and a fake model provider, and runs
  its sandboxes' commands with `bash`.
- `cargo test -p veoveo-mcp-n8n --no-default-features --features contract` checks the
  contract alone.
- Every statement is a `.sql` file under `src/store/workflows/` or `src/store/runs/` run
  through sqlx's checked macros. Regenerate `.sqlx` after changing a query or a migration.
- `docker build --target check .` runs the gate. Its `N8N_IMAGE` names the fork's image by
  digest in the installation's registry; a host that reaches that registry under another
  address passes the same digest there.
