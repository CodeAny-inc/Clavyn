use thiserror::Error;

pub type Result<T> = std::result::Result<T, CoreError>;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("SSH error: {0}")]
    Ssh(String),

    #[error("SSH protocol error: {0}")]
    SshProtocol(#[from] russh::Error),

    #[error("key error: {0}")]
    Key(String),

    #[error("vault error: {0}")]
    Vault(String),

    /// The destructive vault unlink has already completed, but flushing the
    /// containing directory failed. Callers must treat the vault as destroyed
    /// and clear authentication state even though durability could not be
    /// confirmed to the storage device.
    #[error("[vault-reset-durability] vault was deleted but directory sync failed: {0}")]
    VaultResetDurability(String),

    #[error("host key verification failed for {host}: {reason}")]
    HostKeyMismatch { host: String, reason: String },

    #[error("session not found: {0}")]
    SessionNotFound(String),

    #[error("{path} is corrupt and was not loaded: {reason}")]
    CorruptState { path: String, reason: String },

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),
}
