//! What happens between Ctrl+Enter and the prompt reaching the agent.
//!
//! Two phases, and a submission that has not gone yet is one that can still be
//! taken back for free: the pairs never left the stack, `sent_at` was never
//! stamped, and nothing was written to the pty. That is the property this
//! module exists for, and it is unchanged.
//!
//! **What changed is who waits.** D16 asked for an undo window and got one: a
//! five-second delay on every send, so a regretted one could be retracted
//! without ever having existed. That was the wrong trade — the regret is rare
//! and Escape already interrupts a turn, while the five seconds were paid on
//! every send and were most of what the loop felt like. The frontend now arms a
//! submission the moment it queues it, so [`Phase::Undo`] is a state a
//! submission passes through rather than sits in, and the only thing that
//! genuinely waits is a cold-starting pane (`noteBootQuiet` in `ui/app.js`).
//!
//! D11 asked for a mid-turn send to queue rather than interleave, and for two
//! queued sends to be two turns. The second half is still [`Flow::take_ready`]:
//! it hands back the *oldest armed* entry and one at a time, so a second
//! Ctrl+Enter is a second submission and never a merged one. The first half now
//! belongs to Claude Code, whose composer queues a line typed during a turn —
//! dreamd guessing at idleness from outside was approximating a thing the TUI
//! already does correctly.
//!
//! **There is no clock in here, on purpose.** What decides when a submission may
//! go is a timer the frontend already owns (a `setInterval` beside the
//! `pty-data` listener). Putting a deadline in this module would mean either
//! injecting a clock into every test or testing a state machine by sleeping. So
//! the frontend supplies the *events*
//! ([`arm`](Flow::arm), [`cancel`](Flow::cancel), [`take_ready`](Flow::take_ready))
//! and this decides what they are allowed to mean.
//!
//! The invariant worth naming: an id is never in two live submissions at once
//! (see [`Flow::queue`]), and a submission is taken at most once — which is the
//! whole of "no path that submits twice".

use crate::annotations::Id;
use serde::Serialize;

/// A handle on one queued submission. Opaque to the frontend, which only ever
/// hands it back.
pub type Token = u64;

/// Where a queued submission is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// Queued but not yet eligible. The frontend arms immediately, so nothing
    /// stays here for more than one turn of the event loop — the variant
    /// survives because "queued" and "eligible" being the same instant is the
    /// *frontend's* current policy, not a fact about this state machine.
    Undo,
    /// Eligible, and still cancellable right up until it is taken — a
    /// cold-starting pane may sit on one for a second or two, and a reader who
    /// changes their mind in that time has not sent anything either.
    Armed,
}

/// One submission waiting to go.
#[derive(Debug, Clone, Serialize)]
pub struct Pending {
    pub token: Token,
    /// The ids this submission will send, in stack order.
    pub ids: Vec<Id>,
    pub phase: Phase,
}

/// Every submission currently between the keypress and the pty.
///
/// A terminal transition — cancelled, or taken — *removes* the entry rather
/// than marking it. That is what makes double-submit unrepresentable rather
/// than merely guarded against: after [`take_ready`](Flow::take_ready) there is
/// nothing left to take, and [`cancel`](Flow::cancel) on the same token answers
/// false because the token names nothing any more.
#[derive(Default)]
pub struct Flow {
    queue: Vec<Pending>,
    next: Token,
}

impl Flow {
    /// Queue `ids` for submission.
    ///
    /// Ids already spoken for by a live submission are dropped, so no pair can
    /// be in two of these at once — without that, a second Ctrl+Enter before
    /// the first has gone would ask the same questions twice, because the stack
    /// does not shrink until something is actually sent. That window is short
    /// now, but a cold-starting pane still holds it open for a second or two,
    /// which is exactly long enough for a reader to press the key again.
    ///
    /// `None` when nothing usable is left, which is also D10's empty stack: the
    /// caller opens the pane and does nothing else.
    pub fn queue(&mut self, ids: Vec<Id>) -> Option<Pending> {
        let fresh: Vec<Id> = ids
            .into_iter()
            .filter(|id| !self.queue.iter().any(|p| p.ids.contains(id)))
            .collect();
        if fresh.is_empty() {
            return None;
        }
        self.next += 1;
        let pending = Pending {
            token: self.next,
            ids: fresh,
            phase: Phase::Undo,
        };
        self.queue.push(pending.clone());
        Some(pending)
    }

    /// `token` is now eligible to go. False if it names nothing, or is already
    /// armed.
    pub fn arm(&mut self, token: Token) -> bool {
        match self.find(token) {
            Some(p) if p.phase == Phase::Undo => {
                p.phase = Phase::Armed;
                true
            }
            _ => false,
        }
    }

    /// Take it back. False if it names nothing — including a submission that
    /// has already gone, which is the point: after the text reaches the child
    /// there is no undo, only a new turn.
    pub fn cancel(&mut self, token: Token) -> bool {
        let before = self.queue.len();
        self.queue.retain(|p| p.token != token);
        self.queue.len() != before
    }

    /// The oldest armed submission, removed. `None` while every entry is still
    /// unarmed, and `None` once taken.
    ///
    /// One at a time on purpose (D11): two queued sends are two turns, so the
    /// caller comes back for the second after the agent has dealt with the
    /// first.
    pub fn take_ready(&mut self) -> Option<Pending> {
        let at = self.queue.iter().position(|p| p.phase == Phase::Armed)?;
        Some(self.queue.remove(at))
    }

    /// Everything still waiting, oldest first.
    pub fn pending(&self) -> &[Pending] {
        &self.queue
    }

    fn find(&mut self, token: Token) -> Option<&mut Pending> {
        self.queue.iter_mut().find(|p| p.token == token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(names: &[&str]) -> Vec<Id> {
        names.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn a_queued_send_starts_in_the_undo_window() {
        // The whole of D16's shape: nothing has gone anywhere yet, so there is
        // nothing to retract.
        let mut flow = Flow::default();
        let p = flow.queue(ids(&["a", "b"])).expect("queued");
        assert_eq!(p.phase, Phase::Undo);
        assert_eq!(p.ids, ids(&["a", "b"]));
        assert!(
            flow.take_ready().is_none(),
            "an unarmed send must not be takeable"
        );
    }

    #[test]
    fn an_empty_selection_queues_nothing() {
        // D10, in the layer that can be tested: Ctrl+Enter on an empty stack
        // has nothing to say, and the caller reads `None` as "just open the
        // pane".
        let mut flow = Flow::default();
        assert!(flow.queue(Vec::new()).is_none());
        assert!(flow.pending().is_empty());
    }

    #[test]
    fn arming_then_taking_submits_exactly_once() {
        // The no-double-submit path, stated as directly as it can be. The
        // second call has nothing to return because taking *removes*.
        let mut flow = Flow::default();
        let token = flow.queue(ids(&["a"])).expect("queued").token;
        assert!(flow.arm(token));
        let taken = flow.take_ready().expect("armed");
        assert_eq!(taken.token, token);
        assert_eq!(taken.ids, ids(&["a"]));
        assert!(flow.take_ready().is_none(), "submitted twice");
        assert!(flow.pending().is_empty());
    }

    #[test]
    fn cancelling_inside_the_window_leaves_nothing_to_send() {
        let mut flow = Flow::default();
        let token = flow.queue(ids(&["a"])).expect("queued").token;
        assert!(flow.cancel(token));
        assert!(flow.pending().is_empty());
        assert!(!flow.arm(token), "a cancelled send re-armed");
    }

    #[test]
    fn cancelling_after_the_window_still_works() {
        // Armed is not sent. The pane can sit on an armed submission for a
        // whole turn waiting for the agent to go quiet, and a reader who
        // changes their mind in that time has still sent nothing.
        let mut flow = Flow::default();
        let token = flow.queue(ids(&["a"])).expect("queued").token;
        assert!(flow.arm(token));
        assert!(flow.cancel(token));
        assert!(flow.take_ready().is_none());
    }

    #[test]
    fn cancelling_a_submitted_send_is_refused() {
        // After the text reaches the child there is no undo, only a new turn —
        // and the frontend must be told that rather than being allowed to paint
        // an undo that would do nothing.
        let mut flow = Flow::default();
        let token = flow.queue(ids(&["a"])).expect("queued").token;
        flow.arm(token);
        flow.take_ready().expect("armed");
        assert!(!flow.cancel(token));
        assert!(!flow.arm(token));
    }

    #[test]
    fn two_queued_sends_are_two_turns() {
        // D11. Both are armed and both are ready, and `take_ready` still hands
        // back one — the first — so the caller submits them as two turns rather
        // than merging them into one.
        let mut flow = Flow::default();
        let first = flow.queue(ids(&["a"])).expect("queued").token;
        let second = flow.queue(ids(&["b"])).expect("queued").token;
        assert!(flow.arm(first) && flow.arm(second));

        let taken = flow.take_ready().expect("first");
        assert_eq!(taken.token, first);
        assert_eq!(taken.ids, ids(&["a"]));
        assert_eq!(flow.pending().len(), 1, "the second is still waiting");

        let taken = flow.take_ready().expect("second");
        assert_eq!(taken.token, second);
        assert_eq!(taken.ids, ids(&["b"]));
    }

    #[test]
    fn an_armed_send_does_not_jump_the_queue() {
        // Order is the reader's asking order, and an older unarmed submission
        // must not be overtaken by a newer one that happens to arm first —
        // that would put the questions to the agent in an order nobody chose.
        let mut flow = Flow::default();
        let first = flow.queue(ids(&["a"])).expect("queued").token;
        let second = flow.queue(ids(&["b"])).expect("queued").token;
        assert!(flow.arm(second));
        assert_eq!(flow.take_ready().expect("armed").token, second);

        assert!(flow.arm(first));
        assert_eq!(flow.take_ready().expect("armed").token, first);
    }

    #[test]
    fn an_id_cannot_be_in_two_live_sends() {
        // The stack does not shrink until something is actually sent, so a
        // second Ctrl+Enter inside the undo window sees the same pairs. Without
        // this filter it would ask every one of them a second time.
        let mut flow = Flow::default();
        flow.queue(ids(&["a", "b"])).expect("queued");
        assert!(
            flow.queue(ids(&["a", "b"])).is_none(),
            "the same pairs queued twice"
        );
        // A pair added since the first press is genuinely new and does queue —
        // and brings only itself.
        let second = flow.queue(ids(&["a", "b", "c"])).expect("queued");
        assert_eq!(second.ids, ids(&["c"]));
    }

    #[test]
    fn an_id_frees_up_once_its_send_has_gone() {
        // Re-annotating a mark that is already out with the agent mints a
        // second question about the same passage (D14), so the same id must be
        // queueable again — just never twice at once.
        let mut flow = Flow::default();
        let token = flow.queue(ids(&["a"])).expect("queued").token;
        flow.arm(token);
        flow.take_ready().expect("armed");
        assert!(flow.queue(ids(&["a"])).is_some());
    }

    #[test]
    fn tokens_are_not_reused() {
        // The frontend holds a token across a timer; a reused one would cancel
        // or arm somebody else's submission.
        let mut flow = Flow::default();
        let first = flow.queue(ids(&["a"])).expect("queued").token;
        flow.cancel(first);
        let second = flow.queue(ids(&["a"])).expect("queued").token;
        assert_ne!(first, second);
    }

    #[test]
    fn an_unknown_token_is_refused_rather_than_guessed_at() {
        let mut flow = Flow::default();
        let token = flow.queue(ids(&["a"])).expect("queued").token;
        assert!(!flow.arm(token + 99));
        assert!(!flow.cancel(token + 99));
        assert_eq!(flow.pending().len(), 1, "the real one is untouched");
    }
}
