# Agent Instructions

These rules apply to this repository and to repositories created from it. The crate's
own rules are in [`crates/veoveo-mcp-n8n/AGENTS.md`](crates/veoveo-mcp-n8n/AGENTS.md).

- The design is in [crates/veoveo-mcp-n8n/DESIGN.md](crates/veoveo-mcp-n8n/DESIGN.md).
  Change it in the same change as the code.
- This repository is made from
  [veoveo-template-mcp-rust](https://github.com/VeoveoAI/veoveo-template-mcp-rust) and
  keeps its [rules](https://github.com/VeoveoAI/veoveo-template-mcp-rust/blob/main/AGENTS.md):
  MCP from veoveo-hub-sdk, state from veoveo-core, checked SQL in the server's own
  schema, pinned versions and the `check` gate.
- Use n8n from the [VeoveoAI fork](https://github.com/VeoveoAI/n8n) at its pinned revision.
  Adapt it through its documented environment, its public API and its custom-node
  extension point; a change n8n itself needs goes to the fork, recorded in its
  `VEOVEO_FORK.md`.
- The Veoveo nodes under `nodes/` are plain JavaScript that n8n loads from
  `N8N_CUSTOM_EXTENSIONS` without a build. Keep them thin: they reach the server on
  loopback and hold no logic the server owns.
- Never put a credential value in a team's `workflows/` or `n8n/`, in `n8n/admitted.toml`,
  in `templates/` or in the image. Name a provider's environment variable instead.
- A team's network rules name n8n's program by its path in the image,
  `/usr/local/bin/node`, never a glob or a path under `/sandbox/link`.
- Commits carry no AI or tool attribution.
