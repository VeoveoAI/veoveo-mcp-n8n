//! What the server keeps for n8n on the sandbox's persistent disk, under
//! `/sandbox/state/n8n`: the owner's generated password, n8n's encryption key, the API key
//! the server minted, and n8n's own user folder. Each secret is a file of mode 0600 in a
//! directory of mode 0700.
use std::{
    fs,
    io::{self, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use secrecy::{ExposeSecret, SecretString};

const OWNER_PASSWORD: &str = "owner-password";
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

    /// The owner's password, generated on first start.
    pub fn owner_password(&self) -> io::Result<SecretString> {
        self.load_or_generate(OWNER_PASSWORD)
    }

    /// n8n's encryption key, generated on first start.
    pub fn encryption_key(&self) -> io::Result<SecretString> {
        self.load_or_generate(ENCRYPTION_KEY)
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

    fn load_or_generate(&self, name: &str) -> io::Result<SecretString> {
        match fs::read_to_string(self.root.join(name)) {
            Ok(value) if !value.is_empty() => return Ok(SecretString::from(value)),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let value = generate_secret()?;
        self.write_private(name, value.expose_secret())?;
        Ok(value)
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
    fn secrets_are_generated_once_and_kept_private() {
        let directory = tempfile::tempdir().unwrap();
        let state = StateDirectory::prepare(directory.path().join("n8n")).unwrap();
        let password = state.owner_password().unwrap();
        let key = state.encryption_key().unwrap();
        assert_eq!(password.expose_secret().len(), 64);
        assert_ne!(password.expose_secret(), key.expose_secret());
        assert_eq!(
            state.owner_password().unwrap().expose_secret(),
            password.expose_secret()
        );
        let reopened = StateDirectory::prepare(directory.path().join("n8n")).unwrap();
        assert_eq!(
            reopened.encryption_key().unwrap().expose_secret(),
            key.expose_secret()
        );
        for name in [OWNER_PASSWORD, ENCRYPTION_KEY] {
            let mode = fs::metadata(state.root().join(name))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "{name}");
        }
        let mode = fs::metadata(state.root()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        assert!(state.user_folder().is_dir());
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
