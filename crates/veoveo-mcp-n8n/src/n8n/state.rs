//! What the server keeps for n8n on the sandbox's persistent disk, under
//! `/sandbox/state/n8n`: the API key the server minted, and n8n's own user folder. Nothing
//! here outlives a new sandbox, so nothing here is needed to read n8n's data: n8n's
//! encryption key lives in the database with that data, the server writes it into n8n's
//! settings file before every start, and the owner's password is new at every start. Each
//! secret is a file of mode 0600 in a directory of mode 0700.
use std::{
    fs,
    io::{self, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use secrecy::{ExposeSecret, SecretString};

const API_KEY: &str = "api-key";
const USER_FOLDER: &str = "home";
const CREDENTIALS: &str = "credentials.json";

/// The server's state directory for n8n.
#[derive(Debug, Clone)]
pub struct StateDirectory {
    root: PathBuf,
}

impl StateDirectory {
    /// The directory at `root`, created with mode 0700 when it does not exist.
    pub fn prepare(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&root)?;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        let state = Self { root };
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(state.user_folder())?;
        Ok(state)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// n8n's `N8N_USER_FOLDER`, where n8n keeps its own settings file.
    pub fn user_folder(&self) -> PathBuf {
        self.root.join(USER_FOLDER)
    }

    /// The plaintext credentials file for one `import:credentials`, removed after it.
    pub fn credentials_file(&self) -> PathBuf {
        self.root.join(CREDENTIALS)
    }

    /// n8n's own settings file in its user folder.
    pub fn n8n_settings_file(&self) -> PathBuf {
        self.user_folder().join(".n8n").join("config")
    }

    /// Make n8n's settings file name `key`, the database's, before n8n starts: n8n refuses
    /// to start when the file names another key than the one it is given, as a disk that
    /// outlived a reset of n8n's data would. The file's other fields stay; a missing file
    /// is created, readable by its user alone, as n8n requires.
    pub fn align_n8n_settings(&self, key: &SecretString) -> io::Result<()> {
        let folder = self.user_folder().join(".n8n");
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&folder)?;
        let file = self.n8n_settings_file();
        let mut settings = match read_optional(&file)? {
            Some(text) => match serde_json::from_str(&text) {
                Ok(serde_json::Value::Object(settings)) => settings,
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("{} is not n8n's settings object", file.display()),
                    ));
                }
            },
            None => serde_json::Map::new(),
        };
        settings.insert(
            "encryptionKey".to_owned(),
            serde_json::Value::String(key.expose_secret().to_owned()),
        );
        let text = serde_json::to_string_pretty(&serde_json::Value::Object(settings))
            .map_err(io::Error::other)?;
        write_private_at(&folder, "config", &text)
    }

    /// The API key the server minted, when it has one.
    pub fn api_key(&self) -> io::Result<Option<SecretString>> {
        match fs::read_to_string(self.root.join(API_KEY)) {
            Ok(key) if !key.is_empty() => Ok(Some(SecretString::from(key))),
            Ok(_) => Ok(None),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Keep `key` as the server's API key, replacing any earlier one.
    pub fn keep_api_key(&self, key: &SecretString) -> io::Result<()> {
        self.write_private(API_KEY, key.expose_secret())
    }

    /// Write `contents` to `name` with mode 0600, through a temporary file that replaces
    /// it in one rename.
    pub fn write_private(&self, name: &str, contents: &str) -> io::Result<()> {
        write_private_at(&self.root, name, contents)
    }

    /// The secret kept in `name`, generated and kept with mode 0600 the first time.
    pub fn kept_secret(&self, name: &str) -> io::Result<SecretString> {
        match read_optional(&self.root.join(name))? {
            Some(secret) if !secret.is_empty() => Ok(SecretString::from(secret)),
            _ => {
                let secret = generate_secret()?;
                self.write_private(name, secret.expose_secret())?;
                Ok(secret)
            }
        }
    }
}

/// Write `contents` to `name` in `folder` with mode 0600, through a temporary file that
/// replaces it in one rename.
fn write_private_at(folder: &Path, name: &str, contents: &str) -> io::Result<()> {
    let target = folder.join(name);
    let staged = folder.join(format!(".{name}.new"));
    match fs::remove_file(&staged) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staged)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    fs::rename(&staged, &target)
}

fn read_optional(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// 32 bytes from the operating system's generator, as 64 hexadecimal digits.
pub fn generate_secret() -> io::Result<SecretString> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| io::Error::other(error.to_string()))?;
    Ok(SecretString::from(hex::encode(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_directory_and_its_secrets_are_private() {
        let directory = tempfile::tempdir().unwrap();
        let state = StateDirectory::prepare(directory.path().join("n8n")).unwrap();
        state.keep_api_key(&SecretString::from("key")).unwrap();
        let mode = fs::metadata(state.root().join(API_KEY))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let mode = fs::metadata(state.root()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        assert!(state.user_folder().is_dir());
        assert_eq!(generate_secret().unwrap().expose_secret().len(), 64);
    }

    #[test]
    fn n8n_s_settings_file_names_the_databases_key() {
        let directory = tempfile::tempdir().unwrap();
        let state = StateDirectory::prepare(directory.path().join("n8n")).unwrap();
        let read = || -> serde_json::Value {
            serde_json::from_str(&fs::read_to_string(state.n8n_settings_file()).unwrap()).unwrap()
        };
        // A missing file is created, readable by its user alone.
        state
            .align_n8n_settings(&SecretString::from("database-key"))
            .unwrap();
        assert_eq!(read(), serde_json::json!({"encryptionKey": "database-key"}));
        let mode = fs::metadata(state.n8n_settings_file())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        // A stale key is replaced and n8n's other fields stay.
        fs::write(
            state.n8n_settings_file(),
            r#"{"encryptionKey":"stale-key","tunnelSubdomain":"x","fsStorageMigrated":true}"#,
        )
        .unwrap();
        state
            .align_n8n_settings(&SecretString::from("database-key"))
            .unwrap();
        assert_eq!(
            read(),
            serde_json::json!({
                "encryptionKey": "database-key",
                "tunnelSubdomain": "x",
                "fsStorageMigrated": true
            })
        );
        // A file that is not n8n's settings object is refused, not overwritten.
        fs::write(state.n8n_settings_file(), "[]").unwrap();
        assert!(
            state
                .align_n8n_settings(&SecretString::from("database-key"))
                .is_err()
        );
    }

    #[test]
    fn the_api_key_is_kept_and_replaced() {
        let directory = tempfile::tempdir().unwrap();
        let state = StateDirectory::prepare(directory.path()).unwrap();
        assert!(state.api_key().unwrap().is_none());
        state.keep_api_key(&SecretString::from("first")).unwrap();
        state.keep_api_key(&SecretString::from("second")).unwrap();
        assert_eq!(state.api_key().unwrap().unwrap().expose_secret(), "second");
    }
}
