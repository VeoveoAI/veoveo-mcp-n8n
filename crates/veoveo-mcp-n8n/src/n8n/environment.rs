//! n8n's configuration, which reaches n8n only through its documented environment.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use secrecy::{ExposeSecret, SecretString};
use url::Url;

use crate::instance::{EXCLUDED_NODES, NodeType};

/// The Veoveo nodes' types, which every instance loads.
pub const VEOVEO_NODES: &[&str] = &[
    super::VEOVEO_TRIGGER,
    super::VEOVEO_RESULT,
    super::VEOVEO_TOOL,
];

/// n8n's loopback listener. Only processes in the sandbox reach it.
pub const LISTEN_ADDRESS: &str = "127.0.0.1";
pub const PORT: u16 = 5678;
/// The owner n8n creates from the environment; the address receives no mail.
pub const OWNER_EMAIL: &str = "owner@n8n.veoveo.invalid";
/// The prefix of n8n's tables in the server's schema.
pub const TABLE_PREFIX: &str = "n8n_";
/// The bcrypt cost of the owner's password hash.
const BCRYPT_COST: u32 = 10;

/// What n8n needs to reach the installation's PostgreSQL as the server's login.
pub struct N8nDatabase {
    host: String,
    port: u16,
    database: String,
    user: String,
    password: SecretString,
    tls: DatabaseTls,
}

/// How n8n's connections verify PostgreSQL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatabaseTls {
    /// `sslmode=disable`: a test cluster on loopback.
    Disabled,
    /// `sslmode=verify-full`: verified against the platform roots in this bundle, the one
    /// OpenShell names in `SSL_CERT_FILE`.
    Verified { roots: PathBuf },
}

impl N8nDatabase {
    /// The database of the registration's grant: its `url`, `user` and `password`. The
    /// URL's `sslmode` is `verify-full`, verified against `roots`, or `disable`.
    pub fn from_grant(
        url: &Url,
        user: &str,
        password: SecretString,
        roots: Option<&Path>,
    ) -> Result<Self> {
        let host = url
            .host_str()
            .filter(|host| !host.is_empty())
            .context("the database URL names no host")?
            .to_owned();
        let database = url
            .path()
            .strip_prefix('/')
            .filter(|database| !database.is_empty() && !database.contains('/'))
            .context("the database URL names no database")?
            .to_owned();
        let mode = url
            .query_pairs()
            .find(|(name, _)| name == "sslmode")
            .map(|(_, value)| value.into_owned());
        let tls = match mode.as_deref() {
            Some("disable") => DatabaseTls::Disabled,
            Some("verify-full") => DatabaseTls::Verified {
                roots: roots
                    .context("sslmode=verify-full needs the platform roots in SSL_CERT_FILE")?
                    .to_owned(),
            },
            other => bail!(
                "the database URL's sslmode is {other:?}; n8n connects with verify-full or, on a test cluster, disable"
            ),
        };
        Ok(Self {
            host,
            port: url.port().unwrap_or(5432),
            database,
            user: user.to_owned(),
            password,
            tls,
        })
    }
}

/// Everything n8n's environment is built from.
pub struct N8nSettings {
    /// n8n's command line program, `n8n` on `PATH` in the factory image.
    pub program: PathBuf,
    /// n8n's user folder, under the server's state directory.
    pub user_folder: PathBuf,
    /// The one folder workflows may read and write files in.
    pub files: PathBuf,
    pub database: N8nDatabase,
    /// The server's schema and its group role, which n8n's connections take.
    pub schema: String,
    pub group_role: String,
    /// `N8N_EDITOR_BASE_URL`: the root of the server's app origin.
    pub editor_url: Url,
    /// `N8N_WEBHOOK_URL`: the public URL of the server's hooks.
    pub webhook_url: Url,
    /// The node types the instance admits.
    pub nodes: Vec<NodeType>,
    /// The Veoveo nodes' folder, `N8N_CUSTOM_EXTENSIONS`.
    pub veoveo_nodes: PathBuf,
}

/// The secrets the environment carries, kept under the state directory.
pub struct N8nSecrets {
    pub encryption_key: SecretString,
    pub owner_password_hash: SecretString,
}

impl N8nSecrets {
    /// The secrets for `encryption_key` and the owner's `password`, which n8n receives
    /// only as its bcrypt hash.
    pub fn new(encryption_key: SecretString, password: &SecretString) -> Result<Self> {
        let hash = bcrypt::hash(password.expose_secret(), BCRYPT_COST)
            .context("hashing the owner's password")?;
        Ok(Self {
            encryption_key,
            owner_password_hash: SecretString::from(hash),
        })
    }
}

/// Inherited variables whose prefix belongs to the server or to n8n: the server's own
/// configuration and secrets stay with it, and only the settings below configure n8n.
pub const WITHHELD_PREFIXES: &[&str] = &["VEOVEO_", "N8N_", "DB_", "NODES_"];

impl N8nSettings {
    /// n8n's base URL on loopback.
    pub fn base_url() -> Url {
        Url::parse(&format!("http://{LISTEN_ADDRESS}:{PORT}/")).expect("n8n's loopback URL")
    }

    /// n8n's environment. The values that are secrets are the database password, the
    /// encryption key and the owner's password hash.
    pub fn environment(&self, secrets: &N8nSecrets) -> Vec<(&'static str, String)> {
        let database = &self.database;
        let mut environment = vec![
            // n8n runs as in production wherever it runs: without NODE_ENV it takes itself
            // for a development build and drops its own security headers.
            ("NODE_ENV", "production".to_owned()),
            ("N8N_LISTEN_ADDRESS", LISTEN_ADDRESS.to_owned()),
            ("N8N_PORT", PORT.to_string()),
            ("N8N_USER_FOLDER", self.user_folder.display().to_string()),
            ("N8N_ENFORCE_SETTINGS_FILE_PERMISSIONS", "true".to_owned()),
            ("N8N_EDITOR_BASE_URL", self.editor_url.to_string()),
            ("N8N_WEBHOOK_URL", self.webhook_url.to_string()),
            // The Hub forwards no WebSocket upgrade on an app origin.
            ("N8N_PUSH_BACKEND", "sse".to_owned()),
            ("N8N_INSTANCE_OWNER_MANAGED_BY_ENV", "true".to_owned()),
            ("N8N_INSTANCE_OWNER_EMAIL", OWNER_EMAIL.to_owned()),
            (
                "N8N_INSTANCE_OWNER_PASSWORD_HASH",
                secrets.owner_password_hash.expose_secret().to_owned(),
            ),
            ("DB_TYPE", "postgresdb".to_owned()),
            ("DB_POSTGRESDB_HOST", database.host.clone()),
            ("DB_POSTGRESDB_PORT", database.port.to_string()),
            ("DB_POSTGRESDB_DATABASE", database.database.clone()),
            ("DB_POSTGRESDB_USER", database.user.clone()),
            (
                "DB_POSTGRESDB_PASSWORD",
                database.password.expose_secret().to_owned(),
            ),
            ("DB_POSTGRESDB_SCHEMA", self.schema.clone()),
            ("DB_TABLE_PREFIX", TABLE_PREFIX.to_owned()),
            // n8n's PostgreSQL client reads PGOPTIONS, so every connection takes the
            // server's group role and n8n's tables belong to it.
            ("PGOPTIONS", format!("-c role={}", self.group_role)),
            (
                "N8N_ENCRYPTION_KEY",
                secrets.encryption_key.expose_secret().to_owned(),
            ),
            ("N8N_BLOCK_ENV_ACCESS_IN_NODE", "true".to_owned()),
            ("N8N_BLOCK_FILE_ACCESS_TO_N8N_FILES", "true".to_owned()),
            (
                "N8N_RESTRICT_FILE_ACCESS_TO",
                self.files.display().to_string(),
            ),
            (
                "NODES_INCLUDE",
                json_list(
                    self.nodes
                        .iter()
                        .map(NodeType::as_str)
                        .chain(VEOVEO_NODES.iter().copied()),
                ),
            ),
            (
                "N8N_CUSTOM_EXTENSIONS",
                self.veoveo_nodes.display().to_string(),
            ),
            ("NODES_EXCLUDE", json_list(EXCLUDED_NODES.iter().copied())),
            ("N8N_MCP_MANAGED_BY_ENV", "true".to_owned()),
            ("N8N_MCP_ACCESS_ENABLED", "false".to_owned()),
            ("N8N_COMMUNITY_PACKAGES_ENABLED", "false".to_owned()),
            ("N8N_DIAGNOSTICS_ENABLED", "false".to_owned()),
            ("N8N_VERSION_NOTIFICATIONS_ENABLED", "false".to_owned()),
            ("N8N_TEMPLATES_ENABLED", "false".to_owned()),
        ];
        match &database.tls {
            DatabaseTls::Disabled => {
                environment.push(("DB_POSTGRESDB_SSL_ENABLED", "false".to_owned()));
            }
            DatabaseTls::Verified { roots } => {
                environment.push(("DB_POSTGRESDB_SSL_ENABLED", "true".to_owned()));
                // n8n reads a variable's value from the file its `_FILE` form names.
                environment.push(("DB_POSTGRESDB_SSL_CA_FILE", roots.display().to_string()));
                environment.push(("DB_POSTGRESDB_SSL_REJECT_UNAUTHORIZED", "true".to_owned()));
            }
        }
        environment
    }

    /// A command for n8n's program with `arguments` and n8n's environment: the
    /// server's own, without the withheld variables, and the settings above.
    pub fn command(&self, secrets: &N8nSecrets, arguments: &[&str]) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(&self.program);
        command.args(arguments);
        for (name, _) in std::env::vars_os() {
            if let Some(name) = name.to_str()
                && WITHHELD_PREFIXES
                    .iter()
                    .any(|prefix| name.starts_with(prefix))
            {
                command.env_remove(name);
            }
        }
        command.env_remove("WEBHOOK_URL").env_remove("PGOPTIONS");
        command.envs(self.environment(secrets));
        command
    }
}

fn json_list<'a>(values: impl Iterator<Item = &'a str>) -> String {
    serde_json::to_string(&values.collect::<Vec<_>>()).expect("a list of strings serializes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn settings(url: &str, roots: Option<&Path>) -> Result<N8nSettings> {
        let database = N8nDatabase::from_grant(
            &Url::parse(url).unwrap(),
            "n8n_ab12cd34",
            SecretString::from("database-password"),
            roots,
        )?;
        Ok(N8nSettings {
            program: "n8n".into(),
            user_folder: "/sandbox/state/n8n/home".into(),
            files: "/sandbox/n8n-files".into(),
            database,
            schema: "n8n".to_owned(),
            group_role: "server_n8n".to_owned(),
            editor_url: Url::parse("https://n8n.apps.veoveo.example/").unwrap(),
            webhook_url: Url::parse(
                "https://veoveo.example/instances/019a5b30-0000-7000-8000-000000000001/n8n/hooks/",
            )
            .unwrap(),
            nodes: vec![
                NodeType::parse("n8n-nodes-base.webhook").unwrap(),
                NodeType::parse("n8n-nodes-base.set").unwrap(),
            ],
            veoveo_nodes: "/sandbox/src/nodes/n8n-nodes-veoveo".into(),
        })
    }

    fn environment(settings: &N8nSettings) -> BTreeMap<&'static str, String> {
        let secrets = N8nSecrets {
            encryption_key: SecretString::from("encryption-key"),
            owner_password_hash: SecretString::from("$2b$10$hash"),
        };
        settings.environment(&secrets).into_iter().collect()
    }

    #[test]
    fn n8n_listens_on_loopback_and_stores_its_state_in_the_servers_schema() {
        let roots = Path::new("/etc/openshell-tls/ca-bundle.pem");
        let settings = settings(
            "postgres://veoveo-postgres-rw.veoveo-system.svc.cluster.local:5432/veoveo?sslmode=verify-full",
            Some(roots),
        )
        .unwrap();
        let environment = environment(&settings);
        for (name, value) in [
            ("NODE_ENV", "production"),
            ("N8N_LISTEN_ADDRESS", "127.0.0.1"),
            ("N8N_PORT", "5678"),
            ("N8N_PUSH_BACKEND", "sse"),
            ("N8N_EDITOR_BASE_URL", "https://n8n.apps.veoveo.example/"),
            (
                "N8N_WEBHOOK_URL",
                "https://veoveo.example/instances/019a5b30-0000-7000-8000-000000000001/n8n/hooks/",
            ),
            ("DB_TYPE", "postgresdb"),
            (
                "DB_POSTGRESDB_HOST",
                "veoveo-postgres-rw.veoveo-system.svc.cluster.local",
            ),
            ("DB_POSTGRESDB_PORT", "5432"),
            ("DB_POSTGRESDB_DATABASE", "veoveo"),
            ("DB_POSTGRESDB_USER", "n8n_ab12cd34"),
            ("DB_POSTGRESDB_SCHEMA", "n8n"),
            ("DB_TABLE_PREFIX", "n8n_"),
            ("PGOPTIONS", "-c role=server_n8n"),
            ("DB_POSTGRESDB_SSL_ENABLED", "true"),
            (
                "DB_POSTGRESDB_SSL_CA_FILE",
                "/etc/openshell-tls/ca-bundle.pem",
            ),
            ("DB_POSTGRESDB_SSL_REJECT_UNAUTHORIZED", "true"),
            ("N8N_INSTANCE_OWNER_MANAGED_BY_ENV", "true"),
            ("N8N_INSTANCE_OWNER_EMAIL", OWNER_EMAIL),
            ("N8N_BLOCK_ENV_ACCESS_IN_NODE", "true"),
            ("N8N_BLOCK_FILE_ACCESS_TO_N8N_FILES", "true"),
            ("N8N_RESTRICT_FILE_ACCESS_TO", "/sandbox/n8n-files"),
            ("N8N_USER_FOLDER", "/sandbox/state/n8n/home"),
            (
                "NODES_INCLUDE",
                r#"["n8n-nodes-base.webhook","n8n-nodes-base.set","CUSTOM.veoveoTrigger","CUSTOM.veoveoResult","CUSTOM.veoveoTool"]"#,
            ),
            (
                "N8N_CUSTOM_EXTENSIONS",
                "/sandbox/src/nodes/n8n-nodes-veoveo",
            ),
            ("N8N_MCP_MANAGED_BY_ENV", "true"),
            ("N8N_MCP_ACCESS_ENABLED", "false"),
            ("N8N_COMMUNITY_PACKAGES_ENABLED", "false"),
            ("N8N_DIAGNOSTICS_ENABLED", "false"),
            ("N8N_VERSION_NOTIFICATIONS_ENABLED", "false"),
            ("N8N_TEMPLATES_ENABLED", "false"),
        ] {
            assert_eq!(
                environment.get(name).map(String::as_str),
                Some(value),
                "{name}"
            );
        }
        let excluded: Vec<String> = serde_json::from_str(&environment["NODES_EXCLUDE"]).unwrap();
        for node in [
            "n8n-nodes-base.executeCommand",
            "n8n-nodes-base.ssh",
            "n8n-nodes-base.localFileTrigger",
            "@n8n/n8n-nodes-langchain.mcpClientTool",
        ] {
            assert!(excluded.iter().any(|excluded| excluded == node), "{node}");
        }
        // n8n's path setting would move only the editor, so the server never sets it, nor
        // the deprecated WEBHOOK_URL.
        assert!(!environment.contains_key("N8N_PATH"));
        assert!(!environment.contains_key("WEBHOOK_URL"));
    }

    #[test]
    fn a_test_cluster_connects_without_tls_and_nothing_else_does() {
        let test = settings("postgres://127.0.0.1:55432/t_1?sslmode=disable", None).unwrap();
        let environment = environment(&test);
        assert_eq!(environment["DB_POSTGRESDB_SSL_ENABLED"], "false");
        assert_eq!(environment["DB_POSTGRESDB_PORT"], "55432");
        assert!(!environment.contains_key("DB_POSTGRESDB_SSL_CA_FILE"));
        for url in [
            "postgres://db.example/veoveo",
            "postgres://db.example/veoveo?sslmode=require",
            "postgres://db.example/veoveo?sslmode=prefer",
            "postgres://db.example/?sslmode=disable",
        ] {
            assert!(
                settings(url, Some(Path::new("/roots.pem"))).is_err(),
                "{url}"
            );
        }
        let error = settings("postgres://db.example/veoveo?sslmode=verify-full", None)
            .err()
            .unwrap();
        assert!(error.to_string().contains("SSL_CERT_FILE"), "{error}");
    }

    #[test]
    fn the_owner_password_reaches_n8n_only_as_a_bcrypt_hash() {
        let password = SecretString::from("generated-password");
        let secrets = N8nSecrets::new(SecretString::from("key"), &password).unwrap();
        let hash = secrets.owner_password_hash.expose_secret();
        assert!(hash.starts_with("$2b$10$") && hash.len() == 60, "{hash}");
        assert!(bcrypt::verify("generated-password", hash).unwrap());
        assert!(!hash.contains("generated-password"));
    }
}
