//! What the server keeps for n8n on the sandbox's persistent disk, under
//! `/sandbox/state/n8n`: the API key the server minted, and n8n's own user folder. Nothing
//! here outlives a new sandbox, so nothing here is needed to read n8n's data: n8n's
//! encryption key lives in the database with that data, and the owner's password is new
//! at every start. Each secret is a file of mode 0600 in a directory of mode 0700.
use std::{
    fs,
    io::{self, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use secrecy::{ExposeSecret, SecretString};

const ENCRYPTION_KEY: &str = "encryption-key";
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

    /// The encryption key this disk holds, which the server adopts when its database holds
    /// none: the one an earlier version of the server kept here, or the one n8n wrote into
    /// its own settings file, `.n8n/config` in its user folder, which n8n requires to match
    /// the key it starts with.
    pub fn kept_encryption_key(&self) -> io::Result<Option<SecretString>> {
        if let Some(key) = read_optional(&self.root.join(ENCRYPTION_KEY))? {
            let key = key.trim();
            if !key.is_empty() {
                return Ok(Some(SecretString::from(key.to_owned())));
            }
        }
        let Some(settings) = read_optional(&self.n8n_settings_file())? else {
            return Ok(None);
        };
        let settings: serde_json::Value = serde_json::from_str(&settings)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        Ok(settings
            .get("encryptionKey")
            .and_then(serde_json::Value::as_str)
            .filter(|key| !key.is_empty())
            .map(|key| SecretString::from(key.to_owned())))
    }

    /// n8n's own settings file in its user folder.
    pub fn n8n_settings_file(&self) -> PathBuf {
        self.user_folder().join(".n8n").join("config")
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
        let target = self.root.join(name);
        let staged = self.root.join(format!(".{name}.new"));
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
        assert!(state.kept_encryption_key().unwrap().is_none());
        state.keep_api_key(&SecretString::from("key")).unwrap();
        let mode = fs::metadata(state.root().join(API_KEY))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let mode = fs::metadata(state.root()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        assert!(state.user_folder().is_dir());
        // n8n's settings file holds the key n8n started with, which the server adopts.
        fs::create_dir_all(state.user_folder().join(".n8n")).unwrap();
        fs::write(
            state.n8n_settings_file(),
            r#"{"encryptionKey":"n8n-settings-key"}"#,
        )
        .unwrap();
        assert_eq!(
            state
                .kept_encryption_key()
                .unwrap()
                .unwrap()
                .expose_secret(),
            "n8n-settings-key"
        );
        // A key an earlier version kept on the disk comes first.
        state.write_private(ENCRYPTION_KEY, "kept-key\n").unwrap();
        assert_eq!(
            state
                .kept_encryption_key()
                .unwrap()
                .unwrap()
                .expose_secret(),
            "kept-key"
        );
        assert_eq!(generate_secret().unwrap().expose_secret().len(), 64);
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
