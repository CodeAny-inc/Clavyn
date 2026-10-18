//! The capability ACL as the shipped context applies it to a real invoke.
//!
//! `tests/app_command_acl.rs` checks that the command lists agree; this
//! checks what they add up to at runtime, through the same `on_message` path
//! the webview's IPC takes. It lives in the binary rather than under
//! `tests/` because a test executable that links Tauri needs the Windows
//! application manifest the build script attaches to binaries only.

use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{
    get_ipc_response, mock_builder, mock_context, noop_assets, MockRuntime, INVOKE_KEY,
};
use tauri::webview::InvokeRequest;
use tauri::{App, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// Stands in for the app command of the same name. The ACL is keyed on the
/// command name alone, and the real handler takes the Wry runtime's
/// `AppHandle`, which the mock runtime cannot provide.
#[tauri::command]
fn get_app_info() -> &'static str {
    "info"
}

fn invoke_from(window: &WebviewWindow<MockRuntime>, url: &str) -> Result<(), String> {
    get_ipc_response(
        window,
        InvokeRequest {
            cmd: "get_app_info".into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: url.parse().unwrap(),
            body: InvokeBody::default(),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        },
    )
    .map(|_| ())
    .map_err(|error| error.to_string())
}

fn window(app: &App<MockRuntime>, label: &str) -> WebviewWindow<MockRuntime> {
    app.get_webview_window(label).unwrap_or_else(|| {
        WebviewWindowBuilder::new(app, label, WebviewUrl::default())
            .build()
            .unwrap()
    })
}

/// The shipped context, with its ACL, decides who reaches an app command.
#[test]
fn only_the_main_window_on_a_local_page_reaches_app_commands() {
    let app = mock_builder()
        .invoke_handler(tauri::generate_handler![get_app_info])
        // `test = true` leaves out the macOS Info.plist embed, a fixed
        // symbol that `main`'s own context already defines in this binary.
        .build(tauri::generate_context!(test = true))
        .unwrap();
    // Test builds are dev builds, where the app's pages are served from devUrl.
    let local = app
        .config()
        .build
        .dev_url
        .clone()
        .expect("a devUrl")
        .to_string();
    let main = window(&app, "main");
    let other = window(&app, "other");

    assert_eq!(invoke_from(&main, &local), Ok(()));

    let from_other_window = invoke_from(&other, &local).unwrap_err();
    assert!(
        from_other_window.contains("not allowed"),
        "{from_other_window}"
    );

    let from_remote_page = invoke_from(&main, "https://example.com/").unwrap_err();
    assert!(
        from_remote_page.contains("not allowed"),
        "{from_remote_page}"
    );
}

/// The same call against a context with no app manifest: any window on a
/// local page reaches the command. This is what `permissions/app-commands.toml`
/// changes, and why the test above is not vacuous.
#[test]
fn without_an_app_manifest_any_local_window_reaches_app_commands() {
    let app = mock_builder()
        .invoke_handler(tauri::generate_handler![get_app_info])
        .build(mock_context(noop_assets()))
        .unwrap();
    let other = window(&app, "other");
    let local = if cfg!(windows) {
        "http://tauri.localhost/"
    } else {
        "tauri://localhost/"
    };

    assert_eq!(invoke_from(&other, local), Ok(()));
}
