//! Private messages and chat filter modes.
//!
//! Every wait is a pump loop to an observable state plus a tick bound:
//! never wall-clock sleep.

use crate::buffer::Writer;
use crate::client::Bot;
use crate::error::Result;
use crate::words::{pack, to_base37};

/// MESSAGE_PRIVATE body: base-37 name then packed text, exactly the
/// `MessagePrivateDecoder` order (framing adds the length byte, like
/// the client placeholder `p1(0)` + `psize1`).
fn tell_bytes(name: &str, text: &str) -> Vec<u8> {
    let mut body = Writer::new();
    body.u64(to_base37(name));
    body.raw(&pack(text));
    body.bytes()
}

/// CHAT_SETMODE body: public, private, trade, one byte each, exactly
/// the `ChatSetModeDecoder` order.
fn chat_setmode_bytes(public: u8, private: u8, trade: u8) -> Vec<u8> {
    let mut body = Writer::new();
    body.u8(public);
    body.u8(private);
    body.u8(trade);
    body.bytes()
}

impl Bot {
    /// Private message to a player by name: base-37 name + packed text
    /// (`Client.ts` social input path).
    pub fn tell(&mut self, name: &str, text: &str) -> Result<()> {
        self.act(format!("tell {name}"));
        self.send_named("MESSAGE_PRIVATE", &tell_bytes(name, text))
    }

    /// Chat filter modes: public 0-3, private 0-2, trade 0-2, like the
    /// client chat-mode buttons cycle them.
    pub fn chat_setmode(&mut self, public: u8, private: u8, trade: u8) -> Result<()> {
        self.act(format!("chat_setmode {public}/{private}/{trade}"));
        self.send_named("CHAT_SETMODE", &chat_setmode_bytes(public, private, trade))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;

    /// Tell bytes lock the server decoder order (`MessagePrivateDecoder`:
    /// g8 name, then `length - 8` packed bytes): name first, packed text
    /// after, so a swapped field ships a silent bad packet.
    #[test]
    fn tell_payload_matches_the_server_decoder() {
        let body = tell_bytes("alice", "hi");
        let (name, text) = body.split_at(8);
        assert_eq!(
            name,
            &to_base37("alice").to_be_bytes(),
            "base-37 name first"
        );
        assert_eq!(text, &pack("hi"), "packed text after");
    }

    /// Chat modes lock the decoder order (public, private, trade).
    #[test]
    fn chat_setmode_payload_matches_the_server_decoder() {
        assert_eq!(chat_setmode_bytes(3, 1, 2), vec![3, 1, 2]);
    }

    /// The wrappers resolve their op names (a bad name fails `Protocol`,
    /// never reaches the socket): dead socket reads `Login`, proving the
    /// send path is wired, framing included.
    #[test]
    fn social_wrappers_reach_the_send_path() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        for outcome in [bot.tell("alice", "hi"), bot.chat_setmode(3, 1, 2)] {
            assert!(
                matches!(outcome, Err(Error::Login(_))),
                "dead socket fails at login, never at the op name: {outcome:?}"
            );
        }
    }
}
