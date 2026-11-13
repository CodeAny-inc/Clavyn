//! Keeps the app's webviews on the app's own pages.
//!
//! A page from anywhere else, loaded into the main window, is drawn inside
//! Clavyn's own frame, where it can pass for one of the app's screens and ask
//! for a vault passphrase or a host password. It cannot invoke the app's
//! commands, which are granted to local pages only, but it does not need to.
//!
//! Nothing in the app navigates its window: the UI is a single page, and the
//! links it shows open in the system browser through the shell plugin. So
//! every top-level navigation that would leave the app's origin is cancelled,
//! whatever started it: script in the page, or a link dropped onto the window.
//!
//! A cancelled navigation is not handed to the system browser instead. That
//! would let any script in the page open arbitrary URLs outside the scope the
//! shell plugin is configured with.
//!
//! A request for a new window is not a navigation of this webview, so
//! `window.open` and `target="_blank"` links never reach this hook. They open
//! nothing because no webview registers `on_new_window`: without that handler,
//! wry marks a WebView2 `NewWindowRequested` as handled without supplying a
//! window, its WKWebView UI delegate returns no webview, and WebKitGTK's
//! `create` signal is left to its default, which creates none. A webview that
//! registers one opens whatever its handler allows, so that handler has to
//! refuse URLs outside the app the way this hook does.

use tauri::plugin::{Builder, TauriPlugin};
use tauri::utils::config::WindowConfig;
use tauri::{Manager, Runtime, Url};

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("navigation-guard")
        .on_navigation(|webview, url| {
            let config = webview.config();
            let dev_url = tauri::is_dev()
                .then(|| config.build.dev_url.clone())
                .flatten();
            let https = uses_https_scheme(&config.app.windows, webview.label());
            let allowed = is_app_page(url, dev_url.as_ref(), cfg!(windows), https);
            if !allowed {
                tracing::warn!(
                    "blocked webview {} from navigating to {}",
                    webview.label(),
                    url.origin().ascii_serialization()
                );
            }
            allowed
        })
        .build()
}

/// Whether the window labelled `label` serves the app's pages over `https` on
/// WebView2, which is what its `useHttpsScheme` setting selects.
///
/// A webview with no window config counts as `http`, Tauri's default. A
/// webview built in code with `use_https_scheme` then has its own pages
/// refused, which shows up at once as a blank window, rather than a second
/// origin quietly accepted.
fn uses_https_scheme(windows: &[WindowConfig], label: &str) -> bool {
    windows
        .iter()
        .find(|window| window.label == label)
        .is_some_and(|window| window.use_https_scheme)
}

/// Whether `url` is one of the app's own pages.
///
/// A dev build serves them from `devUrl`. A packaged build serves them through
/// Tauri's custom protocol, which WebView2 addresses as
/// `http://tauri.localhost`, or `https://tauri.localhost` when `https` is set,
/// and the other webviews as `tauri://localhost`.
///
/// On WebView2 only the scheme the webview uses is accepted. WebView2
/// intercepts that scheme alone; a request on the other one is an ordinary
/// network request to the loopback address `*.localhost` resolves to, so
/// whatever answers it is not the app. Tauri still counts both schemes as
/// local when it checks commands, which would hand such a page the app's
/// commands if the window were allowed onto it.
///
/// The inputs that depend on the build are parameters, so every combination
/// is reachable from a single test build.
fn is_app_page(url: &Url, dev_url: Option<&Url>, webview2: bool, https: bool) -> bool {
    match dev_url {
        Some(dev) => {
            url.scheme() == dev.scheme()
                && url.host() == dev.host()
                && url.port_or_known_default() == dev.port_or_known_default()
        }
        None if webview2 => {
            let scheme = if https { "https" } else { "http" };
            url.scheme() == scheme
                && url.host_str() == Some("tauri.localhost")
                && url.port().is_none()
        }
        None => url.scheme() == "tauri" && url.host_str() == Some("localhost"),
    }
}

#[cfg(test)]
mod tests {
    use super::{is_app_page, uses_https_scheme};
    use tauri::utils::config::WindowConfig;
    use tauri::Url;

    /// The app pages in `allowed` pass and every URL in `refused` fails.
    fn check(
        dev_url: Option<&str>,
        webview2: bool,
        https: bool,
        allowed: &[&str],
        refused: &[&str],
    ) {
        let dev_url = dev_url.map(|u| Url::parse(u).unwrap());
        for url in allowed {
            let parsed = Url::parse(url).unwrap();
            assert!(
                is_app_page(&parsed, dev_url.as_ref(), webview2, https),
                "{url}"
            );
        }
        for url in refused {
            let parsed = Url::parse(url).unwrap();
            assert!(
                !is_app_page(&parsed, dev_url.as_ref(), webview2, https),
                "{url}"
            );
        }
    }

    #[test]
    fn a_dev_build_stays_on_the_dev_server() {
        for webview2 in [true, false] {
            for https in [true, false] {
                check(
                    Some("http://localhost:1420"),
                    webview2,
                    https,
                    &[
                        "http://localhost:1420/",
                        "http://localhost:1420/index.html?x#y",
                    ],
                    &[
                        "http://localhost:1421/",
                        "https://localhost:1420/",
                        "http://127.0.0.1:1420/",
                        "http://tauri.localhost/",
                        "tauri://localhost/",
                        "https://github.com/",
                    ],
                );
            }
        }
    }

    #[test]
    fn a_packaged_build_on_webview2_stays_on_http_tauri_localhost() {
        check(
            None,
            true,
            false,
            &[
                "http://tauri.localhost/",
                "http://tauri.localhost/index.html",
            ],
            &[
                "https://tauri.localhost/",
                "https://tauri.localhost/index.html",
                "http://tauri.localhost:8080/",
                "http://evil.tauri.localhost/",
                "http://localhost/",
                "tauri://localhost/",
                "https://github.com/CodeAny-inc/Clavyn",
                "file:///C:/Windows/win.ini",
                "data:text/html,hi",
            ],
        );
    }

    #[test]
    fn a_packaged_build_on_webview2_with_https_stays_on_https_tauri_localhost() {
        check(
            None,
            true,
            true,
            &[
                "https://tauri.localhost/",
                "https://tauri.localhost/index.html",
            ],
            &[
                "http://tauri.localhost/",
                "http://tauri.localhost/index.html",
                "https://tauri.localhost:8443/",
                "https://evil.tauri.localhost/",
                "tauri://localhost/",
                "https://github.com/CodeAny-inc/Clavyn",
            ],
        );
    }

    #[test]
    fn a_packaged_build_elsewhere_stays_on_the_tauri_scheme() {
        for https in [true, false] {
            check(
                None,
                false,
                https,
                &["tauri://localhost/", "tauri://localhost/index.html"],
                &[
                    "tauri://evil/",
                    "http://tauri.localhost/",
                    "https://tauri.localhost/",
                    "https://github.com/CodeAny-inc/Clavyn",
                    "file:///etc/passwd",
                    "about:blank",
                ],
            );
        }
    }

    /// The scheme comes from the window config with the webview's label, and
    /// a label with no config falls back to Tauri's `http` default.
    #[test]
    fn the_scheme_follows_the_window_config() {
        let window = |label: &str, use_https_scheme: bool| WindowConfig {
            label: label.to_string(),
            use_https_scheme,
            ..WindowConfig::default()
        };
        let windows = [window("main", true), window("plain", false)];

        assert!(uses_https_scheme(&windows, "main"));
        assert!(!uses_https_scheme(&windows, "plain"));
        assert!(!uses_https_scheme(&windows, "unknown"));
        assert!(!uses_https_scheme(&[], "main"));
    }
}
