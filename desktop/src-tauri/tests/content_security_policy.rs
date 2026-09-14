//! The shipped Content Security Policy.
//!
//! Kept out of the crate because it tests a file, not code: `tauri.conf.json`
//! is read as it ships rather than restated here, so this cannot pass against
//! a copy that has drifted.

/// The shipped policy.
fn csp() -> String {
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json");
    config["app"]["security"]["csp"]
        .as_str()
        .expect("a csp is declared")
        .to_string()
}

/// These are the directives that do not fall back to `default-src`, plus
/// the two that do but whose fallback is wider than intended. Each one
/// closes a way to reach the network or the filesystem that would
/// otherwise be open, and leaving any of them off is silent: the page
/// renders identically either way.
#[test]
fn every_directive_that_does_not_inherit_is_declared() {
    let csp = csp();
    for directive in [
        "default-src 'self'",
        "base-uri 'none'",
        "form-action 'none'",
        "frame-ancestors 'none'",
        "object-src 'none'",
        "frame-src 'none'",
        // Covers workers and nested contexts, which frame-src does not.
        // Nothing here creates either, so this is what makes a future one
        // fail closed instead of inheriting default-src.
        "child-src 'none'",
        "img-src 'self' data:",
        "connect-src 'self' ipc: http://ipc.localhost",
    ] {
        assert!(csp.contains(directive), "{directive} is missing from {csp}");
    }
}

/// Tauri only rewrites a directive when the built assets yield hashes for
/// it, and the built page has no inline script, so leaving `script-src`
/// out is what lets Tauri synthesise it as `'self'` plus its bootstrap
/// hashes. Declaring one here would be overwritten or would fight it.
#[test]
fn script_src_is_left_to_the_builder() {
    assert!(!csp().contains("script-src"));
}
