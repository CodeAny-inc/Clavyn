//! Which pages and windows can invoke the app's own commands.
//!
//! The command list lives in three places: `generate_handler!` in
//! `src/main.rs`, the permissions in `permissions/app-commands.toml`, and the
//! grants in `capabilities/default.json`. They are read here as they ship rather than
//! restated, because a mismatch between them is silent until the command is
//! called: a handler command with no grant is rejected at runtime with a
//! permission error, which no build step reports.

use std::collections::BTreeSet;
use std::path::Path;

/// Command names in the first `generate_handler![...]` of `source`, without
/// their module path.
///
/// The macro accepts outer attributes such as `#[cfg(...)]` on an entry, and
/// the list can hold `//` comments. Both are dropped before the paths are read,
/// and the list ends at the `]` that closes the macro, not at the first `]`,
/// which an attribute would supply.
fn handler_list(source: &str) -> Vec<String> {
    const OPEN: &str = "generate_handler![";
    let start = source.find(OPEN).expect("generate_handler! not found") + OPEN.len();
    let code = source[start..]
        .lines()
        .map(|line| line.split("//").next().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    let mut list = String::new();
    // Nesting depth inside `#[...]`; zero means a path is being read.
    let mut attribute = 0usize;
    let mut chars = code.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '#' if attribute == 0 && chars.peek() == Some(&'[') => {
                chars.next();
                attribute = 1;
            }
            '[' if attribute > 0 => attribute += 1,
            ']' if attribute > 0 => attribute -= 1,
            ']' => break,
            _ if attribute > 0 => {}
            _ => list.push(c),
        }
    }
    list.split(',')
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(|path| path.rsplit("::").next().unwrap().to_string())
        .collect()
}

/// Command names registered in `generate_handler!` in `src/main.rs`.
fn handler_commands() -> Vec<String> {
    handler_list(include_str!("../src/main.rs"))
}

/// The `[[permission]]` entries in `permissions/app-commands.toml`, as
/// (identifier, allowed commands). The file is kept to those two keys, so a
/// line-based read is exact; any other line fails here rather than being
/// skipped.
fn app_permissions() -> Vec<(String, Vec<String>)> {
    let quoted = |value: &str| -> Vec<String> {
        value
            .split('"')
            .skip(1)
            .step_by(2)
            .map(str::to_string)
            .collect()
    };
    let mut permissions: Vec<(String, Vec<String>)> = Vec::new();
    for line in include_str!("../permissions/app-commands.toml").lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line == "[[permission]]" {
            continue;
        }
        if let Some(value) = line.strip_prefix("identifier = ") {
            permissions.push((quoted(value).concat(), Vec::new()));
        } else if let Some(value) = line.strip_prefix("commands.allow = ") {
            permissions
                .last_mut()
                .expect("commands.allow follows an identifier")
                .1 = quoted(value);
        } else {
            panic!("unexpected line in app-commands.toml: {line}");
        }
    }
    permissions
}

fn capability() -> serde_json::Value {
    serde_json::from_str(include_str!("../capabilities/default.json"))
        .expect("capabilities/default.json")
}

fn granted_permissions() -> Vec<String> {
    capability()["permissions"]
        .as_array()
        .expect("a permissions list")
        .iter()
        .map(|p| p.as_str().expect("permissions are plain identifiers"))
        .map(str::to_string)
        .collect()
}

/// App permissions the capability grants. Plugin permissions are prefixed
/// with the plugin's name (`shell:`, `core:event:`); app permissions are not.
fn granted_app_permissions() -> Vec<String> {
    granted_permissions()
        .into_iter()
        .filter(|p| !p.contains(':'))
        .collect()
}

fn unique(names: &[String], what: &str) -> BTreeSet<String> {
    let set: BTreeSet<String> = names.iter().cloned().collect();
    assert_eq!(set.len(), names.len(), "{what} lists a command twice");
    set
}

/// An attribute on an entry or a comment in the list neither ends the list
/// early nor reads as a command.
#[test]
fn the_handler_list_reader_skips_attributes_and_comments() {
    let source = r#"
        .invoke_handler(tauri::generate_handler![
            commands::first,
            // commands::commented_out,
            #[cfg(target_os = "macos")]
            biometric::second, // trailing note
            #[cfg(all(feature = "x", not(test)))] third,
            commands::fourth,
        ])
        .run(context);
        other_macro![not_a_command];
    "#;
    assert_eq!(handler_list(source), ["first", "second", "third", "fourth"]);
}

/// Tauri enables every file under `capabilities/` unless
/// `app.security.capabilities` in tauri.conf.json names a subset or defines
/// capabilities inline. The checks in this file read `default.json` alone, so
/// a capability anywhere else would grant commands they never see.
#[test]
fn default_json_is_the_only_capability() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("capabilities");
    let entries: Vec<String> = std::fs::read_dir(&dir)
        .expect("capabilities/")
        .map(|entry| {
            entry
                .expect("an entry in capabilities/")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        entries,
        ["default.json"],
        "a capability other than default.json needs the checks in this file extended to it"
    );

    let config: serde_json::Value =
        serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json");
    assert!(
        config["app"]["security"].get("capabilities").is_none(),
        "app.security.capabilities would replace or add to capabilities/default.json"
    );
}

/// One `allow-<command>` permission per registered command, each allowing
/// that command and nothing else.
#[test]
fn each_registered_command_has_its_own_permission() {
    let mut declared = Vec::new();
    for (identifier, commands) in app_permissions() {
        assert_eq!(commands.len(), 1, "{identifier} should allow one command");
        let expected = format!("allow-{}", commands[0].replace('_', "-"));
        assert_eq!(identifier, expected, "named after the command it allows");
        declared.push(commands[0].clone());
    }
    let declared = unique(&declared, "app-commands.toml");
    let handler = unique(&handler_commands(), "generate_handler!");
    assert!(!handler.is_empty());
    assert_eq!(
        handler.difference(&declared).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "registered in main.rs but missing from app-commands.toml"
    );
    assert_eq!(
        declared.difference(&handler).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "in app-commands.toml but not registered in main.rs"
    );
}

/// Every registered command is granted, through its own `allow-` permission
/// and nothing broader, so the capability file reads as the full list of what
/// the window can call.
#[test]
fn the_capability_grants_exactly_the_registered_commands() {
    let expected: BTreeSet<String> = handler_commands()
        .iter()
        .map(|name| format!("allow-{}", name.replace('_', "-")))
        .collect();
    let granted = unique(&granted_app_permissions(), "capabilities/default.json");
    assert_eq!(
        expected.difference(&granted).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "registered commands with no grant are rejected at runtime"
    );
    assert_eq!(
        granted.difference(&expected).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "app permissions granted that are not one allow- per registered command"
    );
}

/// The grant covers the main window and the app's own pages only. A `remote`
/// entry would hand the commands to whatever URL it matches.
#[test]
fn the_capability_is_limited_to_the_main_window_and_local_pages() {
    let capability = capability();
    assert_eq!(capability["windows"], serde_json::json!(["main"]));
    assert!(capability.get("webviews").is_none());
    assert!(capability.get("remote").is_none());
    assert_ne!(capability.get("local"), Some(&serde_json::json!(false)));
}

/// Plugin commands are granted one by one, for what the frontend calls. A
/// plugin's `default` set is wider than its name suggests: `core:default`
/// alone lets the page replace the app and window menus and resolve the
/// user's home and data directories.
#[test]
fn plugin_permissions_are_the_ones_the_frontend_uses() {
    let plugin: Vec<String> = granted_permissions()
        .into_iter()
        .filter(|p| p.contains(':'))
        .collect();
    assert_eq!(
        plugin,
        [
            // `listen` in src/api.ts, and the unlisten it returns.
            "core:event:allow-listen",
            "core:event:allow-unlisten",
            // The devtools hotkey Tauri injects into debug builds. Release
            // builds do not register the command, so the grant is inert there.
            "core:webview:allow-internal-toggle-devtools",
            // A native message dialog, which the dialog plugin's replacement
            // for `window.alert` calls. The app asks its questions in the page
            // (src/composables/useConfirm.ts) and shows errors inline, so it
            // makes no call of its own. The file dialogs (`open`, `save`) stay
            // ungranted; the backend shows those itself.
            "dialog:allow-message",
            // `open` from @tauri-apps/plugin-shell, scoped in tauri.conf.json to
            // the project's GitHub page and its releases.
            "shell:allow-open",
        ]
    );
}
