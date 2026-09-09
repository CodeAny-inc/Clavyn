use crate::host::{Host, HostGroup};
use crate::identity::Identity;
use crate::workspace::Workspace;
use crate::Result;
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
            serde_json::from_str(&raw).unwrap_or_default()
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
    pub fn add_workspace(&mut self, ws: Workspace) -> Result<()> {
        self.data.workspaces.push(ws);
        self.save()
    }

    pub fn update_workspace(&mut self, ws: Workspace) -> Result<()> {
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
