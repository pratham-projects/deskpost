//! Ticket status state machine. Statuses match the product (`frontend/lib/types.ts`).

use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TicketStatus {
    Pending,
    InProgress,
    OnHold,
    Resolved,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionError {
    /// The ticket is already in the requested status.
    Unchanged,
    /// Closed tickets can only be reopened to `pending`.
    ClosedIsFinal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownStatus;

impl TicketStatus {
    pub const ALL: [TicketStatus; 5] = [
        Self::Pending,
        Self::InProgress,
        Self::OnHold,
        Self::Resolved,
        Self::Closed,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::OnHold => "on_hold",
            Self::Resolved => "resolved",
            Self::Closed => "closed",
        }
    }

    /// Agents may move a ticket between any open states and close it. A closed ticket stays
    /// closed except for a reopen to `pending` (customer replies reopen it).
    pub fn transition(self, to: TicketStatus) -> Result<TicketStatus, TransitionError> {
        if self == to {
            return Err(TransitionError::Unchanged);
        }
        if self == Self::Closed && to != Self::Pending {
            return Err(TransitionError::ClosedIsFinal);
        }
        Ok(to)
    }

    pub fn can_transition(self, to: TicketStatus) -> bool {
        self.transition(to).is_ok()
    }

    /// Status a ticket takes when the customer replies by email.
    pub fn on_customer_reply(self) -> TicketStatus {
        match self {
            Self::Resolved | Self::Closed | Self::OnHold => Self::Pending,
            other => other,
        }
    }

    /// Only resolved tickets are eligible for the auto-close job.
    pub fn is_auto_closable(self) -> bool {
        self == Self::Resolved
    }
}

impl fmt::Display for TicketStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TicketStatus {
    type Err = UnknownStatus;
    fn from_str(s: &str) -> Result<Self, UnknownStatus> {
        Self::ALL
            .into_iter()
            .find(|st| st.as_str() == s)
            .ok_or(UnknownStatus)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn any_status() -> impl Strategy<Value = TicketStatus> {
        proptest::sample::select(TicketStatus::ALL.to_vec())
    }

    #[test]
    fn string_round_trip_and_unknown() {
        for s in TicketStatus::ALL {
            assert_eq!(s.as_str().parse::<TicketStatus>(), Ok(s));
        }
        assert_eq!("open".parse::<TicketStatus>(), Err(UnknownStatus));
        assert_eq!("Pending".parse::<TicketStatus>(), Err(UnknownStatus));
        assert_eq!("".parse::<TicketStatus>(), Err(UnknownStatus));
    }

    #[test]
    fn same_status_is_rejected() {
        for s in TicketStatus::ALL {
            assert_eq!(s.transition(s), Err(TransitionError::Unchanged));
        }
    }

    #[test]
    fn closed_only_reopens_to_pending() {
        use TicketStatus::*;
        assert_eq!(Closed.transition(Pending), Ok(Pending));
        for to in [InProgress, OnHold, Resolved] {
            assert_eq!(Closed.transition(to), Err(TransitionError::ClosedIsFinal));
        }
    }

    #[test]
    fn open_states_move_freely() {
        use TicketStatus::*;
        for from in [Pending, InProgress, OnHold, Resolved] {
            for to in TicketStatus::ALL {
                if from != to {
                    assert_eq!(from.transition(to), Ok(to), "{from} -> {to}");
                }
            }
        }
    }

    #[test]
    fn customer_reply_reopens() {
        use TicketStatus::*;
        assert_eq!(Resolved.on_customer_reply(), Pending);
        assert_eq!(Closed.on_customer_reply(), Pending);
        assert_eq!(OnHold.on_customer_reply(), Pending);
        assert_eq!(InProgress.on_customer_reply(), InProgress);
        assert_eq!(Pending.on_customer_reply(), Pending);
    }

    #[test]
    fn only_resolved_auto_closes() {
        for s in TicketStatus::ALL {
            assert_eq!(s.is_auto_closable(), s == TicketStatus::Resolved);
        }
    }

    proptest! {
        #[test]
        fn customer_reply_result_is_always_a_legal_move_or_noop(s in any_status()) {
            let to = s.on_customer_reply();
            prop_assert!(to == s || s.can_transition(to));
        }

        #[test]
        fn display_parse_round_trip(s in any_status()) {
            prop_assert_eq!(s.to_string().parse::<TicketStatus>(), Ok(s));
        }

        #[test]
        fn parse_never_panics(s in ".*") {
            let _ = s.parse::<TicketStatus>();
        }
    }
}
