# n8n MCP Design

This document is the design and operational contract for the `veoveo-mcp-n8n` crate.

The server runs n8n workflows as a Veoveo server. A branch holds its workflows as JSON
files. Its factory sandbox runs n8n beside the server: the server applies the branch's
workflows to n8n, runs them as durable Tasks for their callers, and serves their
executions and n8n's editor through the Hub. n8n stays contained: everything it reaches
goes through OpenShell's proxy or through the server and the Hub. n8n holds no real
credential, no Hub credential and no route of its own.

A repository made from this one is one n8n project. It renames the slug, as a
repository made from the template does, and keeps the structure.

## Service Identity

```text
crate        veoveo-mcp-n8n
binary       veoveo-mcp-n8n
repository   veoveo-mcp-n8n
slug         n8n
URI scheme   n8n
permissions  n8n.workflows.read, n8n.workflows.run, n8n.executions.write, n8n.editor
MCP          /n8n/mcp
editor       https://n8n.apps.{public host}/, the server's own app origin
hooks        /n8n/hooks/{path}
liveness     /livez
readiness    /readyz
port         8811
n8n          127.0.0.1:5678, inside the sandbox only
loopback     127.0.0.1:8812, the Veoveo nodes' run callbacks and the bridge
```

MCP and the hooks are under the server's mount, through the SDK's existing route kinds:
`mcp`, and `public_routes`, which carry no Hub authentication and serve only callers
that verify themselves, as the SDK's operation and provider-event callbacks do. The
editor is n8n's own web application, so it gets an origin of its own: the server's
`authenticated_routes`, which the Hub serves at the root of the server's app origin,
`n8n.apps.{public host}`. n8n's REST, public API and webhook routes stay at the root of
its listener whatever `N8N_PATH` says, so the editor needs the whole origin. In a
factory sandbox the Hub supplies the port and the internal base URL. The server declares
the hooks and the app origin in its registration's `veoveo_surfaces`, with
`n8n.editor` as the `app_origin` requirement; the registration response names
`veoveo_public_base` and `veoveo_app_origin`. The server builds the hooks' public URLs
under the first through `PublicDeployment::with_public_base` and announces the hooks and
the editor in `n8n://editor` and in each workflow's resource.

## Standards And Protocols

| Standard or protocol | Implemented profile |
|---|---|
| [Model Context Protocol](https://modelcontextprotocol.io/specification/) | Version `2026-07-28`, JSON-RPC 2.0 over Streamable HTTP, through `veoveo-hub-sdk` under the hosted MCP contract: tools, resources and templates, and MCP Tasks for runs. No other version. |
| [n8n](https://docs.n8n.io/) | The [VeoveoAI fork](https://github.com/VeoveoAI/n8n) at one pinned revision, carrying only the changes its `VEOVEO_FORK.md` records, and configured only through its documented environment, its command line (`import:credentials`), its public REST API (`/api/v1` with `X-N8N-API-KEY`) and its custom-node extension point (`N8N_CUSTOM_EXTENSIONS`). The server uses n8n's login and API-key routes only to mint its API key, when n8n does not accept the one it kept. n8n's own MCP server and MCP client nodes are not used. |
| n8n workflow JSON | The source of each workflow, in the form n8n exports it |
| [JWT access tokens RFC 9068](https://www.rfc-editor.org/rfc/rfc9068.html), [Token Exchange RFC 8693](https://www.rfc-editor.org/rfc/rfc8693.html) | The Hub forwards each request with a token naming the caller. A run's calls into other servers exchange the run's Task for a token acting for its caller |
| [OpenShell](https://github.com/VeoveoAI/OpenShell) policy and providers | The sandbox policy names each program's endpoints. Integration credentials are providers, and n8n holds only their placeholders |
| MCP Apps SEP-1865 `2026-01-26` | Views of a workflow and its executions at `ui://n8n/…`, under the SDK's [Apps contract](https://github.com/VeoveoAI/veoveo-hub-sdk/blob/main/crates/veoveo-hub-sdk/src/apps/DESIGN.md) |
| [PostgreSQL 18](https://www.postgresql.org/docs/18/) | The schema `n8n` holds the server's tables and n8n's own, which n8n's migrations create under the table prefix `n8n_` |
| [JSON Schema Draft 2020-12](https://json-schema.org/draft/2020-12/) | Tool inputs and results, generated from the contract's types. A run's input and output are the workflow's own JSON |

## Domain Contract

The server answers these questions for a caller:

- Which workflows does this branch serve, and is each one active?
- Run this workflow with this input: what did it produce?
- What happened in this execution? Retry it, or stop it.
- Which workflows changed in the editor since the branch's revision?

| Surface | Name | Permission | Result |
|---|---|---|---|
| Tool | `run` | `n8n.workflows.run` | A Task whose result is the execution, its address and the JSON of the items it produced |
| Tool | `retry_execution`, `stop_execution` | `n8n.executions.write` | The execution |
| Tool | `editor_changes` | `n8n.workflows.read` | Each workflow whose n8n version differs from the branch's file, with its JSON |
| Resource | `n8n://workflows{?limit,cursor}` | `n8n.workflows.read` | A page of workflows: name, whether it is runnable and active, the applied file digest |
| Resource | `n8n://workflow/{workflow}` | `n8n.workflows.read` | The workflow's JSON as applied and the public URL of each of its webhooks |
| Resource | `n8n://editor` | `n8n.editor` | The editor's public URL and the public URL of the hooks |
| Resource | `n8n://executions{?workflow,status,limit,cursor}` | `n8n.workflows.read` | A page of executions |
| Resource | `n8n://execution/{execution_id}` | `n8n.workflows.read` | One execution: status, timing, output or error |
| App | `ui://n8n/workflow`, `ui://n8n/executions` | `n8n.workflows.read` | Interactive views of the two resources |
| App | `ui://n8n/editor` | `n8n.editor` | A self-contained launcher: it reads `n8n://editor`, shows the editor's URL and, on the person's click, opens the server's app origin top-level with MCP Apps' `ui/open-link`. It frames nothing and declares no remote origin |
| Resource | `n8n://docs`, `n8n://docs/{doc_id}`, `n8n://contract` | `n8n.workflows.read` | The embedded documents and the contract declaration |

A workflow is named by its file under `workflows/`, such as `invoice-intake` for
`workflows/invoice-intake.json`, never by n8n's internal ID: lowercase ASCII letters and
digits in groups joined by single hyphens, at most 64 characters, so its order is the same
in Rust and in PostgreSQL's `C` collation. It is runnable when it starts with an enabled
Veoveo Trigger node with a webhook ID, `CUSTOM.veoveoTrigger`: n8n names a node it loads
from `N8N_CUSTOM_EXTENSIONS` under the package `CUSTOM`. The Veoveo Result and Veoveo
Tool are `CUSTOM.veoveoResult` and `CUSTOM.veoveoTool`. `n8n.workflows.run` and `n8n.editor` declare the
risk `sensitive`: a workflow acts on outside services with the installation's
credentials, and the editor changes what workflows do.

One n8n instance is one trust domain: any workflow in it can read what n8n stores for
every other. A branch therefore serves one tenant, the one `n8n/instance.toml` names,
and a caller of another tenant is refused.

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
| `workflows` | Reading `workflows/`, applying it to n8n and recording what is applied |
| `runs` | The Task service of `run`, the worker: the dispatch to n8n, the wait and the result, and n8n's status check and cancellation |
| `bridge` | Calls from n8n's Veoveo Tool node into other servers |
| `loopback` | The server's loopback listener, where the Veoveo Result posts a run's output and the Veoveo Tool calls the bridge |
| `editor` | The editor on the app origin: the owner's n8n session attached to each forwarded request, and the record of who made it |
| `hooks` | Public webhook deliveries to n8n's webhook listener |
| `proxy` | Forwarding a delivered request to n8n's listener, for the hooks and the editor |
| `store` | The server's own tables in the `n8n` schema |
| `server` | Configuration, registration with the Hub, the database login, the process instance and serving |

### Repository Layout

| Path | Holds |
|---|---|
| `crates/veoveo-mcp-n8n/` | The server, its migrations, tests and the documents it serves |
| `nodes/n8n-nodes-veoveo/` | n8n custom nodes in plain JavaScript, in n8n's community-node layout: Veoveo Trigger, Veoveo Result and Veoveo Tool. n8n loads them from `N8N_CUSTOM_EXTENSIONS` with no build step, since the factory image has Node.js but no compiler for TypeScript |
| `workflows/` | The branch's workflows, one JSON file each |
| `n8n/instance.toml` | The instance's tenant, the credential map from n8n credential types to provider placeholders, and the node and credential types the instance admits, as [the instance](#the-instance) describes |
| `openshell/image/`, `openshell/run`, `openshell/policy.yaml` | The [factory layout](#factory) |
| Root files | The template's: the workspace, toolchain, gate, `Dockerfile` and `.sqlx/` |

### Startup

The server reads `n8n/instance.toml` and `workflows/` and refuses a malformed one before
it contacts anything. It builds its registration declaration and registers with the Hub,
which issues its database login, its public base and its app origin; a registration
without either fails the start. It applies its
migrations, then starts n8n as its child process with the
[configuration](#n8n-configuration) below. n8n creates its owner from the environment.
On first start the server generates the owner's password, keeps it under
`/sandbox/state/n8n` and passes n8n only its bcrypt hash. Once n8n answers
`/healthz/readiness`, which follows its migrations, the server checks the API key it
kept beside the password against `/api/v1`. When n8n refuses it, on first start or when
the branch's data environment holds an n8n other than the one the sandbox's disk knew, the
server logs in as the owner at `/rest/login`, mints a new key at `/rest/api-keys` with the
scopes it uses and keeps it. It imports the instance's credentials with
`n8n import:credentials`, applies the branch's workflows through `/api/v1` and serves;
`/readyz` answers only from then on. Beside MCP it runs the Task worker of `run` and the
loopback listener of the Veoveo nodes.

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
| `N8N_EDITOR_BASE_URL`, `N8N_WEBHOOK_URL` | The registration's `veoveo_app_origin` with the root path, and `{veoveo_public_base}/hooks/` | n8n builds, shows and registers URLs on the Hub's routes; a production webhook is `{N8N_WEBHOOK_URL}webhook/{path}`. n8n 2.46 deprecates `WEBHOOK_URL` for `N8N_WEBHOOK_URL`, which sets the base of test and production webhooks alike |
| `N8N_PUSH_BACKEND` | `sse` | The Hub forwards no WebSocket upgrade on an app origin, so the editor's push channel is server-sent events; n8n then turns off its collaboration features |
| `N8N_INSTANCE_OWNER_MANAGED_BY_ENV`, `N8N_INSTANCE_OWNER_EMAIL`, `N8N_INSTANCE_OWNER_PASSWORD_HASH` | `true`, the server's owner address, the bcrypt hash of the generated password | n8n's owner exists from the first start, with no setup step |
| `DB_TYPE`, `DB_POSTGRESDB_*`, `DB_TABLE_PREFIX` | `postgresdb`, the registration's login and schema `n8n`, `n8n_` | n8n's state lives in the installation's PostgreSQL, in the branch's data environment |
| `PGOPTIONS` | `-c role=server_n8n` | Every n8n connection takes the server's group role, so n8n's tables belong to it, as core's database design requires of a server's objects. The fork checks for the schema before `CREATE SCHEMA`, so the login needs no CREATE privilege on the database |
| `DB_POSTGRESDB_SSL_ENABLED`, `DB_POSTGRESDB_SSL_CA_FILE`, `DB_POSTGRESDB_SSL_REJECT_UNAUTHORIZED` | `true`, the bundle OpenShell names in `SSL_CERT_FILE`, `true` | n8n verifies PostgreSQL against the platform roots; n8n reads `DB_POSTGRESDB_SSL_CA` from the file its `_FILE` form names. Node reads only `NODE_EXTRA_CA_CERTS`, which OpenShell sets to the sandbox's interception CA alone. The grant's URL has `sslmode=verify-full`; on a test cluster it has `sslmode=disable` and n8n connects without TLS. Any other mode fails the start |
| `N8N_ENCRYPTION_KEY` | Generated on first start, kept under `/sandbox/state/n8n` | n8n encrypts its credentials, which hold only placeholders |
| `N8N_BLOCK_ENV_ACCESS_IN_NODE`, `N8N_BLOCK_FILE_ACCESS_TO_N8N_FILES`, `N8N_RESTRICT_FILE_ACCESS_TO` | `true`, `true`, `/sandbox/n8n-files` | Workflows read no environment and no file outside one folder |
| `NODES_INCLUDE`, `NODES_EXCLUDE` | The node types `n8n/instance.toml` admits, and n8n's command, SSH, Git, local-file and MCP nodes | n8n loads only the admitted node types, and never the excluded ones, which an instance cannot admit. No node runs commands, reaches past the policy's rules or speaks MCP |
| `N8N_CUSTOM_EXTENSIONS` | The checkout's `nodes/n8n-nodes-veoveo` | The Veoveo nodes, which `NODES_INCLUDE` always admits |
| `N8N_MCP_MANAGED_BY_ENV`, `N8N_MCP_ACCESS_ENABLED` | `true`, `false` | n8n's own MCP server stays off |
| `N8N_COMMUNITY_PACKAGES_ENABLED`, `N8N_DIAGNOSTICS_ENABLED`, `N8N_VERSION_NOTIFICATIONS_ENABLED`, `N8N_TEMPLATES_ENABLED` | `false` | n8n installs and fetches nothing on its own |

n8n's program gets the server's environment without the variables whose names start
with `VEOVEO_`, `N8N_`, `DB_` or `NODES_`, nor `WEBHOOK_URL` or `PGOPTIONS`, and with the
settings above. The server's own configuration and its registration secret stay with it,
and only the server configures n8n. `N8N_PATH` is never set: it moves only the editor's
pages, while n8n's REST, public API and webhook routes stay at the root.

### The Instance

`n8n/instance.toml` declares the instance; an unknown member is refused.

```toml
tenant = "reference"
nodes = ["n8n-nodes-base.webhook", "n8n-nodes-base.set"]

[[credentials]]
id = "githubToken0001"
name = "GitHub"
type = "githubApi"
data = { server = "https://api.github.com", user = "veoveo" }
secrets = { accessToken = "GITHUB_TOKEN" }
```

`tenant` is the one tenant the branch serves. `nodes` lists every node type n8n loads, at
least one; an excluded node type fails the start. Each credential is imported under its
`id`, 1 to 36 ASCII letters and digits, so a workflow names it. `data` holds the fields
that hold no secret; `secrets` maps each secret field, by its path with `.` between the
keys of nested objects such as `oauthTokenData.access_token`, to the provider
environment variable whose placeholder fills it. A field is set once, as data or as a
secret, and a provider variable never starts with `VEOVEO_`. A credential whose
variable no attached provider sets fails the start.

## Containment

### Network

The sandbox policy names two programs. n8n's `node` reaches PostgreSQL, the model
endpoint and each integration host that `openshell/policy.yaml` lists, by exact host,
with REST rules where an API states its intent in method and path. The installation's
base rule for PostgreSQL names only the branch's own build, so the repository's policy
adds the rule that admits n8n's `node` to the database host. The server's binary
reaches the Hub, PostgreSQL and Artifact's object URLs. No rule is a wildcard. The
Veoveo nodes run inside n8n's process and reach the server only on loopback.

### Credentials

Real credentials are OpenShell providers that the installation attaches to the
repository's instances; a repository never adds one. n8n holds only placeholders, and
OpenShell's proxy replaces a placeholder only at the endpoint its provider binds. At
each start the server imports every credential `n8n/instance.toml` maps with
`n8n import:credentials`, whatever its type, with the provider's placeholder in place of
each secret field; n8n encrypts it on import. A credential type that n8n authorizes
through OAuth2 holds the placeholder as its access token, with no expiry, and n8n
refreshes it only when an API answers 401. n8n's credential overwrites are not used:
they fill only empty fields, so they set defaults and enforce nothing. The provider's own refresh
runs in OpenShell's gateway, and no rule admits n8n to a token endpoint, so a refresh n8n
attempts fails and the request fails with it.

Each credential-bound endpoint binds strictly: OpenShell admits a request there only
when its credential carries the bound placeholder. A value someone types into n8n's
editor therefore never leaves the sandbox. Workflow files and `n8n/instance.toml` name
providers' environment names, never values.

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

### Models

n8n's AI nodes call the model endpoint through OpenShell's proxy with the model
provider's placeholder. The Hub is not a model proxy.

## Runs

`run` creates a durable Task for its caller, of the branch's tenant and for a workflow the
branch applied that starts with the Veoveo Trigger; a call without the client's Task
support is told it needs it. The worker dispatches one operation to n8n, with the
operation ID as its idempotency key. It records the run with the SHA-256 of a fresh run
token, then calls the workflow's Veoveo Trigger on loopback, n8n's production webhook
`{webhookId}/run`, with the header `x-veoveo-dispatch: run`, which the hooks never
forward, and a body naming the operation ID, the run token, the operation's callback URL,
the bridge's URL and the input. The trigger answers at once with n8n's execution ID, as
n8n's `onReceived` response mode lets a webhook answer with `$execution.id`, and the
dispatch records it as the operation's job. The Task then parks.

The Veoveo Result node posts the JSON of the items it receives to the callback URL on the
server's loopback listener, `/n8n/operations/{operation_id}/{token}`: the operation's
callback token, which only that execution's data holds, authenticates it, and the server
stores it as the operation's final event. Every operation is also checked: the first
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
`editor_changes` returns each workflow whose n8n version differs from the one the server
applied, with n8n's copy in the members a workflow file holds, and
`n8n://executions{?workflow,status,limit,cursor}` pages n8n's executions newest first.
The listing's cursor is the server's, bound to its filters and limit; a next page asks n8n
for the executions after the cursor's, in n8n's own cursor form, and n8n lists a running
execution only when the status filter asks for `running`.

## Workflows From Lore

At each start the server reads `workflows/*.json`, at most 500 files of at most 4 MiB,
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

The editor can change n8n's copy. `editor_changes` returns each workflow whose current
n8n version differs from the one the server applied, with its JSON, so the caller's
agent writes the file and pushes. Lore owns the source: a start applies the branch's
files even over unsaved editor changes, n8n's workflow history keeps the replaced
versions, and the server logs each one it replaced.

## Editor

n8n's editor is a full web application on the server's app origin, and the App
`ui://n8n/editor` is its entry point: a self-contained view that reads `n8n://editor`,
shows the editor's URL and, when the person clicks, sends `ui/open-link` with the app
origin's root, which the Hub admits as an open-link target only for the App's own
server's app origin and opens top-level, in a new tab where the browser allows. A view
never frames the editor: an App runs in a frame sandboxed `allow-scripts` alone, a nested
frame inherits that sandbox, and a framed app origin therefore holds no session of its
own, which the Hub's 401 to such a frame showed. The Hub serves the app origin: it signs
a person in, checks `n8n.editor` and forwards the browser to the selected instance
through OpenShell with a forwarded token, with `Host` the app origin's host and without
its own cookies; the browser chooses no target. The app origin is n8n's alone, so n8n's
script reaches neither the Hub's origin, nor the sandbox origin of self-contained apps,
nor another server's app.

The editor's routes forward every path of the app origin to n8n's listener on loopback,
streaming both bodies, so the editor's pages, `/rest`, its assets and its server-sent
events reach n8n; a request for another host, such as the mount on the deployment's host,
is not the editor's. The server checks `n8n.editor` again on each request. Editor
requests act in n8n as its owner: the server signs in as the owner with the password it
keeps, holds the session itself, renews it from n8n's answers and signs in again an hour
before it expires or after n8n refused it, and attaches it to each forwarded request in
place of any n8n session the browser sent. It removes the Hub's forwarded token and strips
n8n's cookies from the answers, so the browser never holds an n8n credential. It keeps
n8n's `X-Frame-Options: SAMEORIGIN`: nothing frames the editor, and another server's app
origin is same-site, so without it that origin could frame the editor with the person's
session. The server
records the person behind each request that changes n8n, by the forwarded identity.
Strict binding keeps any value typed into a credential from leaving the sandbox.

## Webhooks

The hooks are the server's `public_routes` under `/n8n/hooks/{path}`, with size and rate
limits. The Hub routes a delivery for the branch to its instance through OpenShell, with
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
| A server declares its public routes and app origin in `veoveo_surfaces`; the registration response names the instance's `veoveo_public_base` and the server's `veoveo_app_origin`, and the Hub routes both through OpenShell's service exposure ([SDK registration](https://github.com/VeoveoAI/veoveo-hub-sdk/blob/main/crates/veoveo-hub-sdk/src/registration/DESIGN.md#surfaces)) | [Hub](https://github.com/VeoveoAI/veoveo-hub/blob/main/crates/veoveo-hub/DESIGN.md), [installation](https://github.com/VeoveoAI/veoveo-template-installation/blob/main/DESIGN.md) |
| The Hub is the installation's MCP Apps host for browsers: `/apps/{slug}/{page}` on the public host opens `ui://{slug}/{page}` for the signed-in person's selected branch; self-contained apps run in opaque frames inside the Hub's sandbox proxy at `sandbox.apps.{public host}`; a server's `authenticated_routes` are served at the root of its own app origin, `{slug}.apps.{public host}`, with a forwarded token, and a view opens that origin top-level with `ui/open-link`, which the Hub admits as a target only for the view's own server's app origin, since a view's sandboxed frame cannot frame it with a session. One wildcard name and certificate, `*.apps.{public host}`, cover them all | [Hub](https://github.com/VeoveoAI/veoveo-hub/blob/main/crates/veoveo-hub/DESIGN.md), [SDK Apps](https://github.com/VeoveoAI/veoveo-hub-sdk/blob/main/crates/veoveo-hub-sdk/src/apps/DESIGN.md), [installation](https://github.com/VeoveoAI/veoveo-template-installation/blob/main/DESIGN.md) |
| A branch requests installation provider profiles in `openshell/providers.yaml`; the Hub attaches those the control plane grants, with their refresh, and a person connects an account through the Hub | [Hub](https://github.com/VeoveoAI/veoveo-hub/blob/main/crates/veoveo-hub/DESIGN.md), [installation](https://github.com/VeoveoAI/veoveo-template-installation/blob/main/DESIGN.md) |
| A factory instance reaches the model endpoint with a provider the installation attaches | [installation](https://github.com/VeoveoAI/veoveo-template-installation/blob/main/DESIGN.md) |
| A credential-bound endpoint can bind strictly: OpenShell admits a request there only when its credential carries the bound placeholder | [OpenShell fork](https://github.com/VeoveoAI/OpenShell) |

## Persistence

| Table | Key | Retention |
|---|---|---|
| `n8n.applied_workflow` | `(tenant, workflow)` | While the workflow's file exists on the branch |
| `n8n.run` | `(tenant, task_id)` | While the run's Task runs; the worker deletes it when the Task ends |
| `n8n.n8n_*` | n8n's own | n8n's; the server's statements never touch these tables |

A run records its workflow, its operation, n8n's execution ID and the SHA-256 of its run
token, by which the bridge finds it; its row lasts while its Task runs. Each of
the server's statements binds the tenant and follows the template's
[persistence](https://github.com/VeoveoAI/veoveo-template-mcp-rust/blob/main/crates/veoveo-template-mcp-rust/DESIGN.md#persistence)
rules. n8n's tables are upstream's, created and changed only by n8n's migrations.

## Audit And Usage

| Class | OCSF | Records |
|---|---|---|
| `n8n.run` | API Activity (6003) | A run's start and outcome |
| `n8n.workflow_apply` | API Activity (6003) | Each workflow created, updated or archived from the branch |
| `n8n.bridge_call` | API Activity (6003) | Each call n8n made into another server, admitted or refused |
| `n8n.webhook_delivery` | HTTP Activity (4002) | Each public delivery and n8n's answer |
| `n8n.editor_request` | API Activity (6003) | Each editor request that changes n8n, with the person who made it |

The usage kind `n8n.execution` counts executions in `Units`. Until the server writes
these records through core's audit and usage ledger, each class is a structured log line
that names its class, and no usage is recorded.

## Factory

| Path | Holds |
|---|---|
| `openshell/image/` | The template's Rust image, `rust:1.99.0-trixie` by digest, with Node.js and n8n copied from the VeoveoAI fork's image, built at its pinned revision on `node:26.12.1-trixie-slim` into the installation's registry and named by digest. Both are Debian trixie, so `node`, n8n's native modules and the installation's Lore command line link against one glibc |
| `openshell/run` | Builds the server, then `exec`s it; the Veoveo nodes need no build |
| `openshell/policy.yaml` | The rule `n8n_database`, which admits n8n's program, `/usr/local/bin/node`, to the PostgreSQL host and port of the installation's base rule, and the integration hosts n8n's program reaches, by exact host |
| `openshell/providers.yaml` | `vv-factory-model` and the integration profiles the workflows use |

The Hub supplies the deployment, Hub and registration settings as it does for the
template. The root `Dockerfile`'s check and runtime take n8n from the same image, by the
build argument `N8N_IMAGE`, whose default names the installation's registry; the runtime
image is the fork's image with the server beside n8n. A push that changes `workflows/` restarts the instance, which applies the
files; a push that changes `openshell/image/` or `openshell/policy.yaml` gives the branch
a new sandbox.

## Qualification

| Test | Holds |
|---|---|
| `src/contract` unit tests | Workflow names, execution IDs and typed addresses |
| `src/instance`, `src/n8n`, `src/workflows` unit tests | The instance file's admission, n8n's environment, the generated secrets, the credential import document, the workflow write shape and webhook URLs, and each apply decision |
| `tests/workflows.rs` | Against a real n8n on a migrated database: n8n migrates as the server's login, which may not create in the database, and its tables are prefixed and belong to the group role; the key is minted and kept, a credential imported; applying `workflows/` creates and publishes, keeps, updates, replaces an editor's version and archives by file name, records digests and versions, and the example's production webhook answers; a restart reuses the key; the branch's workflows apply with the Veoveo nodes, `hello-run` runs as a Task to its Veoveo Result's output, and the editor serves n8n's pages and owner session through the proxy |
| `tests/runs.rs` | Through the SDK's in-process mesh and a fake n8n: a run completes from its Veoveo Result, fails and is stopped from n8n's execution through the status check, is cancelled by its caller, which stops the execution, and a restarted worker keeps waiting on the recorded execution |
| `tests/bridge.rs` | Only table tools and only a running run's token, for the run's caller through the mesh's exchange of the run's Task; a caller without the provider's permission gets the Hub's refusal |
| `tests/containment.rs` | n8n listens on loopback only, reads no environment or outside file, loads no excluded node, and a credential-bound endpoint refuses a value that is not its placeholder |
| `tests/mcp.rs` | Discovery, permission refusals, the refusal of another tenant, the workflows, executions and editor resources and the editor's App through the SDK's test Hub; the hooks reach only n8n's webhooks without the server's headers, and the editor acts as n8n's owner on the app origin alone and keeps n8n's frame refusal, against a fake n8n; the editor's App opens the origin with `ui/open-link` and frames nothing |
| `tests/database_conformance.rs`, `tests/sql_confinement.rs` | The template's checks over the server's own tables |
