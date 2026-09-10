use crate::host::{Host, HostGroup};
use crate::identity::Identity;
use crate::workspace::Workspace;
use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Persistent store for non-secret data: hosts, host groups, identities, workspaces.
/// Secrets (private keys) live in the vault; passwords live in the OS keychain.
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

pub struct Store {
    path: PathBuf,
    data: StoreData,
}

impl Store {
    pub fn load(path: PathBuf) -> Result<Self> {
        let data = if path.exists() {
            let raw = std::fs::read_to_string(&path)?;
            // Fail closed. Falling back to an empty store would drop every host,
            // identity and workspace, and the next save() would overwrite the
            // file that still holds them.
            serde_json::from_str(&raw).map_err(|e| CoreError::CorruptState {
                path: path.display().to_string(),
                reason: e.to_string(),
            })?
        } else {
            StoreData::default()
        };
        Ok(Self { path, data })
    }

    pub fn data(&self) -> &StoreData {
        &self.data
    }

    pub fn save(&self) -> Result<()> {
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

    // --- hosts ---
    pub fn add_host(&mut self, host: Host) -> Result<()> {
        self.data.hosts.push(host);
        self.save()
    }

    pub fn update_host(&mut self, host: Host) -> Result<()> {
        if let Some(h) = self.data.hosts.iter_mut().find(|h| h.id == host.id) {
            *h = host;
        }
        self.save()
    }

    pub fn remove_host(&mut self, id: uuid::Uuid) -> Result<()> {
        self.data.hosts.retain(|h| h.id != id);
        self.save()
    }

    pub fn hosts(&self) -> &[Host] {
        &self.data.hosts
    }

    // --- host groups ---
    pub fn add_group(&mut self, group: HostGroup) -> Result<()> {
        self.data.host_groups.push(group);
        self.save()
    }

    pub fn remove_group(&mut self, id: uuid::Uuid) -> Result<()> {
        self.data.host_groups.retain(|g| g.id != id);
        self.data.hosts.iter_mut().for_each(|h| {
            if h.group_id == Some(id) {
                h.group_id = None;
            }
        });
        self.data.identities.iter_mut().for_each(|i| {
            if i.group_id == Some(id) {
                i.group_id = None;
            }
        });
        self.save()
    }

    pub fn groups(&self) -> &[HostGroup] {
        &self.data.host_groups
    }

    // --- identities ---
    pub fn add_identity(&mut self, identity: Identity) -> Result<()> {
        self.data.identities.push(identity);
        self.save()
    }

    pub fn update_identity(&mut self, identity: Identity) -> Result<()> {
        if let Some(i) = self
            .data
            .identities
            .iter_mut()
            .find(|i| i.id == identity.id)
        {
            *i = identity;
        }
        self.save()
    }

    pub fn remove_identity(&mut self, id: uuid::Uuid) -> Result<()> {
        self.data.identities.retain(|i| i.id != id);
        // Unset identity_id on any hosts that referenced it
        self.data.hosts.iter_mut().for_each(|h| {
            if h.identity_id == Some(id) {
                h.identity_id = None;
            }
        });
        self.save()
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
        self.data.workspaces.push(ws);
        self.save()
    }

    pub fn update_workspace(&mut self, mut ws: Workspace) -> Result<()> {
        ws.sanitize()?;
        if let Some(w) = self.data.workspaces.iter_mut().find(|w| w.id == ws.id) {
            *w = ws;
        }
        self.save()
    }

    pub fn remove_workspace(&mut self, id: uuid::Uuid) -> Result<()> {
        self.data.workspaces.retain(|w| w.id != id);
        if self.data.active_workspace_id == Some(id) {
            self.data.active_workspace_id = None;
        }
        self.save()
    }

    pub fn workspaces(&self) -> &[Workspace] {
        &self.data.workspaces
    }

    pub fn set_active_workspace(&mut self, id: uuid::Uuid) -> Result<()> {
        self.data.active_workspace_id = Some(id);
        self.save()
    }
}

#[cfg(test)]
mod tests {
    use super::Store;
    use crate::host::HostGroup;
    use crate::workspace::{PaneLayout, SplitDirection, TabLayout, Workspace};

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
}
