//! Clavyn shared core.
//!
//! Platform-agnostic logic shared by the Tauri desktop app and future mobile
//! frontends via FFI. Contains:
//! - connection & host models
//! - SSH transport (russh) + session management
//! - key parsing / generation (russh keys / ssh-key)
//! - encrypted-at-rest vault for private keys
//! - known_hosts verification (TOFU)
//! - workspace & layout persistence
//!
//! Security rules enforced here, not in the UI layer:
//! - private key material is zeroized on drop
//! - vault ciphertext is the only thing ever persisted to disk
//! - state files are written atomically and restricted to the account that
//!   runs the app: mode 0600 on Unix, and on Windows a protected single-entry
//!   DACL together with a matching owner, because an owner outranks the DACL
//! - host key mismatches never auto-accept

pub mod connection;
pub mod error;
/// Private so that the module stays an implementation detail: the three items
/// below are the whole of its contract, and a `pub` helper added here later
/// would otherwise become part of core's API without anyone deciding so.
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
pub use fs_util::{remove_private, write_private, write_private_bytes, RemovePrivateOutcome};
