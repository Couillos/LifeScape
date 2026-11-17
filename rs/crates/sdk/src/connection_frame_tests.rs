//! Frame decoding across a stalled read (plan P11, 2026-11-17).

use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use super::Connection;
use crate::isaac::Isaac;

/// A frame whose body stalls past the read budget: the timeout is
/// typed, the next `recv` finishes the SAME frame, and the following one
/// decodes (before: the head was dropped, the next opcode was read out of
/// the body, "unknown server opcode N" with N different every time).
#[test]
fn a_stalled_frame_resumes_without_desync() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback bind");
    let port = listener.local_addr().expect("local addr").port();
    let incoming_seed = [7, 11, 13, 17];
    let server = std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().expect("accept");
        let mut cipher = Isaac::new(incoming_seed);
        // OBJ_ADD (81, fixed 5): opcode and 2 body bytes, then a stall.
        let first = 81u8.wrapping_add(cipher.next_int() as u8);
        sock.write_all(&[first, 1, 2]).expect("head");
        std::thread::sleep(Duration::from_millis(900));
        // The rest of the body, then IF_CLOSE (171, fixed 0).
        let second = 171u8.wrapping_add(cipher.next_int() as u8);
        sock.write_all(&[3, 4, 5, second]).expect("tail");
        std::thread::sleep(Duration::from_millis(500));
    });
    let client = TcpStream::connect(("127.0.0.1", port)).expect("loopback dial");
    client.set_read_timeout(Some(Duration::from_millis(100))).expect("client read timeout");
    let mut conn = Connection::new(Duration::from_millis(300));
    conn.attach_for_test(client, [1, 2, 3, 4], incoming_seed);
    let stalled = conn.recv().expect_err("the body stalls past the budget");
    assert!(matches!(stalled, crate::error::Error::Timeout(_)), "typed timeout, got {stalled}");
    // Callers retry a timeout (pump loops): each retry resumes the frame.
    let mut resumed = conn.recv();
    for _ in 0..10 {
        match &resumed {
            Err(crate::error::Error::Timeout(_)) => resumed = conn.recv(),
            _ => break,
        }
    }
    let resumed = resumed.expect("the same frame resumes");
    assert_eq!((resumed.name.as_str(), resumed.body.clone()), ("OBJ_ADD", vec![1, 2, 3, 4, 5]));
    let next = conn.recv().expect("the stream stays in step");
    assert_eq!(next.name, "IF_CLOSE");
    server.join().expect("server thread");
}
