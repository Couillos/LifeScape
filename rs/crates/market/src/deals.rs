#![forbid(unsafe_code)]
//! Deal lifecycle: a match becomes a physical meeting on the square,
//! then a trade, then a settlement or a typed failure. Every transition
//! is checked; the terminal states never move again.

use crate::error::Error;

/// Where a deal stands between the match and the handshake.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DealStatus {
    Matched,
    Convened,
    BothArrived,
    Trading,
    Settled,
    Failed,
}

impl DealStatus {
    /// Wire code used by the socket and the store.
    pub fn as_str(self) -> &'static str {
        match self {
            DealStatus::Matched => "matched",
            DealStatus::Convened => "convened",
            DealStatus::BothArrived => "both_arrived",
            DealStatus::Trading => "trading",
            DealStatus::Settled => "settled",
            DealStatus::Failed => "failed",
        }
    }

    /// Parses a wire code back; unknown codes are caller bugs.
    pub fn parse(text: &str) -> Result<Self, Error> {
        match text {
            "matched" => Ok(DealStatus::Matched),
            "convened" => Ok(DealStatus::Convened),
            "both_arrived" => Ok(DealStatus::BothArrived),
            "trading" => Ok(DealStatus::Trading),
            "settled" => Ok(DealStatus::Settled),
            "failed" => Ok(DealStatus::Failed),
            other => Err(Error::Invalid(format!("bad deal status {other}"))),
        }
    }
}

/// Why a deal died. Stored beside the failed row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FailReason {
    NoShow,
    /// A party could not produce its side of the trade: the seller has
    /// no goods, the buyer no coins (checked against the real bank and
    /// pack at the handover). Its orders are phantom stock and are
    /// cancelled, never relisted.
    Short,
    Mismatch,
    Death,
    Timeout,
    NavDown,
}

impl FailReason {
    /// Wire code used by the socket and the store.
    pub fn as_str(self) -> &'static str {
        match self {
            FailReason::NoShow => "no_show",
            FailReason::Short => "short",
            FailReason::Mismatch => "mismatch",
            FailReason::Death => "death",
            FailReason::Timeout => "timeout",
            FailReason::NavDown => "nav_down",
        }
    }

    /// Parses a wire code back; unknown codes are caller bugs.
    pub fn parse(text: &str) -> Result<Self, Error> {
        match text {
            "no_show" => Ok(FailReason::NoShow),
            "short" => Ok(FailReason::Short),
            "mismatch" => Ok(FailReason::Mismatch),
            "death" => Ok(FailReason::Death),
            "timeout" => Ok(FailReason::Timeout),
            "nav_down" => Ok(FailReason::NavDown),
            other => Err(Error::Invalid(format!("bad fail reason {other}"))),
        }
    }
}

/// One physical delivery. The engine owns the status; callers only ask.
#[derive(Clone, Debug, PartialEq)]
pub struct Deal {
    pub id: u64,
    pub buyer: String,
    pub seller: String,
    pub item: i64,
    pub quantity: i64,
    pub price: i64,
    pub meeting_x: i64,
    pub meeting_y: i64,
    pub deadline_tick: i64,
    pub status: DealStatus,
    pub fail: Option<FailReason>,
}

impl Deal {
    /// A fresh match, before any convocation.
    pub fn fresh(
        id: u64,
        buyer: &str,
        seller: &str,
        item: i64,
        quantity: i64,
        price: i64,
        meeting_x: i64,
        meeting_y: i64,
        deadline_tick: i64,
    ) -> Result<Self, Error> {
        if buyer.is_empty() || seller.is_empty() || buyer == seller {
            return Err(Error::Invalid("bad deal parties".to_string()));
        }
        if quantity <= 0 || price < 1 {
            return Err(Error::Invalid(format!("bad deal qty/price {quantity}/{price}")));
        }
        Ok(Deal {
            id,
            buyer: buyer.to_string(),
            seller: seller.to_string(),
            item,
            quantity,
            price,
            meeting_x,
            meeting_y,
            deadline_tick,
            status: DealStatus::Matched,
            fail: None,
        })
    }

    /// Moves the deal one step; any step to `Failed` needs its reason.
    /// Terminal states never move again.
    pub fn transition(&mut self, next: DealStatus, fail: Option<FailReason>) -> Result<(), Error> {
        let allowed = match (self.status, next) {
            (DealStatus::Matched, DealStatus::Convened) => true,
            (DealStatus::Convened, DealStatus::BothArrived) => true,
            (DealStatus::BothArrived, DealStatus::Trading) => true,
            (DealStatus::Trading, DealStatus::Settled) => true,
            (from, DealStatus::Failed)
                if matches!(
                    from,
                    DealStatus::Matched | DealStatus::Convened | DealStatus::BothArrived | DealStatus::Trading
                ) =>
            {
                true
            }
            _ => false,
        };
        if !allowed {
            return Err(Error::Invalid(format!(
                "deal {} cannot move {} -> {}",
                self.id,
                self.status.as_str(),
                next.as_str()
            )));
        }
        if next == DealStatus::Failed && fail.is_none() {
            return Err(Error::Invalid(format!("deal {} failed without a reason", self.id)));
        }
        if next != DealStatus::Failed && fail.is_some() {
            return Err(Error::Invalid(format!(
                "deal {} carries a failure into {}",
                self.id,
                next.as_str()
            )));
        }
        self.status = next;
        self.fail = fail;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deal() -> Deal {
        Deal::fresh(1, "buyer", "seller", 314, 10, 5, 3212, 3428, 9999).expect("fresh")
    }

    #[test]
    fn happy_path_walks_every_step() {
        let mut current = deal();
        for next in [
            DealStatus::Convened,
            DealStatus::BothArrived,
            DealStatus::Trading,
            DealStatus::Settled,
        ] {
            current.transition(next, None).expect("moves");
        }
        assert_eq!(current.status, DealStatus::Settled);
        assert!(current.transition(DealStatus::Failed, Some(FailReason::Timeout)).is_err());
    }

    /// `short` round-trips on the wire: the daemon parses it back to
    /// cancel the defaulter's orders instead of relisting phantom stock.
    #[test]
    fn short_is_its_own_wire_reason() {
        assert_eq!(FailReason::Short.as_str(), "short");
        assert_eq!(FailReason::parse("short").expect("parses"), FailReason::Short);
        assert_ne!(FailReason::Short, FailReason::NoShow, "a missing lot is not a missing bot");
    }

    #[test]
    fn every_open_state_can_fail_with_its_reason() {
        for reason in [
            FailReason::NoShow,
            FailReason::Short,
            FailReason::Mismatch,
            FailReason::Death,
            FailReason::Timeout,
            FailReason::NavDown,
        ] {
            let mut current = deal();
            if reason == FailReason::Mismatch {
                current.transition(DealStatus::Convened, None).expect("convenes");
                current.transition(DealStatus::BothArrived, None).expect("arrives");
            }
            current.transition(DealStatus::Failed, Some(reason)).expect("fails");
            assert_eq!(current.status, DealStatus::Failed);
            assert_eq!(current.fail, Some(reason));
        }
    }

    #[test]
    fn skips_and_reasonless_failures_are_refused() {
        let mut current = deal();
        assert!(current.transition(DealStatus::Trading, None).is_err());
        assert!(current.transition(DealStatus::Failed, None).is_err());
        assert!(current.transition(DealStatus::Convened, Some(FailReason::Death)).is_err());
    }

    #[test]
    fn status_codes_roundtrip() {
        for status in [
            DealStatus::Matched,
            DealStatus::Convened,
            DealStatus::BothArrived,
            DealStatus::Trading,
            DealStatus::Settled,
            DealStatus::Failed,
        ] {
            assert_eq!(DealStatus::parse(status.as_str()), Ok(status));
        }
        assert!(DealStatus::parse("gone").is_err());
    }
}
