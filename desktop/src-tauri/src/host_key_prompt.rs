//! Native confirmation for the two commands that can put a host back on
//! trust-on-first-use or pin a key the server just offered.
//!
//! The webview's own `confirm()` is not a control: a script calling `invoke`
//! never renders it. An OS-drawn message dialog is, because the page can
//! neither read it, click it nor dismiss it, and because the fingerprint it
//! prints comes from the store rather than from the caller's arguments.
//!
//! The gate and the dialog are not specific to host keys: `host_change_prompt`
//! asks its own question through them, behind a gate of its own.

use std::future::Future;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

/// How long a declined host key confirmation refuses to open another one.
///
/// Without it a caller can drive a destructive command in a loop and put an
/// unbounded stack of dialogs in front of the user, which teaches them to click
/// the default button. With it, one refusal answers the whole burst. The cost
/// is that changing your mind about a cancellation means waiting, which is a
/// fair trade for an action taken this rarely.
const DECLINE_COOLDOWN: Duration = Duration::from_secs(30);

/// How long a dialog has to be on screen before its confirm button counts.
///
/// The confirm button has the focus, and the page sees every key typed into a
/// terminal, so it can open a dialog an instant before a keystroke it expects
/// (the Enter that ends a command, the space between two words) and let that
/// keystroke land on the button. A confirmation that comes back sooner than
/// this cannot have followed from reading the dialog, so it does not count and
/// the dialog is shown again, saying why. Cancelling takes effect at once.
const READ_DELAY: Duration = Duration::from_secs(1);

/// Marks the error raised when the user answered a host key dialog with Cancel,
/// so the view can tell "you said no" apart from a failure worth showing in red.
///
/// Only a real answer carries this tag. A dialog that could not be shown is
/// refused too, but it is a failure the user has to be told about: silently
/// treating it as a cancellation would make the button do nothing at all.
pub const DECLINED: &str = "[host-key-confirmation-declined]";

/// Marks the refusal a gate gives while a recent Cancel still holds it shut, so
/// a view can say "try again shortly" instead of reporting a failure. Nothing
/// was asked, so it is not a `DECLINED` answer either.
pub const COOLING_DOWN: &str = "[confirmation-cooling-down]";

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

/// Serializes one kind of confirmation and remembers a refusal.
///
/// The cooldown is deliberately global rather than per host: a caller that
/// walks the host list would otherwise get a fresh dialog for every entry.
pub struct PromptGate {
    state: Mutex<GateState>,
    /// What the dialogs behind this gate ask about, for the refusals it gives.
    subject: &'static str,
    /// The tag a cancelled dialog's error carries; see `DECLINED`.
    declined: &'static str,
    /// How long a refusal keeps the gate shut; see `DECLINE_COOLDOWN`.
    cooldown: Duration,
}

#[derive(Default)]
struct GateState {
    open: bool,
    declined_until: Option<Instant>,
}

impl PromptGate {
    /// The gate for the host key dialogs.
    pub fn new() -> Self {
        Self::for_subject("host key", DECLINED, DECLINE_COOLDOWN)
    }

    /// A gate of its own for another kind of question, so that refusing one
    /// kind does not silence the other, a view can tell which was refused, and
    /// each kind holds off for as long as suits how it is asked.
    pub fn for_subject(subject: &'static str, declined: &'static str, cooldown: Duration) -> Self {
        Self {
            state: Mutex::new(GateState::default()),
            subject,
            declined,
            cooldown,
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
        let subject = self.subject;
        let mut state = self.lock();
        if state.open {
            return Err(format!(
                "a {subject} confirmation is already open; answer that one first"
            ));
        }
        if let Some(until) = state.declined_until {
            if now < until {
                let seconds = (until - now).as_secs() + 1;
                return Err(format!(
                    "{COOLING_DOWN} a {subject} confirmation was declined; no further {subject} \
                     confirmations will be offered for {seconds}s"
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
        // nobody to click through, so silencing the next few seconds over it
        // would hide the failure as well as the question.
        state.declined_until = match answer {
            Answer::Declined => Some(now + self.cooldown),
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
/// `Ok(())` means the user pressed the confirm button once the dialog had been
/// up long enough to read (`READ_DELAY`). Everything else is an error, so the
/// caller fails closed — but the errors are told apart: a cancellation carries
/// the gate's tag because it is an answer, while a dialog that could not be
/// shown does not, because nothing was asked and the user has to be told that
/// the button they pressed did nothing.
pub async fn confirm(
    app: &AppHandle,
    gate: &PromptGate,
    title: &str,
    message: &str,
    confirm_label: &str,
) -> Result<(), String> {
    let mut claim = gate.open()?;
    let answer = read_answer(message, confirm_label, Instant::now, |text| {
        show(app, title, text, confirm_label)
    })
    .await;
    claim.answered(answer);
    match answer {
        Answer::Confirmed => Ok(()),
        Answer::Declined => Err(format!("{} {title}: cancelled", gate.declined)),
        Answer::Unavailable => Err(format!(
            "{title}: the confirmation dialog could not be shown, so nothing was changed"
        )),
    }
}

/// Show the dialog through `show` until it comes back with an answer that can
/// have followed from reading it; see `READ_DELAY`.
///
/// The loop has no bound because every pass waits on a person: an Allow that
/// keeps arriving too soon is a key held down, and the dialog stays until it
/// is released and the dialog is answered.
async fn read_answer<S, Fut>(
    message: &str,
    confirm_label: &str,
    now: impl Fn() -> Instant,
    mut show: S,
) -> Answer
where
    S: FnMut(String) -> Fut,
    Fut: Future<Output = Answer>,
{
    let mut text = message.to_string();
    loop {
        let opened = now();
        let answer = show(text).await;
        if answer != Answer::Confirmed || now().duration_since(opened) >= READ_DELAY {
            return answer;
        }
        text = format!(
            "\"{confirm_label}\" was pressed as this dialog opened, before it could be read, so it \
             did not count.\n\n{message}"
        );
    }
}

async fn show(app: &AppHandle, title: &str, message: String, confirm_label: &str) -> Answer {
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
    use super::{read_answer, Answer, PromptGate, COOLING_DOWN, DECLINE_COOLDOWN, READ_DELAY};
    use std::cell::{Cell, RefCell};
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
            assert!(error.starts_with(COOLING_DOWN), "untagged refusal: {error}");
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
    fn a_refusal_behind_one_gate_does_not_silence_another() {
        let host_keys = PromptGate::new();
        let other = PromptGate::for_subject("host change", "[other-declined]", DECLINE_COOLDOWN);
        let now = Instant::now();
        host_keys.open_at(now).expect("first prompt");
        host_keys.close_at(Answer::Declined, now);
        host_keys
            .open_at(now)
            .expect_err("a refusal left no cooldown, so this test proves nothing");

        other.open_at(now).expect("the other gate was silenced");
        other.close_at(Answer::Declined, now);
        let refusal = other
            .open_at(now)
            .expect_err("the other gate kept no cooldown");
        assert!(
            refusal.contains("host change confirmation was declined"),
            "unexpected refusal: {refusal}"
        );
    }

    #[test]
    fn each_gate_holds_off_for_its_own_cooldown() {
        let short = Duration::from_secs(5);
        let gate = PromptGate::for_subject("host change", "[other-declined]", short);
        let now = Instant::now();
        gate.open_at(now).expect("first prompt");
        gate.close_at(Answer::Declined, now);

        gate.open_at(now + short - Duration::from_millis(1))
            .expect_err("the cooldown ended early");
        gate.open_at(now + short)
            .expect("the gate kept the host key cooldown instead of its own");
    }

    /// Answers each dialog after `delays[n]`, on a clock the test controls, and
    /// records the text each one showed.
    async fn answer_after(delays: &[Duration], answer: Answer) -> (Answer, Vec<String>) {
        let clock = Cell::new(Instant::now());
        let shown = RefCell::new(Vec::new());
        let result = read_answer(
            "Body",
            "Allow",
            || clock.get(),
            |text| {
                let delay = delays[shown.borrow().len()];
                shown.borrow_mut().push(text);
                clock.set(clock.get() + delay);
                async move { answer }
            },
        )
        .await;
        (result, shown.into_inner())
    }

    #[tokio::test]
    async fn a_confirmation_given_before_the_dialog_could_be_read_does_not_count() {
        let quick = Duration::from_millis(150);
        let (answer, shown) = answer_after(&[quick, quick, READ_DELAY], Answer::Confirmed).await;

        assert_eq!(answer, Answer::Confirmed);
        assert_eq!(
            shown.len(),
            3,
            "an Allow pressed as the dialog opened was taken"
        );
        assert_eq!(shown[0], "Body");
        for again in &shown[1..] {
            assert!(
                again.starts_with("\"Allow\" was pressed as this dialog opened"),
                "{again}"
            );
            assert!(again.ends_with("\n\nBody"), "{again}");
        }
    }

    #[tokio::test]
    async fn a_confirmation_after_reading_is_taken_the_first_time() {
        let (answer, shown) = answer_after(&[READ_DELAY], Answer::Confirmed).await;
        assert_eq!(answer, Answer::Confirmed);
        assert_eq!(shown, ["Body"]);
    }

    #[tokio::test]
    async fn cancelling_takes_effect_at_once() {
        for answer in [Answer::Declined, Answer::Unavailable] {
            let (result, shown) = answer_after(&[Duration::ZERO], answer).await;
            assert_eq!(result, answer);
            assert_eq!(shown.len(), 1);
        }
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
