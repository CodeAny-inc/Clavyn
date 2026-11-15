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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
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

/// What the host key gate holds off for after each refusal in a row.
const DECLINE_COOLDOWNS: &[Duration] = &[DECLINE_COOLDOWN];

/// How long without a refusal a gate waits before it forgets the ones it has
/// counted, so one Cancel last week does not start today's cooldown a step up.
const DECLINE_MEMORY: Duration = Duration::from_secs(15 * 60);

/// How long a dialog has to be up before its confirm button counts.
///
/// The confirm button has the focus, and the page sees every key typed into a
/// terminal, so it can open a dialog an instant before a keystroke it expects
/// (the Enter that ends a command, the space between two words) and let that
/// keystroke land on the button. A confirmation that comes back sooner than
/// this cannot have followed from reading the dialog, so it does not count and
/// the dialog is shown again, saying why. Cancelling takes effect at once.
///
/// The clock starts when the dialog is requested. The dialog plugin reports
/// nothing when the window is actually drawn, so time spent creating it counts
/// toward the delay and a slow window manager shortens what the user really
/// gets. This is a guard against keystrokes already in flight, not a promise of
/// a second of reading time.
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

/// What a gate says while any dialog sharing its slot is on screen.
const ALREADY_OPEN: &str = "a confirmation is already open; answer that one first";

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

/// Whether a native confirmation is on screen, shared by every gate that is
/// handed the same slot, so two kinds of question never stack on one window and
/// leave the user unsure which one a keypress answers.
#[derive(Clone, Default)]
pub struct DialogSlot(Arc<AtomicBool>);

impl DialogSlot {
    fn is_taken(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    fn take(&self) -> bool {
        self.0
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    fn release(&self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Serializes one kind of confirmation and remembers a refusal.
///
/// The cooldown is deliberately global rather than per host: a caller that
/// walks the host list would otherwise get a fresh dialog for every entry.
pub struct PromptGate {
    state: Mutex<GateState>,
    slot: DialogSlot,
    /// What the dialogs behind this gate ask about, for the refusals it gives.
    subject: &'static str,
    /// The tag a cancelled dialog's error carries; see `DECLINED`.
    declined: &'static str,
    /// How long each refusal in a row keeps the gate shut, the last entry
    /// repeating once they run out; see `DECLINE_COOLDOWN`.
    cooldowns: &'static [Duration],
}

#[derive(Default)]
struct GateState {
    declined_until: Option<Instant>,
    /// Refusals in a row, which pick the next cooldown. A confirmation clears
    /// it, and so does `DECLINE_MEMORY` without one.
    declines: usize,
    last_declined: Option<Instant>,
}

impl PromptGate {
    /// The gate for the host key dialogs.
    pub fn new() -> Self {
        Self::for_subject("host key", DECLINED, DECLINE_COOLDOWNS)
    }

    /// A gate of its own for another kind of question, so that refusing one
    /// kind does not silence the other, a view can tell which was refused, and
    /// each kind holds off for as long as suits how it is asked.
    ///
    /// `cooldowns` must not be empty. A first refusal waits `cooldowns[0]`, the
    /// next one `cooldowns[1]` and so on, so a short first wait lets a mistake
    /// be corrected while a caller that keeps being refused is held off for
    /// longer each time.
    pub fn for_subject(
        subject: &'static str,
        declined: &'static str,
        cooldowns: &'static [Duration],
    ) -> Self {
        assert!(!cooldowns.is_empty(), "a gate needs at least one cooldown");
        Self {
            state: Mutex::new(GateState::default()),
            slot: DialogSlot::default(),
            subject,
            declined,
            cooldowns,
        }
    }

    /// Show at most one dialog at a time across this gate and every other one
    /// handed the same slot. Each keeps its own cooldown.
    pub fn sharing(mut self, slot: DialogSlot) -> Self {
        self.slot = slot;
        self
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
        if self.slot.is_taken() {
            return Err(ALREADY_OPEN.to_string());
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
        if !self.slot.take() {
            return Err(ALREADY_OPEN.to_string());
        }
        Ok(())
    }

    fn close_at(&self, answer: Answer, now: Instant) {
        let mut state = self.lock();
        self.slot.release();
        match answer {
            // Only a refusal arms the cooldown, and each one in a row arms a
            // longer one, so a caller that keeps asking is held off for longer
            // every time while a single Cancel stays cheap to take back.
            Answer::Declined => {
                let forgotten = state
                    .last_declined
                    .is_some_and(|last| now.saturating_duration_since(last) > DECLINE_MEMORY);
                if forgotten {
                    state.declines = 0;
                }
                let step = state.declines.min(self.cooldowns.len() - 1);
                state.declined_until = Some(now + self.cooldowns[step]);
                state.declines += 1;
                state.last_declined = Some(now);
            }
            Answer::Confirmed => {
                state.declined_until = None;
                state.declines = 0;
                state.last_declined = None;
            }
            // A dialog that never appeared taught nobody to click through, so
            // silencing the next few seconds over it would hide the failure as
            // well as the question.
            Answer::Unavailable => state.declined_until = None,
        }
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

/// The text in the form the platform's dialog takes it.
///
/// On Linux and the BSDs the dialog plugin hands the body to GTK as the format
/// string of `gtk_message_dialog_format_secondary_text`, with no arguments, so a
/// `%s` or `%n` in text the page wrote (a host label, say) would be run as a
/// directive instead of printed. Doubling each `%` makes GTK print it. Windows
/// and macOS show the text as it is.
fn native_text(message: String) -> String {
    if cfg!(any(windows, target_os = "macos")) {
        message
    } else {
        escape_printf(&message)
    }
}

fn escape_printf(text: &str) -> String {
    text.replace('%', "%%")
}

async fn show(app: &AppHandle, title: &str, message: String, confirm_label: &str) -> Answer {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut builder = app
        .dialog()
        .message(native_text(message))
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
    use super::{
        escape_printf, read_answer, Answer, DialogSlot, PromptGate, COOLING_DOWN, DECLINE_COOLDOWN,
        DECLINE_MEMORY, READ_DELAY,
    };
    use std::cell::{Cell, RefCell};
    use std::time::{Duration, Instant};

    const SHORT: &[Duration] = &[Duration::from_secs(5)];
    const STEPS: &[Duration] = &[
        Duration::from_secs(5),
        Duration::from_secs(30),
        Duration::from_secs(300),
    ];

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
        let other = PromptGate::for_subject("host change", "[other-declined]", SHORT);
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
        let short = SHORT[0];
        let gate = PromptGate::for_subject("host change", "[other-declined]", SHORT);
        let now = Instant::now();
        gate.open_at(now).expect("first prompt");
        gate.close_at(Answer::Declined, now);

        gate.open_at(now + short - Duration::from_millis(1))
            .expect_err("the cooldown ended early");
        gate.open_at(now + short)
            .expect("the gate kept the host key cooldown instead of its own");
    }

    /// Decline at `at`, as the gate would see a dialog answered with Cancel, and
    /// say how long it then refuses to open.
    fn decline_at(gate: &PromptGate, at: Instant) -> Duration {
        gate.open_at(at).expect("the gate was still shut");
        gate.close_at(Answer::Declined, at);
        let mut wait = Duration::ZERO;
        while gate.open_at(at + wait).is_err() {
            wait += Duration::from_secs(1);
        }
        // The probe above claimed the gate on success; hand it back unanswered.
        gate.close_at(Answer::Unavailable, at + wait);
        wait
    }

    #[test]
    fn refusals_in_a_row_are_held_off_for_longer_each_time() {
        let gate = PromptGate::for_subject("host change", "[other-declined]", STEPS);
        let mut now = Instant::now();
        let mut waits = Vec::new();
        for _ in 0..5 {
            let wait = decline_at(&gate, now);
            now += wait;
            waits.push(wait.as_secs());
        }
        // The last step repeats instead of running off the end.
        assert_eq!(waits, [5, 30, 300, 300, 300]);
    }

    #[test]
    fn a_confirmation_starts_the_cooldowns_over() {
        let gate = PromptGate::for_subject("host change", "[other-declined]", STEPS);
        let mut now = Instant::now();
        now += decline_at(&gate, now);
        now += decline_at(&gate, now);

        gate.open_at(now).expect("open");
        gate.close_at(Answer::Confirmed, now);
        assert_eq!(decline_at(&gate, now), STEPS[0]);
    }

    #[test]
    fn a_long_quiet_spell_starts_the_cooldowns_over() {
        let gate = PromptGate::for_subject("host change", "[other-declined]", STEPS);
        let mut now = Instant::now();
        now += decline_at(&gate, now);
        now += decline_at(&gate, now);

        now += DECLINE_MEMORY + Duration::from_secs(1);
        assert_eq!(decline_at(&gate, now), STEPS[0]);
    }

    #[test]
    fn the_host_key_gate_keeps_one_cooldown_however_often_it_is_refused() {
        let gate = PromptGate::new();
        let mut now = Instant::now();
        for _ in 0..3 {
            let wait = decline_at(&gate, now);
            assert_eq!(wait, DECLINE_COOLDOWN);
            now += wait;
        }
    }

    #[test]
    fn gates_sharing_a_slot_never_show_two_dialogs_at_once() {
        let slot = DialogSlot::default();
        let host_keys = PromptGate::new().sharing(slot.clone());
        let host_change =
            PromptGate::for_subject("host change", "[other-declined]", SHORT).sharing(slot);
        let now = Instant::now();

        host_keys.open_at(now).expect("first prompt");
        let error = host_change
            .open_at(now)
            .expect_err("a dialog opened on top of another kind");
        assert!(error.contains("already open"), "{error}");

        // Answering the first frees the slot for either gate, and being turned
        // away left no cooldown on the one that was.
        host_keys.close_at(Answer::Confirmed, now);
        host_change.open_at(now).expect("the slot was not released");
        let error = host_keys
            .open_at(now)
            .expect_err("the first gate opened over the second");
        assert!(error.contains("already open"), "{error}");
    }

    #[test]
    fn gates_with_slots_of_their_own_do_not_wait_for_each_other() {
        let a = PromptGate::new();
        let b = PromptGate::for_subject("host change", "[other-declined]", SHORT);
        let now = Instant::now();
        a.open_at(now).expect("first prompt");
        b.open_at(now).expect("a separate gate was blocked");
    }

    #[test]
    fn a_percent_sign_in_dialog_text_is_doubled_for_gtk() {
        assert_eq!(escape_printf("100% %s%n"), "100%% %%s%%n");
        assert_eq!(escape_printf("no directives"), "no directives");
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

        // A future dropped mid-dialog must release the gate; otherwise `open`
        // would stay set for the life of the process and refuse every later
        // confirmation.
        gate.open()
            .expect("a dropped confirmation wedged the gate closed");
    }
}
