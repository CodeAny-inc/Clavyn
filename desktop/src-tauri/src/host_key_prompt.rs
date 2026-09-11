//! Native confirmation for the two commands that can put a host back on
//! trust-on-first-use or pin a key the server just offered.
//!
//! The webview's own `confirm()` is not a control: a script calling `invoke`
//! never renders it. An OS-drawn message dialog is, because the page can
//! neither read it, click it nor dismiss it, and because the fingerprint it
//! prints comes from the store rather than from the caller's arguments.

use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

/// How long a declined confirmation refuses to open another one.
///
/// Without it a caller can drive a destructive command in a loop and put an
/// unbounded stack of dialogs in front of the user, which teaches them to click
/// the default button. With it, one refusal answers the whole burst. The cost
/// is that changing your mind about a cancellation means waiting, which is a
/// fair trade for an action taken this rarely.
const DECLINE_COOLDOWN: Duration = Duration::from_secs(30);

/// Marks the error raised when the user answered the dialog with Cancel, so the
/// view can tell "you said no" apart from a failure worth showing in red.
pub const DECLINED: &str = "[host-key-confirmation-declined]";

/// Serializes the host-key confirmations and remembers a refusal.
///
/// The cooldown is deliberately global rather than per host: a caller that
/// walks the host list would otherwise get a fresh dialog for every entry.
pub struct PromptGate {
    state: Mutex<GateState>,
}

#[derive(Default)]
struct GateState {
    open: bool,
    declined_until: Option<Instant>,
}

impl PromptGate {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(GateState::default()),
        }
    }

    /// Claim the right to show a dialog, or say why not.
    pub fn open(&self) -> Result<(), String> {
        self.open_at(Instant::now())
    }

    /// Release the claim and record the answer.
    pub fn close(&self, confirmed: bool) {
        self.close_at(confirmed, Instant::now());
    }

    fn open_at(&self, now: Instant) -> Result<(), String> {
        let mut state = self.lock();
        if state.open {
            return Err(
                "a host key confirmation is already open; answer that one first".to_string(),
            );
        }
        if let Some(until) = state.declined_until {
            if now < until {
                let seconds = (until - now).as_secs() + 1;
                return Err(format!(
                    "a host key confirmation was declined; no further host key changes will be \
                     offered for {seconds}s"
                ));
            }
            state.declined_until = None;
        }
        state.open = true;
        Ok(())
    }

    fn close_at(&self, confirmed: bool, now: Instant) {
        let mut state = self.lock();
        state.open = false;
        state.declined_until = if confirmed {
            None
        } else {
            Some(now + DECLINE_COOLDOWN)
        };
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, GateState> {
        // A panic while the gate is held would otherwise leave it permanently
        // closed; the flag it guards is two plain fields, so the state a
        // poisoned lock protects is still consistent.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Default for PromptGate {
    fn default() -> Self {
        Self::new()
    }
}

/// Show a native, OS-drawn confirmation and wait for the answer.
///
/// `Ok(())` means the user pressed the confirm button. Anything else — cancel,
/// a dialog that could not be shown, a burst the gate refused — is an error, so
/// the caller fails closed.
pub async fn confirm(
    app: &AppHandle,
    gate: &PromptGate,
    title: &str,
    message: &str,
    confirm_label: &str,
) -> Result<(), String> {
    gate.open()?;
    let confirmed = show(app, title, message, confirm_label).await;
    gate.close(confirmed);
    if confirmed {
        Ok(())
    } else {
        Err(format!("{DECLINED} {title}: cancelled"))
    }
}

async fn show(app: &AppHandle, title: &str, message: &str, confirm_label: &str) -> bool {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut builder = app
        .dialog()
        .message(message)
        .title(title)
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            confirm_label.to_string(),
            "Cancel".to_string(),
        ));
    // Own the dialog to the app window so it cannot end up behind it, which
    // would read as a frozen app rather than as a question.
    if let Some(window) = app.webview_windows().values().next() {
        builder = builder.parent(window);
    }
    builder.show(move |confirmed| {
        let _ = tx.send(confirmed);
    });
    // `show` hands the dialog to the main thread and answers through the
    // callback, so nothing blocks here and the command's runtime thread stays
    // free. A sender dropped without a send means the dialog never appeared:
    // read that as a refusal rather than as consent.
    rx.await.unwrap_or(false)
}

/// Wording for the dialog that erases the key a removed host is remembered by.
pub fn forget_message(host: &str, fingerprint: &str) -> String {
    format!(
        "Clavyn still remembers the key {host} was pinned to:\n\n    {fingerprint}\n\n\
         That record is how a changed key is told apart from a new one. Forgetting it means the \
         next connection to {host} pins whatever key answers, with no warning."
    )
}

/// Wording for the dialog that pins a key the server presented in place of the
/// recorded one. Both fingerprints are read out of the store, not out of the
/// caller's arguments.
pub fn trust_message(host: &str, pinned: &str, presented: &str) -> String {
    format!(
        "The key {host} presents is not the one Clavyn has pinned.\n\n\
         Pinned:      {pinned}\nPresented:   {presented}\n\n\
         Trust the presented key only if whoever runs {host} confirms this fingerprint."
    )
}

#[cfg(test)]
mod tests {
    use super::{PromptGate, DECLINE_COOLDOWN};
    use std::time::{Duration, Instant};

    #[test]
    fn a_second_confirmation_cannot_open_while_one_is_unanswered() {
        let gate = PromptGate::new();
        let now = Instant::now();
        gate.open_at(now).expect("first prompt");

        let error = gate.open_at(now).expect_err("a second dialog was opened");
        assert!(
            error.contains("already open"),
            "unexpected refusal: {error}"
        );
    }

    #[test]
    fn a_declined_confirmation_answers_the_burst_that_follows_it() {
        let gate = PromptGate::new();
        let now = Instant::now();
        gate.open_at(now).expect("first prompt");
        gate.close_at(false, now);

        for _ in 0..100 {
            let error = gate
                .open_at(now + Duration::from_millis(1))
                .expect_err("a declined prompt was reopened");
            assert!(error.contains("declined"), "unexpected refusal: {error}");
        }
    }

    #[test]
    fn the_cooldown_expires_so_a_refusal_is_not_permanent() {
        let gate = PromptGate::new();
        let now = Instant::now();
        gate.open_at(now).expect("first prompt");
        gate.close_at(false, now);

        gate.open_at(now + DECLINE_COOLDOWN + Duration::from_secs(1))
            .expect("the cooldown never expired");
    }

    #[test]
    fn a_confirmed_prompt_leaves_no_cooldown_behind() {
        let gate = PromptGate::new();
        let now = Instant::now();
        gate.open_at(now).expect("first prompt");
        gate.close_at(true, now);

        gate.open_at(now).expect("a confirmed prompt left a cooldown");
    }
}
