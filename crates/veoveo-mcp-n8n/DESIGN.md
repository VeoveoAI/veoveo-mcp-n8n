# n8n MCP Design

This document is the design and operational contract for the `veoveo-mcp-n8n` crate.

The server runs teams' n8n workflows as a Veoveo server. This repository holds the
server, the Veoveo nodes, its factory layout and what any team may use; a team's
workflows live in a repository of the team's own, which a branch of this repository
links at its top level, one Lore link per team, named by the team, such as `marketing`.
The Hub runs one runner per link: a factory sandbox built from this repository's
revision, with the team's repository cloned at `/sandbox/link`, in which n8n runs beside
the server. The server applies the team's workflows to its n8n, runs them as durable
Tasks for their callers, and serves their executions, its status and an overview App
through the Hub. n8n stays contained: everything it reaches goes through OpenShell's
proxy or through the server and the Hub. n8n holds no real credential, no Hub credential
and no route of its own, and only the server uses n8n's owner account: a runner serves
no editor. People edit workflows in n8n in their own Computer and push the files to the
team's repository.

A repository made from this one is one n8n project. It renames the slug, as a
repository made from the template does, and keeps the structure.

## Service Identity

```text
crate        veoveo-mcp-n8n
binary       veoveo-mcp-n8n, and veoveo-n8n-computer for a person's own Computer
repository   veoveo-mcp-n8n
slug         n8n
URI scheme   n8n
permissions  n8n.workflows.read, n8n.workflows.run, n8n.executions.write
MCP          /n8n/mcp
app          ui://n8n/overview
hooks        /n8n/hooks/{path}
liveness     /livez
readiness    /readyz
port         8811
n8n          127.0.0.1:5678, inside the sandbox only
loopback     127.0.0.1:8812, the Veoveo nodes' run callbacks and the bridge
```

MCP and the hooks are under the server's mount, through the SDK's existing route kinds:
`mcp`, and `public_routes`, which carry no Hub authentication and serve only callers
that verify themselves, as the SDK's operation and provider-event callbacks do. In a
factory sandbox the Hub supplies the port and the internal base URL. The server declares
the hooks in its registration's `veoveo_surfaces`, and no app origin; the registration
response names `veoveo_public_base`, each runner's own. The server builds the hooks'
public URLs under it through `PublicDeployment::with_public_base` and announces them in
`n8n://status` and in each workflow's resource.

A runner never knows its link's name. The Hub names its resources `n8n://{link}/…` and
its tools `n8n__{link}__{tool}`, and forwards each request with the caller's grant on
the linked repository in the token's `https://veoveo.ai/oauth/repository` entry, which
names the repository by its Lore ID and holds the caller's actions on it, `write` among
them for a writer.

## Standards And Protocols

| Standard or protocol | Implemented profile |
|---|---|
| [Model Context Protocol](https://modelcontextprotocol.io/specification/) | Version `2026-07-28`, JSON-RPC 2.0 over Streamable HTTP, through `veoveo-hub-sdk` under the hosted MCP contract: tools, resources and templates, and MCP Tasks for runs. No other version. |
| [n8n](https://docs.n8n.io/) | The [VeoveoAI fork](https://github.com/VeoveoAI/n8n) at one pinned revision, carrying only the changes its `VEOVEO_FORK.md` records, and configured only through its documented environment, its command line (`import:credentials`), its public REST API (`/api/v1` with `X-N8N-API-KEY`) and its custom-node extension point (`N8N_CUSTOM_EXTENSIONS`). The server uses n8n's login and API-key routes only to mint its API key, when n8n does not accept the one it kept. n8n's own MCP server and MCP client nodes are not used. |
| n8n workflow JSON | The source of each workflow, in the form n8n exports it |
| [JWT access tokens RFC 9068](https://www.rfc-editor.org/rfc/rfc9068.html), [Token Exchange RFC 8693](https://www.rfc-editor.org/rfc/rfc8693.html) | The Hub forwards each request with a token naming the caller. A run's calls into other servers exchange the run's Task for a token acting for its caller |
| [OpenShell](https://github.com/VeoveoAI/OpenShell) policy and providers | The sandbox policy names each program's endpoints. Integration credentials are providers, and n8n holds only their placeholders |
| MCP Apps SEP-1865 `2026-01-26` | The overview App `ui://n8n/overview`, a self-contained view under the SDK's [Apps contract](https://github.com/VeoveoAI/veoveo-hub-sdk/blob/main/crates/veoveo-hub-sdk/src/apps/DESIGN.md), which the runners of every link share |
| [Lore](https://github.com/VeoveoAI/lore) links | A branch's top-level links name the teams' repositories; the Hub runs each as a runner, as its [design](https://github.com/VeoveoAI/veoveo-hub/blob/main/crates/veoveo-hub/DESIGN.md) describes |
| [PostgreSQL 18](https://www.postgresql.org/docs/18/) | The schema `n8n` holds the server's tables and n8n's own, which n8n's migrations create under the table prefix `n8n_` |
| [JSON Schema Draft 2020-12](https://json-schema.org/draft/2020-12/) | Tool inputs and results, generated from the contract's types. A run's input and output are the workflow's own JSON |

## Domain Contract

The server answers these questions for a caller:

- Which workflows does this team serve, and is each one active?
- Run this workflow with this input: what did it produce?
- What happened in this execution? Retry it, or stop it.
- What does this runner run, and where do I edit it?

| Surface | Name | Permission | Result |
|---|---|---|---|
| Tool | `run` | `n8n.workflows.run` | A Task whose result is the execution, its address and the JSON of the items it produced |
| Tool | `retry_execution`, `stop_execution` | `n8n.executions.write` and the [data rule](#execution-data)'s access | The execution |
| Resource | `n8n://workflows{?limit,cursor}` | `n8n.workflows.read` | A page of workflows: name, whether it is runnable and active, the applied file digest |
| Resource | `n8n://workflow/{workflow}` | `n8n.workflows.read` | The workflow's JSON as applied and the public URL of each of its webhooks |
| Resource | `n8n://status` | `n8n.workflows.read` | What the runner runs: n8n's version, the server's, when it applied its workflows, each applied file's digest and the hooks' public URL; through a runner, the linked repository the caller reached it through, the branch of it the link tracks, by its Lore ID, whether their grant writes, and where their own Computer edits it |
| Resource | `n8n://executions{?workflow,status,limit,cursor}` | `n8n.workflows.read` | A page of executions: status, timing and mode, no data |
| Resource | `n8n://execution/{execution_id}` | `n8n.workflows.read` | One execution: status, timing, the failing node and the error, and its data as the [data rule](#execution-data) admits |
| App | `ui://n8n/overview` | `n8n.workflows.read` | The [overview](#the-overview-app) of every team the person reaches, with `run` linked for the model and the App |
| Resource | `n8n://docs`, `n8n://docs/{doc_id}`, `n8n://contract` | `n8n.workflows.read` | The embedded documents and the contract declaration |

A workflow is named by its file under the team repository's `workflows/`, such as
`invoice-intake` for `workflows/invoice-intake.json`, never by n8n's internal ID: lowercase ASCII letters and
digits in groups joined by single hyphens, at most 64 characters, so its order is the same
in Rust and in PostgreSQL's `C` collation. It is runnable when it starts with an enabled
Veoveo Trigger node with a webhook ID, `CUSTOM.veoveoTrigger`: n8n names a node it loads
from `N8N_CUSTOM_EXTENSIONS` under the package `CUSTOM`. The Veoveo Result and Veoveo
Tool are `CUSTOM.veoveoResult` and `CUSTOM.veoveoTool`. The Veoveo Trigger's `Input
Schema` parameter, a JSON Schema object, describes the run's input for the overview's
run form; n8n does not check it. A run acts on outside services with its team's
credentials, the providers its team's repository requests and the installation grants
it. `n8n.workflows.run` declares no risk, so a role that accepts none, such as `member`,
runs a team's workflows when it grants the permission and its holder has a grant on the
team's repository. This is open while the platform is in early access, and is to be
locked down later.

One n8n instance is one trust domain: any workflow in it can read what n8n stores for
every other. Each team therefore has an n8n of its own, its runner's, over a database
the Hub gives that runner alone, and a runner serves one tenant, the one the team's
`n8n/instance.toml` names; a caller of another tenant is refused.

## Architecture

### Library Features

| Feature | Holds | Depends on |
|---|---|---|
| `contract` | `WorkflowName`, `ExecutionId`, `N8nResource`, `N8nPermission`, the tool requests and results | `veoveo-types`, the SDK's `contract` feature, Serde and Schemars |
| `server` (default) | The hosted server, n8n's supervision and client, the store and the binary | The SDK's `server` feature, core's store, sqlx and rmcp |

### Modules

| Module | Responsibility |
|---|---|
| `contract` | The public types a consumer compiles against |
| `mcp` | The typed ready routes, resource readers, apps, setup and the registration declaration |
| `n8n` | n8n's child process, its environment, its loopback REST client and its first-start bootstrap |
| `instance` | The team's `n8n/instance.toml` and what this repository's `n8n/admitted.toml` admits |
| `workflows` | Reading the team's `workflows/`, applying it to n8n and recording what is applied |
| `runs` | The Task service of `run`, the worker: the dispatch to n8n, the wait and the result, and n8n's status check and cancellation |
| `bridge` | Calls from n8n's Veoveo Tool node into other servers |
| `loopback` | The server's loopback listener, where the Veoveo Result posts a run's output and the Veoveo Tool calls the bridge |
| `hooks` | Public webhook deliveries to n8n's webhook listener |
| `proxy` | Forwarding a request to n8n's listener, for the hooks and a Computer's editor |
| `computer` | [`veoveo-n8n-computer`](#n8n-in-a-persons-computer): n8n in a person's own Computer, its editor, the teams' checkouts, its bridge and its Assistant's model endpoint and sandbox service |
| `store` | The server's own tables in the `n8n` schema |
| `server` | Configuration, registration with the Hub, the database login, the process instance and serving |

### Repository Layout

| Path | Holds |
|---|---|
| `crates/veoveo-mcp-n8n/` | The server, its migrations, the overview App's document in `assets/`, tests and the documents it serves |
| `crates/veoveo-mcp-n8n/tests/fixtures/` | The tests' teams: one with the tests' workflows and a header and a bearer credential, one with nothing optional and one that names a node type a listed repository, `admitted-narrow.toml`, does not admit |
| `nodes/n8n-nodes-veoveo/` | n8n custom nodes in plain JavaScript, in n8n's community-node layout: Veoveo Trigger, Veoveo Result and Veoveo Tool. n8n loads them from `N8N_CUSTOM_EXTENSIONS` with no build step, since the factory image has Node.js but no compiler for TypeScript |
| `n8n/admitted.toml` | The node and credential types any team may use, as [the instance](#the-instance) describes |
| `templates/team/` | The template of a team's repository, as [teams](#teams) describes, with two example workflows |
| `openshell/image/`, `openshell/run`, `openshell/policy.yaml` | The [factory layout](#factory) |
| Top-level links of a branch | One per team, named by the team, to the team's repository. A branch holds no `workflows/` of its own |
| Root files | The template's: the workspace, toolchain, gate, `Dockerfile` and `.sqlx/` |

### Teams

A team's repository is any Lore repository made from `templates/team/`:

| Path | Holds |
|---|---|
| `workflows/*.json` | The team's workflows, one JSON file each; none when the folder is missing |
| `n8n/instance.toml` | The team's tenant, the node types it narrows its runner to, if any, and its credential map, as [the instance](#the-instance) describes |
| `openshell/providers.yaml` | The installation provider profiles the team requests, such as `vv-factory-model`; optional |
| `openshell/policy.yaml` | Network rules for the team's integration hosts, under the same limits as a branch's; optional |

The Hub adds the team's network rules and providers to its runner's sandbox, and none of
this repository's providers: a team requests what its workflows use. A team's rule names
n8n's program by its path in this repository's image, `/usr/local/bin/node`, and never a
glob or a program under `/sandbox/link`, which the Hub refuses: what lies there is
content the server reads, never a program of its own. A team changes what its runner
serves by pushing to the tracked branch of its link; this repository changes only to
add, retarget or remove a team's link, or to change the server, the nodes or what teams
may use.

### Startup

`openshell/run` starts the server with `--workflows /sandbox/link/workflows`,
`--instance /sandbox/link/n8n/instance.toml` and `--admitted n8n/admitted.toml`, the
last from this repository's checkout. The server reads what this repository admits, the
team's instance and the team's workflows, and refuses a malformed one before it
contacts anything; a team whose instance names a node or credential type
`n8n/admitted.toml` does not admit fails the start with a message that names the type
and the file to change. It builds its registration declaration and registers with the
Hub, which issues its database login and its public base; a registration without either
fails the start. It applies its
migrations, reads n8n's encryption key from its own table, as [the key](#the-key)
describes, then starts n8n as its child process with the
[configuration](#n8n-configuration) below. n8n creates its owner from the environment:
the server generates a new password for the owner at every start and passes n8n only its
bcrypt hash, which n8n applies to the owner it keeps. Once n8n answers
`/healthz/readiness`, which follows its migrations, the server checks the API key it
kept under `/sandbox/state/n8n` against `/api/v1`. When the disk keeps none, on first
start or in a new sandbox, or n8n refuses it, as when the runner's database holds an n8n
other than the one the sandbox's disk knew, the server logs in as the owner at
`/rest/login`, revokes the keys under its label that n8n still holds, since n8n lets an
owner hold one key per label, mints a new key at `/rest/api-keys` with the scopes it
uses and keeps it. While n8n starts, the server asks `n8n --version` for the version
`n8n://status` names, or names it `unknown`. It imports the instance's credentials with
`n8n import:credentials`, applies the team's workflows through `/api/v1`, forgets the
[dispatched executions](#execution-data) n8n no longer keeps and serves; `/readyz`
answers only from then on. Beside MCP it runs the Task worker of `run` and the loopback
listener of the Veoveo nodes.

### The Key

n8n's data, its tables in schema `n8n`, lives in the runner's database and outlives the
sandbox: a push that changes `openshell/image/` or `openshell/policy.yaml`, a team's
push that changes its `openshell/` files, or a changed factory base, gives the runner a
new sandbox with an empty disk over the same data. n8n's encryption key therefore lives with that
data, in the server's table `n8n.instance_key`, the only source of the key: a kept row is
used; with no row and none of n8n's tables, the server generates a key and keeps it; with
n8n's tables and no row, the start fails and says that n8n's data cannot be read without
its key, rather than let n8n fail on its own data. Before every start the server writes
the key into n8n's settings file, `encryptionKey` in `.n8n/config` of n8n's user folder,
keeping the file's other fields or creating it, because n8n refuses to start when that
file names another key, as a disk that outlived a reset of n8n's data would. Every
credential n8n holds is a provider's placeholder, so the key protects no real secret: it
only lets n8n read its own data again. The key's row also names the n8n instance: a new
row, with n8n's data started again over a reset schema, is a new instance, whose
execution numbers start again at 1.

What a new sandbox loses is only what the server makes again: the owner's password, new
at every start, the API key, minted again, and n8n's user folder, which holds nothing n8n
cannot rebuild.

If n8n exits, the server exits, and OpenShell restarts the workload; the server never
restarts n8n itself. SIGTERM stops claiming runs, lets running dispatches settle for up
to 30 seconds, stops n8n and exits. Registration and deployment identity follow the
template.

## n8n Configuration

| Setting | Value | Holds |
|---|---|---|
| `NODE_ENV` | `production` | n8n runs as in production wherever it runs; without it n8n takes itself for a development build and drops its security headers, `X-Frame-Options` among them |
| `N8N_LISTEN_ADDRESS`, `N8N_PORT` | `127.0.0.1`, `5678` | Only processes in the sandbox reach n8n |
| `N8N_USER_FOLDER`, `N8N_ENFORCE_SETTINGS_FILE_PERMISSIONS` | `/sandbox/state/n8n/home`, `true` | n8n's own settings file and folders live on the sandbox's persistent disk, readable by its user alone |
| `N8N_WEBHOOK_URL` | `{veoveo_public_base}/hooks/` | n8n builds and registers webhook URLs on the Hub's routes; a production webhook is `{N8N_WEBHOOK_URL}webhook/{path}`. n8n 2.46 deprecates `WEBHOOK_URL` for `N8N_WEBHOOK_URL`, which sets the base of test and production webhooks alike. No editor base URL is set: a runner serves no editor |
| `N8N_INSTANCE_OWNER_MANAGED_BY_ENV`, `N8N_INSTANCE_OWNER_EMAIL`, `N8N_INSTANCE_OWNER_PASSWORD_HASH` | `true`, the server's owner address, the bcrypt hash of the password the server generates at every start, which n8n applies to its owner | n8n's owner exists from the first start, with no setup step |
| `DB_TYPE`, `DB_POSTGRESDB_*`, `DB_TABLE_PREFIX` | `postgresdb`, the registration's login and schema `n8n`, `n8n_` | n8n's state lives in the installation's PostgreSQL, in the runner's own database |
| `PGOPTIONS` | `-c role=server_n8n` | Every n8n connection takes the server's group role, so n8n's tables belong to it, as core's database design requires of a server's objects. The fork checks for the schema before `CREATE SCHEMA`, so the login needs no CREATE privilege on the database |
| `DB_POSTGRESDB_SSL_ENABLED`, `DB_POSTGRESDB_SSL_CA_FILE`, `DB_POSTGRESDB_SSL_REJECT_UNAUTHORIZED` | `true`, the bundle OpenShell names in `SSL_CERT_FILE`, `true` | n8n verifies PostgreSQL against the platform roots; n8n reads `DB_POSTGRESDB_SSL_CA` from the file its `_FILE` form names. Node reads only `NODE_EXTRA_CA_CERTS`, which OpenShell sets to the sandbox's interception CA alone. The grant's URL has `sslmode=verify-full`; on a test cluster it has `sslmode=disable` and n8n connects without TLS. Any other mode fails the start |
| `N8N_ENCRYPTION_KEY` | The key in `n8n.instance_key`, beside n8n's data, as [the key](#the-key) describes | n8n encrypts its credentials, which hold only placeholders, so the key protects no real secret |
| `N8N_BLOCK_ENV_ACCESS_IN_NODE`, `N8N_BLOCK_FILE_ACCESS_TO_N8N_FILES`, `N8N_RESTRICT_FILE_ACCESS_TO` | `true`, `true`, `/sandbox/n8n-files` | Workflows read no environment and no file outside one folder |
| `NODES_INCLUDE`, `NODES_EXCLUDE` | The node types the runner loads, as [the instance](#the-instance) decides, with the Veoveo nodes; and n8n's command, SSH, Git, local-file and MCP nodes, among them the Microsoft Agent 365 trigger, whose agent calls Microsoft's MCP servers. When the team loads every node type n8n ships, `NODES_INCLUDE` is unset | n8n loads the team's node types and the Veoveo nodes, and never the excluded ones, which neither file can admit. No node n8n ships runs commands, reaches past the policy's rules or speaks MCP. n8n's `mcp-registry` module, which stays on, makes a node type `@n8n/mcp-registry.{server}` on the MCP registry client node for each server of n8n's registry once it has fetched the registry from `api.n8n.io`; it takes that base node from the langchain package directly, and `NODES_EXCLUDE` names no type it makes, so those node types are not excluded |
| `N8N_CUSTOM_EXTENSIONS` | The checkout's `nodes/n8n-nodes-veoveo` | The Veoveo nodes, which n8n always loads |
| `VEOVEO_BRIDGE_URL` | The loopback bridge, `http://127.0.0.1:8812/n8n/bridge/calls` | The Veoveo Tool node lists the bridge table's tools for its [tool list](#calls-into-veoveo); each run still names the bridge and its token |
| `N8N_MCP_MANAGED_BY_ENV`, `N8N_MCP_ACCESS_ENABLED` | `true`, `false` | n8n's own MCP server stays off |
| `N8N_COMMUNITY_PACKAGES_ENABLED`, `N8N_DIAGNOSTICS_ENABLED`, `N8N_VERSION_NOTIFICATIONS_ENABLED`, `N8N_TEMPLATES_ENABLED` | `false` | n8n installs and fetches nothing on its own |

n8n's program gets the server's environment without the variables whose names start
with `VEOVEO_`, `N8N_`, `DB_` or `NODES_`, nor `WEBHOOK_URL` or `PGOPTIONS`, and with the
settings above. The server's own configuration and its registration secret stay with it,
and only the server configures n8n. A runner sets no `N8N_PATH`; only a person's own
Computer serves the editor, under its path.

### The Instance

This repository's `n8n/admitted.toml` states what any team may use; an unknown member is
refused. Each of its two members is `"all"` or a list, and this repository admits all:

```toml
nodes = "all"
credentials = "all"
```

`nodes` are the node types a team's workflows may use, beside the Veoveo nodes: `"all"`,
every one n8n ships but the excluded ones, or a list of at least one, which may not name
an excluded node type. `credentials` are the credential types a team may import:
`"all"`, or a list, which is empty when the file names none. A list such as
`nodes = ["n8n-nodes-base.webhook", "n8n-nodes-base.set"]` narrows what teams may use.

The team's `n8n/instance.toml` declares its instance; an unknown member is refused.

```toml
tenant = "reference"

[[credentials]]
id = "githubToken0001"
name = "GitHub"
type = "githubApi"
data = { server = "https://api.github.com", user = "veoveo" }
secrets = { accessToken = "GITHUB_TOKEN" }
```

`tenant` is the one tenant the runner serves. Without `nodes`, as the team template has
it, the runner loads every node type `n8n/admitted.toml` admits: with `"all"`, every one
n8n ships but the excluded ones, so `NODES_INCLUDE` stays unset; with a list, that list.
A team may narrow it with `nodes`, the node types n8n loads beside the Veoveo nodes, each
one `n8n/admitted.toml` admits, and `nodes = []` for the Veoveo nodes alone; an excluded
or unadmitted node type fails the start. Each credential, of a type `n8n/admitted.toml`
admits, which with `"all"` is any type, is imported under its
`id`, 1 to 36 ASCII letters and digits, so a workflow names it. `data` holds the fields
that hold no secret; `secrets` maps each secret field, by its path with `.` between the
keys of nested objects such as `oauthTokenData.access_token`, to the provider
environment variable whose placeholder fills it. A field is set once, as data or as a
secret, and a provider variable never starts with `VEOVEO_`. A credential whose
variable no attached provider sets fails the start.

## Containment

### Network

The sandbox policy names two programs. n8n's `node` reaches PostgreSQL, the model
endpoint and each integration host that the team's `openshell/policy.yaml` lists, by
exact host, with REST rules where an API states its intent in method and path. The
installation's base rule for PostgreSQL names only the branch's own build, so this
repository's policy adds the rule that admits n8n's `node` to the database host. The server's binary
reaches the Hub, PostgreSQL and Artifact's object URLs. No rule is a wildcard. The
Veoveo nodes run inside n8n's process and reach the server only on loopback.

### Credentials

Real credentials are OpenShell providers that the installation attaches to the
repository's instances; a repository never adds one. n8n holds only placeholders, and
OpenShell's proxy replaces a placeholder only at the endpoint its provider binds. At
each start the server imports every credential the team's `n8n/instance.toml` maps with
`n8n import:credentials`, whatever its type, with the provider's placeholder in place of
each secret field; n8n encrypts it on import. A credential type that n8n authorizes
through OAuth2 holds the placeholder as its access token, with no expiry, and n8n
refreshes it only when an API answers 401. n8n's credential overwrites are not used:
they fill only empty fields, so they set defaults and enforce nothing. The provider's own refresh
runs in OpenShell's gateway, and no rule admits n8n to a token endpoint, so a refresh n8n
attempts fails and the request fails with it.

Each credential-bound endpoint binds strictly: OpenShell admits a request there only
when its credential carries the bound placeholder. A value a workflow file carries in
place of a placeholder therefore never leaves the sandbox. Workflow files and
`n8n/instance.toml` name providers' environment names, never values.

### Calls Into Veoveo

The Veoveo Tool node posts a tool name, its arguments and its run's token to the bridge
on the server's loopback listener, `/n8n/bridge/calls`. The bridge admits a call only for
a tool in its typed bridge table and only with the token of a run whose Task has not
ended. Each entry names the provider's tool by its Hub name, such as
`time__resolve_time`, and the provider permission it is called with, which the server
declares in its `uses`; the arguments are n8n's JSON, which the provider checks against
its own schema. An entry names the tool rather than taking the provider's `ToolCall`
because the providers' contract features pin other SDK revisions than this server's. The
bridge calls through `ServerCalls::for_task` for that run's Task: the Hub exchanges the
Task for a token that acts for the run's caller and carries the permissions the server
declared and the caller holds at that moment. A tool outside the table and a token of no
running run are refused with 403; a refusal of the Hub or the provider is the tool's
error result. An execution that a webhook or n8n's own schedule started belongs to no
run and calls into no server.

The node's Tool is a list n8n loads from `GET /n8n/bridge/tools` on the bridge that
`VEOVEO_BRIDGE_URL` names in n8n's environment, with `VEOVEO_BRIDGE_TOKEN` as the bearer
where it is set: on a runner the bridge table's tools, which the server lists to any
caller on loopback, and in a person's Computer the tools the Hub lists to the owner. The
list holds each tool's Hub name, the value the node stores, so a workflow file keeps its
form; a stored name the list does not hold stays, and a name can be typed in n8n's
expression mode. A runner serves no editor, so only n8n's own API shows its list there.

### Models

n8n's AI nodes call the model endpoint through OpenShell's proxy with the model
provider's placeholder. The Hub is not a model proxy. In a person's Computer, n8n's
Assistant reaches the Computer's model through `veoveo-n8n-computer`
([The Assistant](#n8n-in-a-persons-computer)).

## Runs

`run` creates a durable Task for its caller, of the runner's tenant and for a workflow the
runner applied that starts with the Veoveo Trigger; a call without the client's Task
support is told it needs it. The worker dispatches one operation to n8n, with the
operation ID as its idempotency key. It records the run with the SHA-256 of a fresh run
token, then calls the workflow's Veoveo Trigger on loopback, n8n's production webhook
`{webhookId}/run`, with the header `x-veoveo-dispatch: run`, which the hooks never
forward, and a body naming the operation ID, the run token, the operation's callback URL,
the bridge's URL and the input. The trigger answers at once with n8n's execution ID, as
n8n's `onReceived` response mode lets a webhook answer with `$execution.id`, and the
dispatch records the operation's job as `{instance}:{execution}`, the n8n instance of
[the key](#the-key) and the execution number, so a new n8n's executions never meet an
earlier one's runs. Beside it the dispatch records the execution as the Task caller's,
as [execution data](#execution-data) describes. The Task then parks.

The Veoveo Result node posts the JSON of the items it receives to the callback URL on the
server's loopback listener, `/n8n/operations/{operation_id}/{token}`: the operation's
callback token, which only that execution's data holds, authenticates it, and the server
stores it as the operation's final event, named by the operation, as each status check's
observation is, never by the execution number alone. Every operation is also checked: the first
status check two seconds after the dispatch, each interval 1.2 times the one before, at
most 60 within the run's deadline of 24 hours, through n8n's public API. A run without a
callback, such as one dispatched again, completes from the checks.

| n8n's execution | The Task |
|---|---|
| `success` | Completes with the execution, its address and the items of the Veoveo Result, or of the last node when none ran |
| `error`, `crashed` | Fails with `execution_failed` and n8n's error |
| `canceled` by someone in n8n | Fails with `execution_stopped`: the Task runtime ends a Task as cancelled only after its caller asked |
| Running past the deadline | The server stops the execution and fails the Task with `run_deadline_passed` |

Cancelling the Task stops the execution, and n8n's answer is the cancellation's receipt.
A restarted worker finds the recorded operation and keeps waiting on its execution
instead of starting the workflow again; a dispatch that recorded no execution looks for
the execution whose Veoveo Trigger emitted its operation ID before it calls the trigger
again. The worker deletes the run's row when the Task ends. Binary output goes to
Artifact for the caller through Artifact's signed URLs.

`retry_execution` and `stop_execution` act on n8n's executions through its public API,
for a caller with the data rule's access to the execution, and
`n8n://executions{?workflow,status,limit,cursor}` pages n8n's executions newest first.
The listing's cursor is the server's, bound to its filters and limit; a next page asks n8n
for the executions after the cursor's, in n8n's own cursor form, and n8n lists a running
execution only when the status filter asks for `running`.

### Execution Data

An execution's data, its nodes' inputs and outputs and its result, follows the authority
it was fetched with:

| The execution | Its data goes to |
|---|---|
| Dispatched for a `run` Task, job `{instance}:{execution}`, or a retry of one | That Task's caller alone: the same principal, of the runner's tenant |
| Started by a webhook or a schedule, or a retry of one | A caller whose grant on the linked repository, in the forwarded token, holds `write` |

Everyone else admitted to `n8n://execution/{execution_id}` reads its status, timing,
the failing node's name and the error's message, and the read names its `access`,
`data` or `summary`. A listing carries no data for anyone. `retry_execution` and
`stop_execution` need the data rule's access beside `n8n.executions.write`. A workflow
file's `settings.saveDataSuccessExecution` and `saveDataErrorExecution` reach n8n as the
file sets them, so with `none` n8n keeps no data of such an execution, and nobody reads
it. The dispatch records the execution's Task and caller in `n8n.run_execution`, since
the run's own row ends with its Task; a retry of a run's execution, which only that
caller may ask for, is recorded as theirs too. An execution no row names is a webhook's
or a schedule's. Each start forgets the rows of another n8n instance and those older
than n8n's default pruning of 14 days.

## Workflows From Lore

At each start the server reads the team's `workflows/*.json`, at most 500 files of at most 4 MiB,
each in the form n8n exports a workflow. It sends n8n the members of the public API's
write shape, `name`, `description`, `nodes`, `connections`, `settings`, `staticData`,
`pinData` and `nodeGroups`, and leaves out the members n8n sets itself, such as `id`,
`active`, `versionId`, `meta` and `tags`; any other member fails the start, so a typo
never passes silently. A file without `name` takes its workflow's name, and one without
`settings` empty settings.

For each file the server decides from what it recorded at the last apply and what n8n
holds now:

| n8n holds | The server |
|---|---|
| Nothing the server recorded, or the recorded workflow is gone | Creates the workflow and publishes it |
| The recorded workflow, with another file digest, another version than the one recorded, or archived | Restores it when archived, replaces it with the file and publishes it |
| The recorded workflow and version, unpublished | Publishes it |
| The recorded workflow and version, published | Keeps it |

A workflow the server recorded whose file is gone is archived, which unpublishes it, and
forgotten; one n8n no longer holds or already archived is only forgotten. The server
records each file's SHA-256, the n8n version its apply produced and the JSON it sent,
before it publishes, so a failed publish never leaves a workflow it does not know. A
workflow n8n refuses, such as one with a node type the instance does not admit, fails the
start with n8n's answer, and the server logs each create, update, publish, archive and
replaced version.

Lore owns the source. Only the server writes a runner's n8n, so a version the server
did not apply comes only from n8n itself; a start applies the team's files over it,
n8n's workflow history keeps the replaced version, and the server logs it.

## The Overview App

`ui://n8n/overview` is a self-contained view, with no remote origin, which the runners
of every link share: the Hub lists it once for the server. It finds the teams the person
reaches among the server's resources the host lists for the view, each runner's
`n8n://{link}/status`, and reads each link's `status`, `workflows`, `workflow/{name}` and
`executions?limit=100`, with `executions?status=error&limit=100` for the failures of the
last 24 hours. A team the person cannot read shows why. For each team it shows its
workflows, each with its trigger, whether it is active, its last run, its failures in
24 hours and its webhooks' public URLs; a list of what needs attention: a last run that
failed, recent failures, a trigger that is not active, a team that cannot be read; the
recent executions; and, on the person's request:

- **Run**: a form built from the Veoveo Trigger's input schema, top-level fields of
  string, number, integer or boolean, or a JSON object otherwise, which calls
  `{link}__run`, the App's only linked tool, as an MCP Task and polls it.
- **View**: the workflow's nodes, read-only, and its file digest.
- **An execution**: its status, timing, failing node and error, and its data only when
  the read's `access` is `data`, as [execution data](#execution-data) rules.
- **Edit in my Computer**: `ui/open-link` to the status's `editUrl` with
  `&workflow={name}`, `{issuer}/computer/apps/n8n/open?repository={linked repository
  ID}&workflow={name}`, which the Hub opens top-level as a program of the person's own
  Computer. The server knows the Hub's issuer from its registration and the linked
  repository from the forwarded token's repository entry.

It reads again when the person asks, never on a timer, and polls only a Task it started.
A server that is no runner names no link: the overview reads `n8n://status` and calls
`run`, and offers no Computer.

## n8n In A Person's Computer

`veoveo-n8n-computer` runs n8n in a person's own Computer, as a program of the Computer
version that the Hub forwards `/computer/c/{computer}/apps/n8n/…` to on a loopback port,
5680 by default. It is the one program of the Computer that calls the Hub and the model
for n8n: n8n's process receives none of the Computer's credential placeholders and no
credential rule names it.

- **n8n.** It starts n8n on loopback, as a runner does, with SQLite in n8n's user folder
  under `$HOME/.veoveo/n8n`, every node type but the excluded ones, and the Veoveo nodes.
  n8n's encryption key and its owner's password are generated once and kept there with
  mode 0600; n8n creates the owner from the environment with the password's hash, and
  the program mints its API key, with the workflow and tag scopes it uses, as the server
  does. One person, one Computer: they are its owner.
- **The editor.** The browser reaches the editor at `{base path}/apps/n8n/`, n8n's
  `N8N_PATH` and, under the Hub's public origin, its `N8N_EDITOR_BASE_URL`. The program
  forwards everything under `/apps/n8n/` to n8n's listener without the mount, streaming
  both bodies, with the owner's n8n session, which it signs in for with the kept
  password, renews from n8n's answers and signs in again an hour before it expires or
  after n8n refused it. It sets the request's origin to n8n's own, as n8n's push channel
  checks, sends server-sent events for the push channel, and strips n8n's cookies from
  the answers; the Hub already strips the browser's cookies and credentials both ways.
- **Teams.** It lists the teams the person reaches through the Hub's MCP endpoint of the
  Computers profile as the owner, with the Computer's `COMPUTER_HUB_REFERENCE`
  placeholder as its bearer: each `n8n://{link}/status` among the person's resources is
  a team, which names its repository, its tracked branch and whether the person writes
  it. `GET /apps/n8n/teams` shows them.
- **Checkouts.** `GET /apps/n8n/open?repository=&workflow=` finds the team whose status
  names the repository, clones it with the Computer's Lore at the tracked branch into
  `$HOME/n8n/{team}` unless that checkout exists, writes back what was saved, applies
  the checkout's `workflows/*.json` and redirects to the workflow in the editor, or to the
  editor's workflows without one. A team whose runner names no tracked branch is not
  cloned. The apply is the server's, with each workflow a draft: it is created or updated
  and never published, so its triggers run only in the team's runner; the team's
  workflows carry the n8n tag `veoveo:{team}`, and what was applied is recorded in
  `$HOME/.veoveo/n8n/teams/{team}.json`. Nothing is synced from Lore after the clone:
  the checkout is the person's.
- **Saving back.** Every five seconds the program reads each applied workflow's
  version from n8n's public API; a version the person saved since is written back to
  `$HOME/n8n/{team}/workflows/{file}.json` in the server's file form, the public API's
  write shape, and recorded, so the next apply keeps it. A new workflow carrying the
  team's tag gets a new file named after it, unless one by that name exists. Polling
  the public API is the choice: n8n's external hooks run code inside n8n, which the
  program keeps out of n8n's process. Nothing is committed or pushed: the person, or
  their agent, commits and pushes with Lore, and the team's runner applies the push.
- **The Computer image.** The root `Dockerfile`'s `computer` target holds what a
  Computer adds for n8n: Node.js, npm and n8n from the fork's image at
  `/usr/local/bin/node`, `/usr/local/lib/node_modules/npm` and
  `/usr/local/lib/node_modules/n8n`, `veoveo-n8n-computer` at `/usr/local/bin`, built
  as the server is, and the Veoveo nodes at `/opt/veoveo/n8n/nodes`, every file readable
  and every directory traversable by everyone, root owning them, whatever modes the build
  context gives them; the build checks as the Computer's user, uid 10001, that it reads
  every tree, lists the nodes and runs Node.js, npm, n8n and the program. The installation
  copies them into its Computer image, which starts from its factory image, a Debian
  trixie image as the fork's and the build's are, and adds `librdkafka1`, which n8n's
  native modules load; the factory base has no Node.js of its own. The Computer's start
  command starts the program in the background, restarting it after a pause whenever it
  exits, and `exec`s OpenCode in the foreground, so n8n never delays or stops OpenCode
  and its output, failures included, is the Computer's own. The Computer version names
  the program `n8n` on its port in `computers.apps`, and its policy's `hub` and `model`
  rules name `/usr/local/bin/veoveo-n8n-computer`. The Computer image names the
  Assistant's model in `VEOVEO_N8N_MODEL`, as its OpenCode configuration names
  OpenCode's.
- **The bridge.** The Veoveo Tool node finds the program's loopback bridge,
  `/n8n/bridge/calls`, and a token of this start in n8n's environment when no run names
  them, and posts the runner's request shape. The program admits the token, a tool's
  Hub name `{server}__{tool}`, and calls it through the Hub as the owner with the
  Computer's own credential, so the Hub decides each call as it decides the owner's.
  `GET /n8n/bridge/tools`, with the token as its bearer, lists the tools the Hub lists
  to the owner, each with its Hub name and description, for the node's
  [tool list](#calls-into-veoveo).
- **The Assistant.** n8n's Assistant (Instance AI) is configured from n8n's environment
  and reaches the model, its sandboxes and the person's tools only through the program,
  which admits the token of this start on each, as on the bridge:
  - *The model.* `N8N_INSTANCE_AI_MODEL` is `custom/{VEOVEO_N8N_MODEL}` and
    `N8N_INSTANCE_AI_MODEL_URL` is `http://127.0.0.1:{port}/v1/openai`, with the token as
    its key. n8n's `custom/` provider with a URL is its OpenAI-compatible client, which
    posts only `{url}/chat/completions` with the model's name after `custom/`. The
    program serves `POST /v1/openai/chat/completions`, the provider's own path, and
    forwards it to `VEOVEO_N8N_MODEL_ENDPOINT`, DeepInfra's chat completions by default,
    with `DEEPINFRA_API_KEY`, the Computer's placeholder, as its bearer, which the
    `model` rule's binding replaces with the owner's key. The request goes whole, with
    its length, to the proxy that inspects it, and the answer streams. n8n's
    model catalog, `https://models.dev/api.json`, gives costs and checks of the chat
    model nodes the Assistant writes; it is optional to n8n, and the Computer's internet
    rule, which admits names of three labels or more, refuses that two-label name.
  - *The sandboxes.* `N8N_INSTANCE_AI_SANDBOX_ENABLED` is `true` with the provider
    `n8n-sandbox`, whose service is the program at `http://127.0.0.1:{port}/n8n`, with
    the token as its key. The program serves the n8n sandbox service's API as
    `@n8n/sandbox-client` 0.1.1 calls it: sandboxes created by id or anew, read and
    deleted; executions streamed as NDJSON events (`started`, `stdout`, `stderr`, `exit`
    or `error`, each with its `seq`), the same `exec_id` reading the same execution,
    followed after a `seq`, and deleted, which kills a running one; and the file calls.
    Each sandbox is a directory of the person's home, `$HOME/n8n/.sandboxes/{id}`, which
    is its home; the Assistant works in its `workspace`. A command runs as the person
    with `bash -c` in its own process group, from the workspace or the directory it
    names in the sandbox, with the sandbox's home as `HOME`, the person's npm cache, the
    request's variables and the program's environment without the variables n8n never
    receives; at its limit, and once it exits, the program kills its group. A file call's
    path is absolute in the sandbox or relative to its home, and a path outside it is
    refused. n8n states its sandboxes' workspace in the Assistant's prompt, and the
    workspace refuses absolute paths outside it, so the fork's `N8N_SANDBOX_SERVICE_HOMES`
    names `$HOME/n8n/.sandboxes`, and n8n takes `{N8N_SANDBOX_SERVICE_HOMES}/{id}` for a
    sandbox's home instead of `/home/user`. A sandbox stays until n8n deletes it, and an
    execution's events until n8n deletes them, as the client does after the exit. The
    Assistant's workspace setup installs its packages with npm from the registry, through
    the internet rule.
  - *The person's tools.* `N8N_INSTANCE_AI_MCP_SERVERS` is
    `veoveo=http://127.0.0.1:{port}/n8n/mcp/{token}`: n8n's form is `name=url` pairs
    with no headers, so the token is the path's last segment. The program serves
    `POST /n8n/mcp/{token}` and forwards each request to the Hub's MCP endpoint of the
    Computers profile, `COMPUTER_HUB_URL`, with `COMPUTER_HUB_REFERENCE` as its bearer,
    which the `hub` rule's binding replaces with the owner's token: the request's
    headers but the connection's own, its credentials and its cookies, and its body
    whole, with its length; the answer streams back without cookies. The Hub answers
    every request itself, `server/discover` among them, and decides each call as the
    owner's, as for OpenCode. The fork's MCP client offers MCP 2026-07-28 first and the
    Hub serves only it, so the Assistant speaks 2026-07-28 to the Hub. A path with
    another token is refused with 401, and another method with 405. n8n names each tool
    `veoveo_{server}__{tool}` to its model.
  - *Permission and confirmation.* n8n's own. A server from `N8N_INSTANCE_AI_MCP_SERVERS`
    carries no tool permissions of its own, so the Assistant's two MCP permissions
    apply, which the owner sets in the Assistant's settings: `mcpRead`, by default
    *always allow*, for a read tool, and `mcpWrite`, by default *require approval*, for
    every other. A tool is a read tool when its `readOnlyHint` annotation is true; it is
    a write tool when that hint is false or its `destructiveHint` is true; without
    either, its name decides: a write word such as `create`, `run` or `send` makes it a
    write tool, a read word such as `get`, `list` or `search` a read tool, and a name
    with neither is a write tool. A tool that requires approval stops the Assistant's
    run until the person approves or denies the call in the chat; a blocked one is not
    offered to the model. The Hub decides every call that is made.
  - *Search and browser use.* The program turns the Assistant's web search off in n8n's
    settings at each start, as the owner (`PUT /rest/instance-ai/settings` with
    `{"searchDisabled": true}`): a person's own n8n has no search provider, and n8n
    counts the Assistant as set up once its model, its sandbox and search are each
    configured or off. `N8N_INSTANCE_AI_BROWSER_USE_ENABLED` is `false`.

## Webhooks

The hooks are the server's `public_routes` under `/n8n/hooks/{path}`, with size and rate
limits. The Hub routes a delivery for a runner to its instance through OpenShell, with
`Host` the public base's host and without cookies. The server passes the path after
`/n8n/hooks/` to the root of n8n's listener on loopback only when it names one of n8n's
webhook or form endpoints, `webhook/`, `webhook-test/`, `webhook-waiting/`, `form/`,
`form-test/` or `form-waiting/`, where n8n serves `webhook/{path}`; n8n's REST and public
API are never reached. It forwards the body, at most 1 MiB, the query and the sender's
headers without the server's own `x-veoveo-` headers or cookies, so no delivery starts a
run, strips cookies from n8n's answer and records the delivery and n8n's status. The
webhook's own signature, which n8n's trigger node checks, verifies the sender. n8n
registers its trigger URLs with outside services itself, under `N8N_WEBHOOK_URL`, through
the integration's provider.

## Platform Contracts

Nothing here is particular to n8n: each contract is one any server uses, designed by
its owner. The server relies on them as follows.

| Contract | Owner |
|---|---|
| A server declares its public routes in `veoveo_surfaces`; the registration response names the instance's `veoveo_public_base`, and the Hub routes them through OpenShell's service exposure ([SDK registration](https://github.com/VeoveoAI/veoveo-hub-sdk/blob/main/crates/veoveo-hub-sdk/src/registration/DESIGN.md#surfaces)) | [Hub](https://github.com/VeoveoAI/veoveo-hub/blob/main/crates/veoveo-hub/DESIGN.md), [installation](https://github.com/VeoveoAI/veoveo-template-installation/blob/main/DESIGN.md) |
| A branch's top-level Lore links run as runners: the parent's program in `/sandbox/src` over the linked repository's tracked branch at `/sandbox/link`, with the linked repository's network rules and providers, URIs `{slug}://{link}/…`, tools `{slug}__{link}__{tool}`, each runner's own database, and the caller's grant on the linked repository in the forwarded token | [Hub](https://github.com/VeoveoAI/veoveo-hub/blob/main/crates/veoveo-hub/DESIGN.md), [SDK registration](https://github.com/VeoveoAI/veoveo-hub-sdk/blob/main/crates/veoveo-hub-sdk/src/registration/DESIGN.md) |
| The Hub is the installation's MCP Apps host for browsers: `/apps/{slug}/{page}` on the public host opens `ui://{slug}/{page}` for the signed-in person; self-contained apps run in opaque frames inside the Hub's sandbox proxy; a view of a linked server reads `{slug}://{link}/…` and calls `{link}__{tool}`; the host answers a view's `resources/list` with the view's own server's resources the person may read, from which the overview finds the links; and `ui/open-link` opens a program of the person's own Computer under `/computer/apps/` top-level | [Hub](https://github.com/VeoveoAI/veoveo-hub/blob/main/crates/veoveo-hub/DESIGN.md), [SDK Apps](https://github.com/VeoveoAI/veoveo-hub-sdk/blob/main/crates/veoveo-hub-sdk/src/apps/DESIGN.md) |
| A person's Computer runs `veoveo-n8n-computer` as its program `n8n`, whose `open?repository=&workflow=` clones the team's repository, applies its workflows and opens one in the editor; the Hub forwards `/computer/c/{computer}/apps/n8n/…` to its port with the person's admission, without their cookies or credentials, and the Computer policy's `hub` rule names its binary | [Hub](https://github.com/VeoveoAI/veoveo-hub/blob/main/crates/veoveo-hub/DESIGN.md), [installation](https://github.com/VeoveoAI/veoveo-template-installation/blob/main/DESIGN.md) |
| A runner's start command gives the parent's program the tracked branch's Lore ID as `VEOVEO_TRACKED_BRANCH`, which `n8n://status` names | [Hub](https://github.com/VeoveoAI/veoveo-hub/blob/main/crates/veoveo-hub/DESIGN.md) |
| A team's repository requests installation provider profiles in `openshell/providers.yaml`; the Hub attaches to its runner those the control plane grants to that repository, with their refresh, and a person connects an account through the Hub | [Hub](https://github.com/VeoveoAI/veoveo-hub/blob/main/crates/veoveo-hub/DESIGN.md), [installation](https://github.com/VeoveoAI/veoveo-template-installation/blob/main/DESIGN.md) |
| A factory instance reaches the model endpoint with a provider the installation attaches | [installation](https://github.com/VeoveoAI/veoveo-template-installation/blob/main/DESIGN.md) |
| A credential-bound endpoint can bind strictly: OpenShell admits a request there only when its credential carries the bound placeholder | [OpenShell fork](https://github.com/VeoveoAI/OpenShell) |

## Persistence

| Table | Key | Retention |
|---|---|---|
| `n8n.applied_workflow` | `(tenant, workflow)` | While the workflow's file exists on the team's tracked branch |
| `n8n.instance_key` | `(tenant)` | For the life of n8n's data in the schema, which the key decrypts |
| `n8n.run` | `(tenant, task_id)` | While the run's Task runs; the worker deletes it when the Task ends |
| `n8n.run_execution` | `(tenant, instance_id, execution_id)` | While n8n keeps the execution, 14 days by its default pruning; each start deletes older rows and those of another n8n instance |
| `n8n.n8n_*` | n8n's own | n8n's; the server's statements never touch these tables |

A run records its workflow, its operation, n8n's execution ID and the SHA-256 of its run
token, by which the bridge finds it; its row lasts while its Task runs. A dispatched
execution records its Task and the Task's caller, which the data rule reads. Each of
the server's statements binds the tenant and follows the template's
[persistence](https://github.com/VeoveoAI/veoveo-template-mcp-rust/blob/main/crates/veoveo-template-mcp-rust/DESIGN.md#persistence)
rules. n8n's tables are upstream's, created and changed only by n8n's migrations.

## Audit And Usage

| Class | OCSF | Records |
|---|---|---|
| `n8n.run` | API Activity (6003) | A run's start and outcome |
| `n8n.workflow_apply` | API Activity (6003) | Each workflow created, updated or archived from the team's files |
| `n8n.bridge_call` | API Activity (6003) | Each call n8n made into another server, admitted or refused |
| `n8n.webhook_delivery` | HTTP Activity (4002) | Each public delivery and n8n's answer |

The usage kind `n8n.execution` counts executions in `Units`. Until the server writes
these records through core's audit and usage ledger, each class is a structured log line
that names its class, and no usage is recorded.

## Factory

| Path | Holds |
|---|---|
| `openshell/image/` | The template's Rust image, `rust:1.99.0-trixie` by digest, with Node.js and n8n copied from the VeoveoAI fork's image, built at its pinned revision on `node:26.12.1-trixie-slim` into the installation's registry and named by digest. Both are Debian trixie, so `node`, n8n's native modules and the installation's Lore command line link against one glibc |
| `openshell/run` | Builds the server, then `exec`s it with the team's workflows and instance under `/sandbox/link` and `n8n/admitted.toml`; the Veoveo nodes need no build |
| `openshell/policy.yaml` | The rule `n8n_database`, which admits n8n's program, `/usr/local/bin/node`, to the PostgreSQL host and port of the installation's base rule. A team's integration hosts are in its own policy |

The Hub supplies the deployment, Hub and registration settings as it does for the
template. The root `Dockerfile`'s check and runtime take n8n from the same image, by the
build argument `N8N_IMAGE`, whose default names the installation's registry; the runtime
image is the fork's image with the server beside n8n, and runs it with a team mounted at
`/sandbox/link` or the paths its flags name. A team's push to its tracked branch restarts
its runner, which applies the files; a push of this repository's branch restarts every
runner of it, and one that changes `openshell/image/` or `openshell/policy.yaml` gives
them new sandboxes.

## Qualification

| Test | Holds |
|---|---|
| `src/contract` unit tests | Workflow names, execution IDs and typed addresses |
| `src/instance`, `src/n8n`, `src/workflows` unit tests | The instance and admitted files' admission and a team's use of only what is admitted, n8n's environment without an editor, n8n's reported version, the generated secrets, the credential import document, the workflow write shape and webhook URLs, and each apply decision |
| `src/computer` unit tests | The program's flags, n8n's Assistant environment, sandbox paths and ids, and command output as text across reads |
| `tests/computer.rs` | `veoveo-n8n-computer` against a fake n8n, a fake Hub MCP endpoint, a fake Lore and a fake model provider: `open` clones a team once at its tracked branch, applies tagged drafts and redirects to the workflow; a saved change and a new tagged workflow are written back in the file form; a team without a tracked branch or a repository no team serves opens nothing; `teams` lists them; the editor reaches n8n without the mount as its owner and sets no cookie; the bridge admits only the Computer's token and calls the Hub with the Computer's credential, and lists the tools the Hub lists to the owner by their Hub names to that token alone; the Assistant's MCP endpoint admits only the token in its path and serves only POST, and a real MCP client lists and calls the Hub's tools through it, while a raw `server/discover` with its protocol headers and a cookie reaches the Hub with the Computer's credential instead of the client's and without the cookie, and the Hub's answer comes back; the model endpoint admits only that token and streams the provider's answer to a request it sent with the Computer's model placeholder; the sandbox service admits only that token, keeps a sandbox in a directory of the person's home, runs commands there as the test's user with its own `HOME` and without withheld variables, reads one execution once per `exec_id`, resumes after a `seq`, kills a command at its limit or when deleted, keeps file calls in the sandbox and removes its directory; the start's search call reaches n8n as the owner |
| `tests/team.rs` | The team template and the tests' team read as `n8n/admitted.toml`, which admits all, admits them, and load every node type n8n ships since they name none; the tests' team's credential types are admitted without being named; a team without its optional files uses what the repository admits, and a listed repository's list; under a listed repository, a team node type or credential type not admitted stops the start with a message naming it; the template's rules name n8n's program by its image path |
| `tests/workflows.rs` | Against a real n8n on a migrated database: n8n migrates as the server's login, which may not create in the database, and its tables are prefixed and belong to the group role; n8n reports its version; the key is minted and kept, a credential imported; applying the template's webhook example creates and publishes, keeps, updates, replaces a version saved in n8n and archives by file name, records digests and versions, and its production webhook answers; a stale key in n8n's settings file over a fresh schema starts n8n with the database's key; a restart reads n8n's key from the database and reuses the API key; a new sandbox's empty disk starts with the key the database keeps, a new owner password and a newly minted API key; n8n's tables with no key refuse the start; the template's and the tests' team's workflows apply with the Veoveo nodes and, the team naming no node types, every node type n8n ships: its HTTP Request calls with the bearer credential the team imports, whose type the repository admits without naming it, and sends its placeholder, and `hello-run` runs as a Task to its Veoveo Result's output, its execution recorded as the Task caller's |
| `tests/runs.rs` | Through the SDK's in-process mesh and a fake n8n: a run completes from its Veoveo Result, its execution recorded as its Task caller's, fails and is stopped from n8n's execution through the status check, is cancelled by its caller, which stops the execution, and a restarted worker keeps waiting on the recorded execution; a run's job names its n8n instance, so a new n8n's execution numbers never meet an earlier one's |
| `tests/bridge.rs` | The bridge lists its table's tools to any caller on loopback; it calls only table tools and only with a running run's token, for the run's caller through the mesh's exchange of the run's Task; a caller without the provider's permission gets the Hub's refusal |
| `tests/containment.rs` | n8n listens on loopback only, reads no environment or outside file, loads no excluded node, and a credential-bound endpoint refuses a value that is not its placeholder |
| `tests/mcp.rs` | Through the SDK's test Hub against a fake n8n: discovery lists the overview App, `run` linked to it for the model and the App and no editor; the host's shared admission resolves the App's `{link}__run` to that link's `run` and nothing else; permission refusals and the refusal of another tenant; the workflows and status resources, with the linked repository, the writer flag and the Computer's edit URL from the forwarded grant; the data rule for a run's caller, a writer and a reader, on reads, stops and retries; the hooks reach only n8n's webhooks without the server's headers, and nothing serves n8n's editor |
| `tests/database_conformance.rs`, `tests/sql_confinement.rs` | The template's checks over the server's own tables |
