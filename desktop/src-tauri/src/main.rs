#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod biometric;
mod commands;
mod sftp_transfer;
mod state;
mod vault_commands;
mod vault_keychain_cleanup;

use state::AppState;
use tauri::Manager;
#[cfg(not(debug_assertions))]
use tauri::Emitter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "clavyn_desktop=info,clavyn_core=info".into()),
        )
        .init();

    tauri::Builder::default()
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
            vault_commands::secure_initialize_vault,
            vault_commands::secure_unlock_vault,
            vault_commands::secure_lock_vault,
            vault_commands::secure_reset_vault,
            commands::is_vault_unlocked,
            biometric::biometric_available,
            biometric::biometric_passphrase_stored,
            biometric::store_biometric_passphrase,
            biometric::unlock_with_biometric,
            biometric::clear_biometric_passphrase,
            commands::list_keys,
            commands::generate_key,
            commands::import_key,
            commands::delete_key,
            commands::list_known_hosts,
            commands::remove_known_host,
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
