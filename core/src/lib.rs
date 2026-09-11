//! Clavyn shared core.
//!
//! Platform-agnostic logic shared by the Tauri desktop app and future mobile
//! frontends via FFI. Contains:
//! - connection & host models
//! - SSH transport (russh) + session management
//! - key parsing / generation (russh-keys)
//! - encrypted-at-rest vault for private keys
//! - known_hosts verification (TOFU)
//! - workspace & layout persistence
//!
//! Security rules enforced here, not in the UI layer:
//! - private key material is zeroized on drop
//! - vault ciphertext is the only thing ever persisted to disk
//! - state files are written atomically and owner-only
//! - host key mismatches never auto-accept

pub mod connection;
pub mod error;
mod fs_util;
pub mod host;
pub mod identity;
pub mod keys;
pub mod known_hosts;
pub mod output;
pub mod session;
pub mod sftp;
pub mod store;
pub mod vault;
pub mod workspace;

pub use error::{CoreError, Result};
