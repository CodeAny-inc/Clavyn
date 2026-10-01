//! Recognizing a vault file that was rolled back to an older copy.
//!
//! Every save of the vault advances an epoch that is authenticated with the
//! rest of the header, so it cannot be forged. That alone does not reveal a
//! rollback: an older copy carries its own, older, genuine epoch. Telling the
//! two apart needs a record of the highest epoch this device has seen, kept
//! somewhere a rewrite of the app data directory does not reach. That record
//! lives in the OS credential store (Credential Manager on Windows, Keychain
//! on macOS, Secret Service on Linux), keyed by the vault's binding id.
//!
//! A restored backup is also a rollback, and a legitimate one, so an older
//! vault is not refused outright: unlock stops and says what it found, and the
//! user can open the older copy deliberately, which lowers the record to it.
//!
//! Where no credential store is available (a Linux session without Secret
//! Service, say) there is nothing to compare with. Unlock then proceeds as it
//! always has, without rollback protection, and says so in the log. Losing the
//! store never locks anyone out of their vault.

type ApiResult<T> = std::result::Result<T, String>;

/// Prefix of the error an unlock returns for a rolled-back vault, so the UI
/// can offer to open the older copy instead of only showing the message.
pub const ROLLBACK_MARKER: &str = "[vault-rollback]";

// Tests use `MemoryEpochStore` and never reach the credential store.
#[cfg_attr(test, allow(dead_code))]
const SERVICE: &str = "com.clavyn.app.vault-epoch";

/// Where the highest seen epoch per vault generation is kept.
pub trait EpochStore: Send + Sync {
    fn read(&self, binding_id: &str) -> ApiResult<Option<u64>>;
    fn write(&self, binding_id: &str, epoch: u64) -> ApiResult<()>;
}

/// The OS credential store.
#[cfg_attr(test, allow(dead_code))]
pub struct CredentialEpochStore;

impl EpochStore for CredentialEpochStore {
    fn read(&self, binding_id: &str) -> ApiResult<Option<u64>> {
        let entry = keyring::Entry::new(SERVICE, binding_id).map_err(|e| e.to_string())?;
        match entry.get_password() {
            Ok(value) => value
                .trim()
                .parse()
                .map(Some)
                .map_err(|e| format!("stored vault epoch is not a number: {e}")),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn write(&self, binding_id: &str, epoch: u64) -> ApiResult<()> {
        keyring::Entry::new(SERVICE, binding_id)
            .and_then(|entry| entry.set_password(&epoch.to_string()))
            .map_err(|e| e.to_string())
    }
}

/// An in-memory store, so tests never touch the real credential store.
#[cfg(test)]
#[derive(Default)]
pub struct MemoryEpochStore {
    epochs: std::sync::Mutex<std::collections::HashMap<String, u64>>,
    pub unavailable: std::sync::atomic::AtomicBool,
}

#[cfg(test)]
impl EpochStore for MemoryEpochStore {
    fn read(&self, binding_id: &str) -> ApiResult<Option<u64>> {
        if self.unavailable.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("no credential store".into());
        }
        Ok(self.epochs.lock().unwrap().get(binding_id).copied())
    }

    fn write(&self, binding_id: &str, epoch: u64) -> ApiResult<()> {
        if self.unavailable.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("no credential store".into());
        }
        self.epochs.lock().unwrap().insert(binding_id.to_string(), epoch);
        Ok(())
    }
}

/// The store the app uses. Tests get an in-memory one.
pub fn default_store() -> std::sync::Arc<dyn EpochStore> {
    #[cfg(test)]
    return std::sync::Arc::new(MemoryEpochStore::default());
    #[cfg(not(test))]
    std::sync::Arc::new(CredentialEpochStore)
}

/// Refuse to unlock a vault older than one this device has already opened,
/// unless the user chose to open the older copy.
///
/// Runs after the passphrase is verified, so the epoch it compares is the
/// authenticated one, and before the vault is unlocked for the session.
pub fn check_on_unlock(
    store: &dyn EpochStore,
    binding_id: &str,
    file_epoch: u64,
    accept_older: bool,
) -> ApiResult<()> {
    let seen = match store.read(binding_id) {
        Ok(seen) => seen,
        Err(error) => {
            tracing::warn!("vault rollback protection unavailable: {error}");
            return Ok(());
        }
    };
    match seen {
        Some(seen) if file_epoch < seen && !accept_older => Err(format!(
            "{ROLLBACK_MARKER} This vault file is older than one this device has already opened \
             (it has been saved {file_epoch} times; the newest copy seen had {seen}). It may be \
             a restored backup, or it may have been replaced to bring back keys that were \
             removed. Open it only if you restored it yourself."
        )),
        Some(seen) if file_epoch < seen => {
            // The user chose the older copy, so it becomes the reference.
            write_logged(store, binding_id, file_epoch);
            Ok(())
        }
        _ => {
            record(store, binding_id, file_epoch);
            Ok(())
        }
    }
}

/// Raise the recorded epoch after the vault was saved or opened. A failure is
/// logged, not returned: the vault itself is already written, and a record that
/// lags only narrows what a later rollback check can see.
pub fn record(store: &dyn EpochStore, binding_id: &str, epoch: u64) {
    match store.read(binding_id) {
        Ok(Some(seen)) if seen >= epoch => {}
        Ok(_) => write_logged(store, binding_id, epoch),
        Err(error) => tracing::warn!("vault rollback protection unavailable: {error}"),
    }
}

fn write_logged(store: &dyn EpochStore, binding_id: &str, epoch: u64) {
    if let Err(error) = store.write(binding_id, epoch) {
        tracing::warn!("could not record the vault epoch: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_first_unlock_records_the_epoch() {
        let store = MemoryEpochStore::default();
        check_on_unlock(&store, "vault", 4, false).expect("first unlock");
        assert_eq!(store.read("vault").unwrap(), Some(4));
    }

    #[test]
    fn a_current_or_newer_vault_unlocks_and_raises_the_record() {
        let store = MemoryEpochStore::default();
        store.write("vault", 4).unwrap();
        check_on_unlock(&store, "vault", 4, false).expect("same epoch");
        check_on_unlock(&store, "vault", 7, false).expect("newer epoch");
        assert_eq!(store.read("vault").unwrap(), Some(7));
    }

    #[test]
    fn an_older_vault_is_refused_until_the_user_accepts_it() {
        let store = MemoryEpochStore::default();
        store.write("vault", 7).unwrap();

        let error = check_on_unlock(&store, "vault", 3, false).expect_err("rollback");
        assert!(error.starts_with(ROLLBACK_MARKER), "{error}");
        assert_eq!(store.read("vault").unwrap(), Some(7));

        check_on_unlock(&store, "vault", 3, true).expect("accepted");
        assert_eq!(store.read("vault").unwrap(), Some(3));
        check_on_unlock(&store, "vault", 3, false).expect("accepted copy is current now");
    }

    #[test]
    fn generations_are_tracked_separately() {
        let store = MemoryEpochStore::default();
        store.write("old generation", 9).unwrap();
        check_on_unlock(&store, "new generation", 1, false).expect("new vault");
    }

    #[test]
    fn saves_only_ever_raise_the_record() {
        let store = MemoryEpochStore::default();
        record(&store, "vault", 5);
        record(&store, "vault", 2);
        assert_eq!(store.read("vault").unwrap(), Some(5));
    }

    #[test]
    fn without_a_store_unlock_proceeds_unprotected() {
        let store = MemoryEpochStore::default();
        store.unavailable.store(true, std::sync::atomic::Ordering::SeqCst);
        check_on_unlock(&store, "vault", 1, false).expect("no store, no refusal");
        record(&store, "vault", 2);
    }
}
