//! n8n's encryption key, kept in the server's own table beside n8n's data, which outlives
//! the sandbox's disk, with the identity of that n8n instance. Every credential n8n holds
//! is a provider's placeholder, so the key protects no real secret; it only lets n8n read
//! its own data again. The instance identity is new with each kept key, so the data of an
//! n8n started again over a reset schema is another instance's.
use secrecy::{ExposeSecret, SecretString};
use veoveo_store::{PlatformStore, TenantId};

use super::StoreError;

/// Where the key the server uses came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyOrigin {
    /// The database kept it.
    Kept,
    /// The sandbox's disk held it from an earlier version, and the database keeps it now.
    Adopted,
    /// n8n has no data yet, so the server generated it.
    Generated,
}

/// Why the server has no key for n8n's data.
#[derive(Debug, thiserror::Error)]
pub enum InstanceKeyError {
    #[error(
        "n8n's data in schema n8n cannot be read without its encryption key, which neither the database nor the sandbox's disk holds; reset n8n's tables or restore the key"
    )]
    Lost,
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("generating n8n's encryption key: {0}")]
    Generate(#[from] std::io::Error),
}

struct KeyRow {
    instance_id: uuid::Uuid,
    encryption_key: String,
}

/// n8n's key and the identity of the n8n instance whose data it reads.
pub struct InstanceKey {
    pub instance: uuid::Uuid,
    pub encryption_key: SecretString,
}

/// n8n's encryption key in the server's schema.
#[derive(Clone)]
pub struct InstanceKeyStore {
    store: PlatformStore,
}

impl InstanceKeyStore {
    pub fn new(store: PlatformStore) -> Self {
        Self { store }
    }

    /// The key of `tenant`'s n8n, when the database keeps one.
    pub async fn key(&self, tenant: TenantId) -> Result<Option<InstanceKey>, StoreError> {
        Ok(sqlx::query_file_as!(
            KeyRow,
            "src/store/instance/instance_key.sql",
            tenant as TenantId
        )
        .fetch_optional(self.store.pool())
        .await?
        .map(|row| InstanceKey {
            instance: row.instance_id,
            encryption_key: SecretString::from(row.encryption_key),
        }))
    }

    /// Keep `key` for `tenant`'s n8n with a new instance identity unless a key is kept
    /// already, and return the kept one.
    async fn keep(&self, tenant: TenantId, key: &SecretString) -> Result<InstanceKey, StoreError> {
        sqlx::query_file!(
            "src/store/instance/keep_instance_key.sql",
            tenant as TenantId,
            uuid::Uuid::now_v7(),
            key.expose_secret(),
        )
        .execute(self.store.pool())
        .await?;
        self.key(tenant)
            .await?
            .ok_or(StoreError::Corrupt("instance key"))
    }

    /// Whether n8n's migrations ran in the server's schema.
    pub async fn n8n_has_data(&self) -> Result<bool, StoreError> {
        Ok(
            sqlx::query_file_scalar!("src/store/instance/n8n_has_data.sql")
                .fetch_one(self.store.pool())
                .await?,
        )
    }

    /// The key n8n starts with: the one the database keeps; else the one an earlier
    /// version kept on the sandbox's disk, `on_disk`, which the database keeps from then
    /// on; else a new one, but only while n8n has no data a new key could not read.
    pub async fn encryption_key(
        &self,
        tenant: TenantId,
        on_disk: Option<SecretString>,
    ) -> Result<(InstanceKey, KeyOrigin), InstanceKeyError> {
        if let Some(key) = self.key(tenant).await? {
            return Ok((key, KeyOrigin::Kept));
        }
        if let Some(key) = on_disk {
            return Ok((self.keep(tenant, &key).await?, KeyOrigin::Adopted));
        }
        if self.n8n_has_data().await? {
            return Err(InstanceKeyError::Lost);
        }
        let key = crate::n8n::generate_secret()?;
        Ok((self.keep(tenant, &key).await?, KeyOrigin::Generated))
    }
}
