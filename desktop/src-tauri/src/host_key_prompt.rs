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
///
/// Only a real answer carries this tag. A dialog that could not be shown is
/// refused too, but it is a failure the user has to be told about: silently
/// treating it as a cancellation would make the button do nothing at all.
pub const DECLINED: &str = "[host-key-confirmation-declined]";

/// What came back from a confirmation dialog.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Answer {
    /// The user pressed the confirm button.
    Confirmed,
    /// The user cancelled, closed the dialog or pressed Escape.
    Declined,
    /// The dialog never appeared, so nobody was asked anything.
    Unavailable,
}

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
    ///
    /// The claim is released by the returned guard, on the way out of `confirm`
    /// or on the way out of a future that was dropped mid-dialog. Closing it by
    /// hand would leave the gate claimed for the rest of the process if the
    /// command between the two calls never finished.
    pub fn open(&self) -> Result<Claim<'_>, String> {
        self.open_at(Instant::now())?;
        Ok(Claim {
            gate: self,
            answer: Answer::Unavailable,
        })
    }

    /// Release the claim and record the answer.
    pub fn close(&self, answer: Answer) {
        self.close_at(answer, Instant::now());
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

    fn close_at(&self, answer: Answer, now: Instant) {
        let mut state = self.lock();
        state.open = false;
        // Only a refusal arms the cooldown. A dialog that never appeared taught
        // nobody to click through, so silencing the next 30 seconds over it
        // would hide the failure as well as the question.
        state.declined_until = match answer {
            Answer::Declined => Some(now + DECLINE_COOLDOWN),
            Answer::Confirmed | Answer::Unavailable => None,
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

/// An open claim on the gate. Releases it on drop with whatever answer it is
/// carrying, so a confirmation future that is dropped before the dialog answers
/// leaves the gate usable instead of claimed for good.
pub struct Claim<'a> {
    gate: &'a PromptGate,
    answer: Answer,
}

impl Claim<'_> {
    /// Record the answer the dialog came back with. Until this is called the
    /// claim releases as `Unavailable`, which is what a dropped future is.
    fn answered(&mut self, answer: Answer) {
        self.answer = answer;
    }
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        self.gate.close(self.answer);
    }
}

/// Show a native, OS-drawn confirmation and wait for the answer.
///
/// `Ok(())` means the user pressed the confirm button. Everything else is an
/// error, so the caller fails closed — but the errors are told apart: a
/// cancellation carries `DECLINED` because it is an answer, while a dialog that
/// could not be shown does not, because nothing was asked and the user has to
/// be told that the button they pressed did nothing.
pub async fn confirm(
    app: &AppHandle,
    gate: &PromptGate,
    title: &str,
    message: &str,
    confirm_label: &str,
) -> Result<(), String> {
    let mut claim = gate.open()?;
    let answer = show(app, title, message, confirm_label).await;
    claim.answered(answer);
    match answer {
        Answer::Confirmed => Ok(()),
        Answer::Declined => Err(format!("{DECLINED} {title}: cancelled")),
        Answer::Unavailable => Err(format!(
            "{title}: the confirmation dialog could not be shown, so nothing was changed"
        )),
    }
}

async fn show(app: &AppHandle, title: &str, message: &str, confirm_label: &str) -> Answer {
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
    // would read as a frozen app rather than as a question. The main window is
    // named rather than picked out of the map, whose order is arbitrary.
    if let Some(window) = app.get_webview_window("main") {
        builder = builder.parent(&window);
    }
    builder.show(move |confirmed| {
        let _ = tx.send(confirmed);
    });
    // `show` hands the dialog to the main thread and answers through the
    // callback, so nothing blocks here and the command's runtime thread stays
    // free. A sender dropped without a send means the dialog never appeared:
    // that is not consent, and it is not a refusal either.
    match rx.await {
        Ok(true) => Answer::Confirmed,
        Ok(false) => Answer::Declined,
        Err(_) => Answer::Unavailable,
    }
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
    use super::{Answer, PromptGate, DECLINE_COOLDOWN};
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
        gate.close_at(Answer::Declined, now);

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
        gate.close_at(Answer::Declined, now);

        // A gate that never refuses anything would pass the line below on its
        // own, so check that the cooldown is real a moment before it lapses.
        gate.open_at(now + DECLINE_COOLDOWN - Duration::from_secs(1))
            .expect_err("the cooldown ended early");
        gate.open_at(now + DECLINE_COOLDOWN + Duration::from_secs(1))
            .expect("the cooldown never expired");
    }

    #[test]
    fn a_confirmed_prompt_leaves_no_cooldown_behind() {
        let now = Instant::now();

        // The same sequence with a refusal in place of the confirmation, so
        // what the assertion below proves is the answer and not the clock.
        let declined = PromptGate::new();
        declined.open_at(now).expect("first prompt");
        declined.close_at(Answer::Declined, now);
        declined
            .open_at(now)
            .expect_err("a refusal left no cooldown, so this test proves nothing");

        let gate = PromptGate::new();
        gate.open_at(now).expect("first prompt");
        gate.close_at(Answer::Confirmed, now);
        gate.open_at(now).expect("a confirmed prompt left a cooldown");
    }

    #[test]
    fn a_dialog_that_could_not_be_shown_leaves_no_cooldown_behind() {
        let gate = PromptGate::new();
        let now = Instant::now();
        gate.open_at(now).expect("first prompt");
        gate.close_at(Answer::Unavailable, now);

        // Nobody was asked anything, so there is no refusal to honour. Arming
        // the cooldown here would silence the next 30 seconds of a failure the
        // user is supposed to be seeing.
        gate.open_at(now)
            .expect("a dialog that never appeared left a cooldown");
    }

    #[test]
    fn a_confirmation_dropped_before_it_is_answered_releases_the_gate() {
        let gate = PromptGate::new();
        {
            let _claim = gate.open().expect("first prompt");
            assert!(
                gate.open().is_err(),
                "the claim was not held while it was alive"
            );
        }

        // A future dropped mid-dialog used to leave `open` set for the life of
        // the process, which refused every later confirmation.
        gate.open()
            .expect("a dropped confirmation wedged the gate closed");
    }
}
