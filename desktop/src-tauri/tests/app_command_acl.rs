//! Which pages and windows can invoke the app's own commands.
//!
//! The command list lives in three places: `generate_handler!` in
//! `src/main.rs`, the permissions in `permissions/app-commands.toml`, and the
//! grants in `capabilities/default.json`. They are read here as they ship rather than
//! restated, because a mismatch between them is silent until the command is
//! called: a handler command with no grant is rejected at runtime with a
//! permission error, which no build step reports.

use std::collections::BTreeSet;

/// The text between `open` and the next `close` after it.
fn between<'a>(text: &'a str, open: &str, close: &str) -> &'a str {
    let start = text
        .find(open)
        .unwrap_or_else(|| panic!("{open} not found"))
        + open.len();
    let len = text[start..]
        .find(close)
        .unwrap_or_else(|| panic!("{close} not found after {open}"));
    &text[start..start + len]
}

/// Command names registered in `generate_handler!`, without their module path.
fn handler_commands() -> Vec<String> {
    between(include_str!("../src/main.rs"), "generate_handler![", "]")
        .split(',')
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(|path| path.rsplit("::").next().unwrap().to_string())
        .collect()
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

/// App permissions the capability grants. Plugin permissions carry a
/// `plugin:` prefix; app permissions have none.
fn granted_app_permissions() -> Vec<String> {
    capability()["permissions"]
        .as_array()
        .expect("a permissions list")
        .iter()
        .map(|p| p.as_str().expect("permissions are plain identifiers"))
        .filter(|p| !p.contains(':'))
        .map(str::to_string)
        .collect()
}

fn unique(names: &[String], what: &str) -> BTreeSet<String> {
    let set: BTreeSet<String> = names.iter().cloned().collect();
    assert_eq!(set.len(), names.len(), "{what} lists a command twice");
    set
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
