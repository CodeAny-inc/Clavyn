#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod biometric;
mod biometric_commands;
mod commands;
mod host_key_prompt;
mod sftp_transfer;
mod state;
mod vault_commands;
mod vault_initialize;
mod vault_keychain_cleanup;

use state::AppState;
use tauri::Manager;
#[cfg(not(debug_assertions))]
use tauri::Emitter;

/// Whether this build claims the single-instance guard.
///
/// The guard is keyed on the bundle identifier on every platform the plugin
/// supports, and its Linux backend resolves a D-Bus session address with
/// `unwrap` inside a setup hook that runs before any window exists. Two cases
/// therefore stay out of it:
///
/// * Debug builds, which carry the same `com.clavyn.app` identifier as an
///   installed release build. A guarded debug build launched next to an
///   installed one hands focus to the installed window and exits instead of
///   starting, which makes `tauri dev` unusable on a machine that also has
///   Clavyn installed. Running both at once still shares one app-data
///   directory, so run one at a time.
/// * A Linux session whose `DBUS_SESSION_BUS_ADDRESS` does not parse. The
///   plugin panics on that address rather than returning an error, and
///   `panic = "abort"` turns the panic into a silent exit with no window and
///   no message. Resolving the address here first downgrades that to a missing
///   guard. A session with no bus running at all needs no guard from here: the
///   plugin's own connect already fails soft and the app starts unguarded.
fn single_instance_guard_applies() -> bool {
    guard_applies(cfg!(debug_assertions), session_bus_is_addressable())
}

/// The guard decision with both of its inputs supplied by the caller.
///
/// Neither input can be varied from inside a test: `debug_assertions` is fixed
/// when the test binary is compiled, and the address check answers from the
/// ambient session. Taking them as parameters keeps every combination reachable
/// from a single build profile.
///
/// Both arguments are evaluated before the call, so a debug build resolves the
/// address it then ignores. That costs an environment read and a parse, with no
/// I/O and nothing to fail on.
fn guard_applies(is_debug_build: bool, bus_addressable: bool) -> bool {
    !is_debug_build && bus_addressable
}

/// Whether the address the single-instance plugin resolves on startup parses.
///
/// `zbus::Address::session()` is the exact call the plugin unwraps: it reads
/// `DBUS_SESSION_BUS_ADDRESS` and falls back to `$XDG_RUNTIME_DIR/bus`, which
/// always parses. It performs no I/O, so a session with no bus running still
/// answers yes here and is left to the plugin, whose connect attempt fails
/// soft.
#[cfg(target_os = "linux")]
fn session_bus_is_addressable() -> bool {
    zbus::Address::session().is_ok()
}

/// Windows keys the guard on a named mutex and macOS on a socket path, neither
/// of which can fail to be named.
#[cfg(not(target_os = "linux"))]
fn session_bus_is_addressable() -> bool {
    true
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "clavyn_desktop=info,clavyn_core=info".into()),
        )
        .init();

    let mut builder = tauri::Builder::default();

    if single_instance_guard_applies() {
        // Stays the first plugin: it decides whether this process keeps
        // running at all. Every persisted file (store, vault, known_hosts) is
        // read once at startup and rewritten in full on each change, so two
        // processes sharing one app-data directory silently overwrite each
        // other — a second instance can drop a host key the first just pinned,
        // downgrading that host back to trust-on-first-use.
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }));
    }

    builder
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let app_data = app
                .path()
                .app_data_dir()
                .expect("no app data dir");
            let state = AppState::init(app.handle(), app_data)?;
            app.manage(state);

            #[cfg(not(debug_assertions))]
            {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    check_for_updates_silent(handle).await;
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_hosts,
            commands::add_host,
            commands::update_host,
            commands::delete_host,
            commands::list_groups,
            commands::add_group,
            commands::delete_group,
            commands::list_identities,
            commands::add_identity,
            commands::update_identity,
            commands::delete_identity,
            commands::vault_is_initialized,
            vault_initialize::secure_initialize_vault,
            vault_commands::secure_unlock_vault,
            vault_commands::secure_lock_vault,
            vault_commands::secure_reset_vault,
            commands::is_vault_unlocked,
            biometric::biometric_available,
            biometric_commands::biometric_passphrase_stored,
            biometric_commands::store_biometric_passphrase,
            biometric::unlock_with_biometric,
            biometric_commands::clear_biometric_passphrase,
            commands::list_keys,
            commands::generate_key,
            commands::import_key,
            commands::delete_key,
            commands::list_known_hosts,
            commands::remove_known_host,
            commands::list_removed_known_hosts,
            commands::forget_known_host,
            commands::list_host_key_changes,
            commands::replace_known_host,
            commands::list_workspaces,
            commands::create_workspace,
            commands::save_workspace,
            commands::delete_workspace,
            commands::set_active_workspace,
            commands::connect_ssh,
            commands::create_local_terminal,
            commands::session_write,
            commands::session_resize,
            commands::close_session,
            commands::list_sessions,
            commands::sftp_connect,
            commands::sftp_list_dir,
            commands::sftp_canonicalize,
            commands::sftp_create_dir,
            commands::sftp_remove_file,
            commands::sftp_remove_dir,
            commands::sftp_rename,
            commands::sftp_close,
            sftp_transfer::sftp_download_to_local,
            sftp_transfer::sftp_upload_from_local,
            commands::read_key_file,
            commands::get_app_info,
            commands::check_for_updates,
            commands::install_update,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Clavyn");
}

#[cfg(not(debug_assertions))]
async fn check_for_updates_silent(app: tauri::AppHandle) {
    match commands::check_with_prerelease_endpoint(&app).await {
        Ok(Some(update)) => {
            let current = app.package_info().version.to_string();
            tracing::info!("update available: v{} (current: {})", update.version, current);
            let _ = app.emit("update-available", serde_json::json!({
                "available": true,
                "version": update.version,
                "current_version": current,
                "date": update.date.map(|d| d.to_string()),
                "body": update.body,
            }));
        }
        Ok(None) => tracing::debug!("no update available"),
        Err(e) => tracing::warn!("update check failed: {e}"),
    }
}

#[cfg(test)]
mod single_instance_guard_tests {
    use super::guard_applies;

    /// Both halves of the decision, in every combination, under whichever
    /// profile the test binary happens to be built with.
    ///
    /// A debug build must never claim the guard: it carries the same bundle
    /// identifier as an installed release build, and the guard is keyed on that
    /// identifier, so claiming it would make `tauri dev` hand focus to the
    /// installed window and exit instead of starting. A release build claims it
    /// only when the plugin's D-Bus address resolves, because the plugin
    /// unwraps that address inside a setup hook and `panic = "abort"` turns the
    /// failure into a silent exit.
    #[test]
    fn only_a_release_build_with_an_addressable_bus_claims_the_guard() {
        assert!(guard_applies(false, true));
        assert!(!guard_applies(false, false));
        assert!(!guard_applies(true, true));
        assert!(!guard_applies(true, false));
    }
}
