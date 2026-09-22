use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A saved SSH host. Passwords are never stored: each connection prompts for
/// one and drops it once the connection is made. Key references point to vault
/// key ids.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Host {
    pub id: Uuid,
    pub label: String,
    pub hostname: String,
    pub port: u16,
    pub username: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub group_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub key_id: Option<Uuid>,
    pub auth: AuthMethod,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub startup_command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub proxy_command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub jump_host_id: Option<Uuid>,
    /// Optional reference to an Identity. When set, the username and auth
    /// method are resolved from the identity at connect time.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub identity_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AuthMethod {
    /// Password authentication. The password itself is prompted for at each
    /// connection and never stored; `credential_key` is a label kept for the
    /// stored format and nothing looks anything up by it.
    Password { credential_key: String },
    PublicKey,
    Agent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostGroup {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub color: Option<String>,
}

impl Host {
    pub fn new(
        label: impl Into<String>,
        hostname: impl Into<String>,
        port: u16,
        username: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            label: label.into(),
            hostname: hostname.into(),
            port,
            username: username.into(),
            group_id: None,
            key_id: None,
            auth: AuthMethod::Agent,
            tags: Vec::new(),
            startup_command: None,
            proxy_command: None,
            jump_host_id: None,
            identity_id: None,
        }
    }
}

impl HostGroup {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            color: None,
        }
    }
}
