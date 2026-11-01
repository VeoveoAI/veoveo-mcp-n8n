//! A runner's start reads what the n8n repository admits and the team's instance and
//! workflows before it contacts anything: the template for teams and the tests' team read
//! as the repository, which admits every node and credential type, admits them, and load
//! every node type n8n ships since they name none; a team without its optional files
//! reads; and under a repository that lists what it admits, a team whose instance uses a
//! node type the list does not name stops the start with a message that names it. The
//! template's network rules name n8n's program by its path in the image. Needs no
//! database.
use std::path::{Path, PathBuf};

use veoveo_mcp_n8n::{instance::Admits, server::read_team};

const REPOSITORY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn admitted() -> PathBuf {
    Path::new(REPOSITORY).join("n8n/admitted.toml")
}

fn team(root: &Path) -> anyhow::Result<veoveo_mcp_n8n::server::Team> {
    team_under(&admitted(), root)
}

fn team_under(admitted: &Path, root: &Path) -> anyhow::Result<veoveo_mcp_n8n::server::Team> {
    read_team(
        admitted,
        &root.join("n8n/instance.toml"),
        &root.join("workflows"),
    )
}

fn names(team: &veoveo_mcp_n8n::server::Team) -> Vec<String> {
    let mut names: Vec<_> = team
        .files
        .iter()
        .map(|file| file.workflow.to_string())
        .collect();
    names.sort();
    names
}

#[test]
fn the_template_and_the_tests_team_read_as_the_repository_admits_them() {
    let template = team(&Path::new(REPOSITORY).join("templates/team")).unwrap();
    assert_eq!(names(&template), ["hello-run", "hello-veoveo"]);
    assert_eq!(template.instance.tenant.as_str(), "reference");
    assert!(template.instance.credentials.is_empty());
    assert_eq!(template.admitted.nodes, Admits::All);
    assert_eq!(template.admitted.credentials, Admits::All);
    // Naming no node types, the template loads every one n8n ships but the excluded ones.
    assert!(template.instance.nodes.is_none());
    assert_eq!(template.instance.node_types(&template.admitted), None);
    let tests = team(&Path::new(FIXTURES).join("team")).unwrap();
    assert_eq!(
        names(&tests),
        ["http-bearer", "resolve-time", "slow-run", "undeclared-tool"]
    );
    assert_eq!(tests.instance.tenant.as_str(), "tenant-a");
    assert_eq!(tests.instance.node_types(&tests.admitted), None);
    // Its credentials' types are admitted though the repository names none.
    let types: Vec<&str> = tests
        .instance
        .credentials
        .iter()
        .map(|credential| credential.credential_type.as_str())
        .collect();
    assert_eq!(types, ["httpHeaderAuth", "httpBearerAuth"]);
}

#[test]
fn a_team_without_its_optional_files_uses_what_the_repository_admits() {
    let bare = team(&Path::new(FIXTURES).join("bare-team")).unwrap();
    assert!(bare.files.is_empty());
    assert!(bare.instance.nodes.is_none());
    assert_eq!(bare.instance.node_types(&bare.admitted), None);
    assert!(bare.instance.credentials.is_empty());
    // Under a repository that lists node types, the same team loads that list.
    let narrow = team_under(&narrow(), &Path::new(FIXTURES).join("bare-team")).unwrap();
    assert_eq!(
        narrow
            .instance
            .node_types(&narrow.admitted)
            .map(|nodes| nodes.len()),
        Some(7)
    );
}

fn narrow() -> PathBuf {
    Path::new(FIXTURES).join("admitted-narrow.toml")
}

#[test]
fn a_node_type_the_repository_does_not_admit_stops_the_start() {
    // The repository's open admission admits the team's HTTP Request node.
    let open = team(&Path::new(FIXTURES).join("unadmitted-team")).unwrap();
    assert_eq!(
        open.instance
            .node_types(&open.admitted)
            .map(|nodes| nodes.len()),
        Some(2)
    );
    let error = team_under(&narrow(), &Path::new(FIXTURES).join("unadmitted-team"))
        .err()
        .expect("a team the repository does not admit");
    assert_eq!(
        error.to_string(),
        "the team's n8n/instance.toml uses the node type n8n-nodes-base.httpRequest, which the \
         n8n repository's n8n/admitted.toml does not admit: ask its owners to admit it, or \
         stop using it"
    );
    // A team without its instance names no tenant, and does not start either.
    // Nor, under the listed repository, the tests' team's credential types.
    let error = team_under(&narrow(), &Path::new(FIXTURES).join("team"))
        .err()
        .expect("credential types the list does not admit");
    assert!(
        error.to_string().contains("is of type httpHeaderAuth"),
        "{error}"
    );
    let missing = team(&Path::new(FIXTURES).join("no-such-team"))
        .err()
        .expect("a team without n8n/instance.toml");
    assert!(
        missing.to_string().contains("n8n/instance.toml"),
        "{missing}"
    );
}

#[test]
fn the_templates_rules_name_n8n_s_program_by_its_image_path() {
    let policy =
        std::fs::read_to_string(Path::new(REPOSITORY).join("templates/team/openshell/policy.yaml"))
            .unwrap();
    let binaries: Vec<&str> = policy
        .lines()
        .filter_map(|line| line.trim_start_matches(['#', ' ']).strip_prefix("- path: "))
        .collect();
    assert_eq!(binaries, ["/usr/local/bin/node"]);
    // The repository's own image installs n8n's program there.
    let image =
        std::fs::read_to_string(Path::new(REPOSITORY).join("openshell/image/Dockerfile")).unwrap();
    assert!(image.contains("COPY --from=n8n /usr/local/bin/node /usr/local/bin/node"));
}
