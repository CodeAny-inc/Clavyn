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

use tauri::plugin::{Builder, TauriPlugin};
use tauri::{Manager, Runtime, Url};

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("navigation-guard")
        .on_navigation(|webview, url| {
            let dev_url = tauri::is_dev()
                .then(|| webview.config().build.dev_url.clone())
                .flatten();
            let allowed = is_app_page(url, dev_url.as_ref(), cfg!(windows));
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

/// Whether `url` is one of the app's own pages.
///
/// A dev build serves them from `devUrl`. A packaged build serves them through
/// Tauri's custom protocol, which WebView2 addresses as
/// `http://tauri.localhost` (or `https://` with `useHttpsScheme`) and the
/// other webviews as `tauri://localhost`.
///
/// Both inputs that depend on the build are parameters, so every combination
/// is reachable from a single test build.
fn is_app_page(url: &Url, dev_url: Option<&Url>, webview2: bool) -> bool {
    match dev_url {
        Some(dev) => {
            url.scheme() == dev.scheme()
                && url.host() == dev.host()
                && url.port_or_known_default() == dev.port_or_known_default()
        }
        None if webview2 => {
            matches!(url.scheme(), "http" | "https")
                && url.host_str() == Some("tauri.localhost")
                && url.port().is_none()
        }
        None => url.scheme() == "tauri" && url.host_str() == Some("localhost"),
    }
}

#[cfg(test)]
mod tests {
    use super::is_app_page;
    use tauri::Url;

    /// The app pages in `allowed` pass and every URL in `refused` fails.
    fn check(dev_url: Option<&str>, webview2: bool, allowed: &[&str], refused: &[&str]) {
        let dev_url = dev_url.map(|u| Url::parse(u).unwrap());
        for url in allowed {
            let parsed = Url::parse(url).unwrap();
            assert!(is_app_page(&parsed, dev_url.as_ref(), webview2), "{url}");
        }
        for url in refused {
            let parsed = Url::parse(url).unwrap();
            assert!(!is_app_page(&parsed, dev_url.as_ref(), webview2), "{url}");
        }
    }

    #[test]
    fn a_dev_build_stays_on_the_dev_server() {
        for webview2 in [true, false] {
            check(
                Some("http://localhost:1420"),
                webview2,
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

    #[test]
    fn a_packaged_build_on_webview2_stays_on_tauri_localhost() {
        check(
            None,
            true,
            &[
                "http://tauri.localhost/",
                "https://tauri.localhost/index.html",
            ],
            &[
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
    fn a_packaged_build_elsewhere_stays_on_the_tauri_scheme() {
        check(
            None,
            false,
            &["tauri://localhost/", "tauri://localhost/index.html"],
            &[
                "tauri://evil/",
                "http://tauri.localhost/",
                "https://github.com/CodeAny-inc/Clavyn",
                "file:///etc/passwd",
                "about:blank",
            ],
        );
    }
}
