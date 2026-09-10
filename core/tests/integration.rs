use clavyn_core::host::{AuthMethod, Host, HostGroup};
use clavyn_core::identity::Identity;
use clavyn_core::keys::{generate_ed25519, parse_openssh_private};
use clavyn_core::store::Store;
use clavyn_core::vault::Vault;
use clavyn_core::workspace::Workspace;
use tempfile::TempDir;

fn temp_dir() -> TempDir {
    tempfile::tempdir().expect("create temp dir")
}

// ============================================================
// Host tests
// ============================================================

#[test]
fn test_host_new() {
    let host = Host::new("My Server", "example.com", 22, "root");
    assert_eq!(host.label, "My Server");
    assert_eq!(host.hostname, "example.com");
    assert_eq!(host.port, 22);
    assert_eq!(host.username, "root");
    assert_eq!(host.auth, AuthMethod::Agent);
    assert!(host.group_id.is_none());
    assert!(host.key_id.is_none());
    assert!(host.tags.is_empty());
}

#[test]
fn test_host_group_new() {
    let group = HostGroup::new("Production");
    assert_eq!(group.name, "Production");
    assert!(group.color.is_none());
}

#[test]
fn test_host_serialization() {
    let host = Host::new("Test", "host.com", 2222, "user");
    let json = serde_json::to_string(&host).unwrap();
    let deserialized: Host = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.label, "Test");
    assert_eq!(deserialized.port, 2222);
}

#[test]
fn test_auth_method_serialization() {
    let agent = AuthMethod::Agent;
    let json = serde_json::to_string(&agent).unwrap();
    assert_eq!(json, "\"agent\"");

    let pubkey = AuthMethod::PublicKey;
    let json = serde_json::to_string(&pubkey).unwrap();
    assert_eq!(json, "\"publickey\"");

    let password = AuthMethod::Password {
        credential_key: "key123".to_string(),
    };
    let json = serde_json::to_string(&password).unwrap();
    assert!(json.contains("password"));
    assert!(json.contains("key123"));
}

// ============================================================
// Key tests
// ============================================================

#[test]
fn test_generate_ed25519() {
    let (private_pem, public_b64) = generate_ed25519().expect("generate key");
    assert!(private_pem.contains("BEGIN OPENSSH PRIVATE KEY"));
    assert!(public_b64.starts_with("AAA"));
    assert!(!public_b64.contains("ssh-ed25519"));
}

#[test]
fn test_parse_generated_key() {
    let (private_pem, _) = generate_ed25519().expect("generate key");
    let (meta, _pair) = parse_openssh_private(&private_pem, None).expect("parse key");
    assert_eq!(meta.key_type, clavyn_core::keys::KeyType::Ed25519);
    assert!(!meta.fingerprint.is_empty());
    assert!(!meta.public_key_base64.is_empty());
}

#[test]
fn test_parse_with_wrong_passphrase_fails() {
    let (private_pem, _) = generate_ed25519().expect("generate key");
    let result = parse_openssh_private(&private_pem, Some("wrong-passphrase"));
    assert!(result.is_err());
}

#[test]
fn test_generate_two_keys_are_different() {
    let (priv1, pub1) = generate_ed25519().expect("generate key 1");
    let (priv2, pub2) = generate_ed25519().expect("generate key 2");
    assert_ne!(priv1, priv2);
    assert_ne!(pub1, pub2);
}

// ============================================================
// Store tests
// ============================================================

#[test]
fn test_store_load_empty() {
    let dir = temp_dir();
    let store = Store::load(dir.path().join("store.json")).expect("load store");
    assert!(store.hosts().is_empty());
    assert!(store.groups().is_empty());
    assert!(store.workspaces().is_empty());
}

#[test]
fn test_store_add_and_list_host() {
    let dir = temp_dir();
    let mut store = Store::load(dir.path().join("store.json")).expect("load store");

    let host = Host::new("Server 1", "host1.com", 22, "root");
    let host_id = host.id;
    store.add_host(host).expect("add host");

    assert_eq!(store.hosts().len(), 1);
    assert_eq!(store.hosts()[0].id, host_id);
    assert_eq!(store.hosts()[0].label, "Server 1");
}

#[test]
fn test_store_update_host() {
    let dir = temp_dir();
    let mut store = Store::load(dir.path().join("store.json")).expect("load store");

    let mut host = Host::new("Server 1", "host1.com", 22, "root");
    store.add_host(host.clone()).expect("add host");

    host.label = "Updated Server".to_string();
    store.update_host(host).expect("update host");

    assert_eq!(store.hosts().len(), 1);
    assert_eq!(store.hosts()[0].label, "Updated Server");
}

#[test]
fn test_store_remove_host() {
    let dir = temp_dir();
    let mut store = Store::load(dir.path().join("store.json")).expect("load store");

    let host = Host::new("Server 1", "host1.com", 22, "root");
    let host_id = host.id;
    store.add_host(host).expect("add host");
    assert_eq!(store.hosts().len(), 1);

    store.remove_host(host_id).expect("remove host");
    assert!(store.hosts().is_empty());
}

#[test]
fn test_store_add_and_remove_group() {
    let dir = temp_dir();
    let mut store = Store::load(dir.path().join("store.json")).expect("load store");

    let group = HostGroup::new("Production");
    let group_id = group.id;
    store.add_group(group).expect("add group");
    assert_eq!(store.groups().len(), 1);

    // Add a host with this group
    let mut host = Host::new("Server", "host.com", 22, "user");
    host.group_id = Some(group_id);
    store.add_host(host).expect("add host");

    // Remove group — host's group_id should be cleared
    store.remove_group(group_id).expect("remove group");
    assert!(store.groups().is_empty());
    assert!(store.hosts()[0].group_id.is_none());
}

#[test]
fn test_store_persistence() {
    let dir = temp_dir();
    let path = dir.path().join("store.json");

    // Write data
    let mut store = Store::load(path.clone()).expect("load store");
    store
        .add_host(Host::new("Persisted", "host.com", 22, "root"))
        .expect("add host");

    // Reload from disk
    let reloaded = Store::load(path).expect("reload store");
    assert_eq!(reloaded.hosts().len(), 1);
    assert_eq!(reloaded.hosts()[0].label, "Persisted");
}

#[test]
fn test_store_workspaces() {
    let dir = temp_dir();
    let mut store = Store::load(dir.path().join("store.json")).expect("load store");

    let ws = Workspace::new("My Workspace");
    let ws_id = ws.id;
    store.add_workspace(ws).expect("add workspace");
    assert_eq!(store.workspaces().len(), 1);

    store.set_active_workspace(ws_id).expect("set active");
    assert_eq!(store.data().active_workspace_id, Some(ws_id));

    store.remove_workspace(ws_id).expect("remove workspace");
    assert!(store.workspaces().is_empty());
    assert_eq!(store.data().active_workspace_id, None);
}

// ============================================================
// Vault tests
// ============================================================

#[test]
fn test_vault_open_empty() {
    let dir = temp_dir();
    let vault = Vault::open(dir.path().join("vault.json")).expect("open vault");
    assert!(!vault.is_initialized());
    assert!(vault.keys_meta().is_empty());
}

#[tokio::test]
async fn test_vault_initialize() {
    let dir = temp_dir();
    let mut vault = Vault::open(dir.path().join("vault.json")).expect("open vault");
    assert!(!vault.is_initialized());

    vault.initialize("my-passphrase").await.expect("init vault");
    assert!(vault.is_initialized());
    assert!(vault.keys_meta().is_empty());
}

#[tokio::test]
async fn test_vault_add_and_get_key() {
    let dir = temp_dir();
    let mut vault = Vault::open(dir.path().join("vault.json")).expect("open vault");
    let master = vault.initialize("passphrase").await.expect("init vault");

    // Generate a key
    let (private_pem, _) = generate_ed25519().expect("generate key");
    let (meta, _) = parse_openssh_private(&private_pem, None).expect("parse key");
    let key_id = meta.id;

    let mut meta = meta;
    meta.label = "Test Key".to_string();

    vault
        .add_key(&master, meta, &private_pem)
        .expect("add key");

    assert_eq!(vault.keys_meta().len(), 1);
    assert_eq!(vault.keys_meta()[0].label, "Test Key");

    // Retrieve the key
    let retrieved = vault
        .get_key("passphrase", &key_id.to_string())
        .await
        .expect("get key");
    let retrieved_str = String::from_utf8(retrieved).unwrap();
    assert!(retrieved_str.contains("BEGIN OPENSSH PRIVATE KEY"));
}

#[tokio::test]
async fn test_vault_wrong_passphrase_fails() {
    let dir = temp_dir();
    let mut vault = Vault::open(dir.path().join("vault.json")).expect("open vault");
    let master = vault.initialize("correct-pass").await.expect("init vault");

    let (private_pem, _) = generate_ed25519().expect("generate key");
    let (meta, _) = parse_openssh_private(&private_pem, None).expect("parse key");
    let key_id = meta.id.to_string();

    vault
        .add_key(&master, meta, &private_pem)
        .expect("add key");

    // Try to get with wrong passphrase
    let result = vault.get_key("wrong-pass", &key_id).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_vault_remove_key() {
    let dir = temp_dir();
    let mut vault = Vault::open(dir.path().join("vault.json")).expect("open vault");
    let master = vault.initialize("pass").await.expect("init vault");

    let (private_pem, _) = generate_ed25519().expect("generate key");
    let (meta, _) = parse_openssh_private(&private_pem, None).expect("parse key");
    let key_id = meta.id.to_string();

    vault.add_key(&master, meta, &private_pem).expect("add key");
    assert_eq!(vault.keys_meta().len(), 1);

    vault.remove_key(&master, &key_id).expect("remove key");
    assert!(vault.keys_meta().is_empty());

    // Key should no longer be retrievable
    let result = vault.get_key("pass", &key_id).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_vault_persistence() {
    let dir = temp_dir();
    let path = dir.path().join("vault.json");

    // Initialize and add a key
    let mut vault = Vault::open(path.clone()).expect("open vault");
    let master = vault.initialize("passphrase").await.expect("init vault");

    let (private_pem, _) = generate_ed25519().expect("generate key");
    let (meta, _) = parse_openssh_private(&private_pem, None).expect("parse key");
    let key_id = meta.id;
    let mut meta = meta;
    meta.label = "Persisted Key".to_string();

    vault
        .add_key(&master, meta, &private_pem)
        .expect("add key");

    // Reload from disk
    let reloaded = Vault::open(path).expect("reload vault");
    assert!(reloaded.is_initialized());
    assert_eq!(reloaded.keys_meta().len(), 1);
    assert_eq!(reloaded.keys_meta()[0].label, "Persisted Key");

    // Key should be retrievable
    let retrieved = reloaded
        .get_key("passphrase", &key_id.to_string())
        .await
        .expect("get key");
    assert!(String::from_utf8(retrieved).unwrap().contains("OPENSSH PRIVATE KEY"));
}

// ============================================================
// Workspace tests
// ============================================================

#[test]
fn test_workspace_new() {
    let ws = Workspace::new("My Workspace");
    assert_eq!(ws.name, "My Workspace");
    assert!(ws.tabs.is_empty());
    assert!(ws.icon.is_none());
}

// ============================================================
// Identity tests
// ============================================================

#[test]
fn test_identity_new() {
    let id = Identity::new("Work Admin", "admin");
    assert_eq!(id.label, "Work Admin");
    assert_eq!(id.username, "admin");
    assert_eq!(id.auth, AuthMethod::Agent);
    assert!(id.key_id.is_none());
    assert!(id.tags.is_empty());
}

#[test]
fn test_identity_serialization() {
    let id = Identity::new("Test", "root");
    let json = serde_json::to_string(&id).unwrap();
    let deserialized: Identity = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.label, "Test");
    assert_eq!(deserialized.username, "root");
}

#[test]
fn test_store_add_and_list_identity() {
    let dir = temp_dir();
    let mut store = Store::load(dir.path().join("store.json")).expect("load store");

    let id = Identity::new("Work Admin", "admin");
    let id_id = id.id;
    store.add_identity(id).expect("add identity");

    assert_eq!(store.identities().len(), 1);
    assert_eq!(store.identities()[0].id, id_id);
    assert_eq!(store.identities()[0].label, "Work Admin");
}

#[test]
fn test_store_update_identity() {
    let dir = temp_dir();
    let mut store = Store::load(dir.path().join("store.json")).expect("load store");

    let mut id = Identity::new("Original", "user");
    store.add_identity(id.clone()).expect("add identity");

    id.label = "Updated".to_string();
    store.update_identity(id).expect("update identity");

    assert_eq!(store.identities().len(), 1);
    assert_eq!(store.identities()[0].label, "Updated");
}

#[test]
fn test_store_remove_identity_clears_host_refs() {
    let dir = temp_dir();
    let mut store = Store::load(dir.path().join("store.json")).expect("load store");

    let id = Identity::new("Work Admin", "admin");
    let id_id = id.id;
    store.add_identity(id).expect("add identity");

    // Add a host that references this identity
    let mut host = Host::new("Server", "host.com", 22, "user");
    host.identity_id = Some(id_id);
    store.add_host(host).expect("add host");

    assert_eq!(store.identities().len(), 1);
    assert!(store.hosts()[0].identity_id.is_some());

    // Remove identity — host's identity_id should be cleared
    store.remove_identity(id_id).expect("remove identity");
    assert!(store.identities().is_empty());
    assert!(store.hosts()[0].identity_id.is_none());
}

#[test]
fn test_identity_persistence() {
    let dir = temp_dir();
    let path = dir.path().join("store.json");

    let mut store = Store::load(path.clone()).expect("load store");
    store
        .add_identity(Identity::new("Persisted", "root"))
        .expect("add identity");

    let reloaded = Store::load(path).expect("reload store");
    assert_eq!(reloaded.identities().len(), 1);
    assert_eq!(reloaded.identities()[0].label, "Persisted");
}
