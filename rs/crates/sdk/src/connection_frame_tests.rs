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

/// Tour 2 P2-9 (Client-TS 274 `tcpIn`: an unknown packet type or a
/// decode error ends in `logout()`, never a further read). An unknown
/// opcode drops the connection: the error is `Io` (the reconnect class),
/// it names the opcode, and no byte of the desynced stream is read after
/// it (live 11:10: opcodes 41, 193, 213, 137 read on in 6 s).
#[test]
fn an_unknown_opcode_drops_the_connection() {
    let unknown = (0u8..=255).find(|opcode| crate::protocol::server_op(*opcode).is_none()).expect("an unused opcode");
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback bind");
    let port = listener.local_addr().expect("local addr").port();
    let incoming_seed = [7, 11, 13, 17];
    let server = std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().expect("accept");
        let mut cipher = Isaac::new(incoming_seed);
        let first = 171u8.wrapping_add(cipher.next_int() as u8);
        let bad = unknown.wrapping_add(cipher.next_int() as u8);
        let third = 171u8.wrapping_add(cipher.next_int() as u8);
        sock.write_all(&[first, bad, third]).expect("frames");
        std::thread::sleep(Duration::from_millis(500));
    });
    let client = TcpStream::connect(("127.0.0.1", port)).expect("loopback dial");
    client.set_read_timeout(Some(Duration::from_millis(100))).expect("client read timeout");
    let mut conn = Connection::new(Duration::from_millis(300));
    conn.attach_for_test(client, [1, 2, 3, 4], incoming_seed);
    assert_eq!(conn.recv().expect("the first frame decodes").name, "IF_CLOSE");
    let desync = conn.recv().expect_err("unknown opcode");
    assert!(matches!(desync, crate::error::Error::Io(_)), "reconnect class, got {desync}");
    assert!(desync.to_string().contains(&format!("unknown server opcode {unknown}")), "names the opcode: {desync}");
    assert!(!conn.is_open(), "the connection is dropped");
    assert!(conn.recv().is_err(), "nothing is read after the desync");
    server.join().expect("server thread");
}

/// One synthesized server stream: real opcodes and sizes of the zone,
/// interface and chat families, fixed, byte-length and short-length
/// frames, empty bodies and the 255/256 length edges. Returns the wire
/// bytes (ISAAC-shifted) and the frames expected back `(name, body)`.
fn sample_stream(seed: [i32; 4]) -> (Vec<u8>, Vec<(&'static str, Vec<u8>)>) {
    let mut cipher = Isaac::new(seed);
    let mut wire = Vec::new();
    let mut expected = Vec::new();
    let filler = |len: usize, salt: usize| -> Vec<u8> { (0..len).map(|i| ((i * 31 + salt * 7) % 251) as u8).collect() };
    let mut push = |opcode: u8, body: Vec<u8>| {
        let (name, size) = crate::protocol::server_op(opcode).expect("known opcode");
        wire.push(opcode.wrapping_add(cipher.next_int() as u8));
        match size {
            -1 => wire.push(body.len() as u8),
            -2 => wire.extend_from_slice(&(body.len() as u16).to_be_bytes()),
            fixed => assert_eq!(fixed as usize, body.len(), "{name} fixed size"),
        }
        wire.extend_from_slice(&body);
        expected.push((name, body));
    };
    for round in 0..3usize {
        push(153, filler(2, round)); // UPDATE_ZONE_FULL_FOLLOWS
        push(138, filler(4, round)); // LOC_ADD_CHANGE
        push(138, filler(4, round + 1));
        push(32, filler(2, round)); // UPDATE_ZONE_PARTIAL_FOLLOWS
        push(195, filler(2 + 4 * (round + 1) + 5, round)); // UPDATE_ZONE_PARTIAL_ENCLOSED
        push(81, filler(5, round)); // OBJ_ADD
        push(183, filler(4, round)); // IF_SETCOLOUR
        push(235, filler(8 + 4 + 1 + 17, round)); // MESSAGE_PRIVATE: id, id, rights, huffman text
        push(161, Vec::new()); // MESSAGE_GAME, empty
        push(161, filler(255, round)); // MESSAGE_GAME at the byte limit
        push(44, filler(0, round)); // IF_SETTEXT, empty
        push(44, filler(256, round)); // IF_SETTEXT, length 0x0100
        push(44, filler(300, round));
        push(167, filler(1200, round)); // PLAYER_INFO
        push(171, Vec::new()); // IF_CLOSE
        push(106, filler(65, round)); // UPDATE_INV_FULL
    }
    (wire, expected)
}

/// Serve `chunks` of the stream over loopback (no Nagle, a pause between
/// chunks so each lands as its own read when it can) and decode it all.
fn decode_with_chunks(seed: [i32; 4], wire: Vec<u8>, chunks: Vec<usize>, total: usize) -> Vec<(String, Vec<u8>)> {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback bind");
    let port = listener.local_addr().expect("local addr").port();
    let server = std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().expect("accept");
        sock.set_nodelay(true).expect("nodelay");
        let mut at = 0;
        let mut sizes = chunks.into_iter().cycle();
        while at < wire.len() {
            let end = (at + sizes.next().expect("cycle").max(1)).min(wire.len());
            sock.write_all(&wire[at..end]).expect("chunk");
            at = end;
            std::thread::sleep(Duration::from_micros(300));
        }
        std::thread::sleep(Duration::from_millis(300));
    });
    let client = TcpStream::connect(("127.0.0.1", port)).expect("loopback dial");
    client.set_nodelay(true).expect("nodelay");
    client.set_read_timeout(Some(Duration::from_millis(100))).expect("client read timeout");
    let mut conn = Connection::new(Duration::from_millis(300));
    conn.attach_for_test(client, [1, 2, 3, 4], seed);
    let mut got = Vec::new();
    while got.len() < total {
        match conn.recv() {
            Ok(frame) => got.push((frame.name, frame.body)),
            Err(crate::error::Error::Timeout(_)) => {}
            Err(other) => panic!("after {} frames: {other}", got.len()),
        }
    }
    server.join().expect("server thread");
    got
}

/// Decoding must not depend on how TCP cut the stream: 1-byte reads,
/// every fixed chunk size, pseudo-random cuts, and one big write all give
/// the same frames.
#[test]
fn decoding_is_independent_of_segmentation() {
    let seed = [21, 34, 55, 89];
    let (wire, expected) = sample_stream(seed);
    let expected: Vec<(String, Vec<u8>)> = expected.into_iter().map(|(name, body)| (name.to_string(), body)).collect();
    let mut patterns: Vec<Vec<usize>> = vec![vec![1], vec![wire.len()], vec![2, 1, 3], vec![1, 255, 1]];
    for size in [2usize, 3, 4, 5, 7, 11, 64, 255, 256, 257, 1460] {
        patterns.push(vec![size]);
    }
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    for _ in 0..8 {
        let cuts: Vec<usize> = (0..64)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                1 + (state % 40) as usize
            })
            .collect();
        patterns.push(cuts);
    }
    for pattern in patterns {
        let got = decode_with_chunks(seed, wire.clone(), pattern.clone(), expected.len());
        assert_eq!(got.len(), expected.len(), "frame count for cuts {pattern:?}");
        for (index, (have, want)) in got.iter().zip(&expected).enumerate() {
            assert_eq!(have, want, "frame {index} for cuts {:?}", &pattern[..pattern.len().min(8)]);
        }
    }
}

/// The pump's own shape: one blocking `recv`, then `available()` and
/// `recv` while more is waiting (lot 6.3: `available` leaves the socket
/// non-blocking, reads come in blocks). Same frames for any cutting.
fn decode_pumped(seed: [i32; 4], wire: Vec<u8>, chunks: Vec<usize>, total: usize) -> Vec<(String, Vec<u8>)> {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback bind");
    let port = listener.local_addr().expect("local addr").port();
    let server = std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().expect("accept");
        sock.set_nodelay(true).expect("nodelay");
        let mut at = 0;
        let mut sizes = chunks.into_iter().cycle();
        while at < wire.len() {
            let end = (at + sizes.next().expect("cycle").max(1)).min(wire.len());
            sock.write_all(&wire[at..end]).expect("chunk");
            at = end;
            std::thread::sleep(Duration::from_micros(300));
        }
        std::thread::sleep(Duration::from_millis(300));
    });
    let client = TcpStream::connect(("127.0.0.1", port)).expect("loopback dial");
    client.set_nodelay(true).expect("nodelay");
    client.set_read_timeout(Some(Duration::from_millis(100))).expect("client read timeout");
    let mut conn = Connection::new(Duration::from_millis(300));
    conn.attach_for_test(client, [1, 2, 3, 4], seed);
    let mut got = Vec::new();
    while got.len() < total {
        match conn.recv() {
            Ok(frame) => got.push((frame.name, frame.body)),
            Err(crate::error::Error::Timeout(_)) => continue,
            Err(other) => panic!("after {} frames: {other}", got.len()),
        }
        while got.len() < total && conn.available().expect("available") {
            match conn.recv() {
                Ok(frame) => got.push((frame.name, frame.body)),
                Err(crate::error::Error::Timeout(_)) => break,
                Err(other) => panic!("after {} frames: {other}", got.len()),
            }
        }
    }
    server.join().expect("server thread");
    got
}

#[test]
fn pumped_decoding_is_independent_of_segmentation() {
    let seed = [21, 34, 55, 89];
    let (wire, expected) = sample_stream(seed);
    let expected: Vec<(String, Vec<u8>)> = expected.into_iter().map(|(name, body)| (name.to_string(), body)).collect();
    for pattern in [vec![1], vec![wire.len()], vec![2, 1, 3], vec![7], vec![255], vec![1460], vec![1, 255, 1]] {
        let got = decode_pumped(seed, wire.clone(), pattern.clone(), expected.len());
        assert_eq!(got, expected, "frames for cuts {:?}", &pattern[..pattern.len().min(8)]);
    }
}

/// A send after `available()` (which leaves the socket non-blocking)
/// goes through whole, and reads still work afterwards.
#[test]
fn a_send_after_available_still_goes_through() {
    use std::io::Read;
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback bind");
    let port = listener.local_addr().expect("local addr").port();
    let incoming_seed = [7, 11, 13, 17];
    let server = std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().expect("accept");
        sock.set_read_timeout(Some(Duration::from_secs(5))).expect("server read timeout");
        let mut got = [0u8; 1];
        sock.read_exact(&mut got).expect("the keepalive arrives");
        let mut cipher = Isaac::new(incoming_seed);
        sock.write_all(&[171u8.wrapping_add(cipher.next_int() as u8)]).expect("frame");
        std::thread::sleep(Duration::from_millis(300));
    });
    let client = TcpStream::connect(("127.0.0.1", port)).expect("loopback dial");
    client.set_read_timeout(Some(Duration::from_millis(100))).expect("client read timeout");
    let mut conn = Connection::new(Duration::from_millis(500));
    conn.attach_for_test(client, [1, 2, 3, 4], incoming_seed);
    assert!(!conn.available().expect("idle socket"), "nothing waits");
    conn.send("NO_TIMEOUT", &[]).expect("send after a non-blocking probe");
    let mut frame = conn.recv();
    for _ in 0..20 {
        if matches!(frame, Err(crate::error::Error::Timeout(_))) {
            frame = conn.recv();
        }
    }
    assert_eq!(frame.expect("the frame decodes").name, "IF_CLOSE");
    server.join().expect("server thread");
}

/// Micro-benchmark (`--ignored --nocapture`): `available()` on a quiet
/// socket, and a 48-frame burst consumed the pump's way.
#[test]
#[ignore]
fn bench_available_and_burst() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback bind");
    let port = listener.local_addr().expect("local addr").port();
    let seed = [21, 34, 55, 89];
    let (wire, expected) = sample_stream(seed);
    let rounds = 300usize;
    let server_wire = wire.clone();
    let server = std::thread::spawn(move || {
        let mut held = Vec::new();
        for _ in 0..rounds {
            let (mut sock, _) = listener.accept().expect("accept");
            sock.set_nodelay(true).expect("nodelay");
            sock.write_all(&server_wire).expect("burst");
            held.push(sock);
        }
        std::thread::sleep(Duration::from_millis(500));
    });
    let mut idle_ns = 0f64;
    let mut burst_ns = 0f64;
    for round in 0..rounds {
        let client = TcpStream::connect(("127.0.0.1", port)).expect("loopback dial");
        client.set_read_timeout(Some(Duration::from_millis(500))).expect("client read timeout");
        let mut conn = Connection::new(Duration::from_millis(500));
        conn.attach_for_test(client, [1, 2, 3, 4], seed);
        let mut frames = 0;
        frames += conn.recv().map(|_| 1).expect("first frame");
        let started = std::time::Instant::now();
        loop {
            while conn.available().expect("available") {
                conn.recv().expect("frame");
                frames += 1;
            }
            if frames >= expected.len() {
                break;
            }
            conn.recv().expect("frame");
            frames += 1;
        }
        burst_ns += started.elapsed().as_nanos() as f64;
        if round == 0 {
            let started = std::time::Instant::now();
            let calls = 100_000;
            for _ in 0..calls {
                assert!(!conn.available().expect("available"));
            }
            idle_ns = started.elapsed().as_nanos() as f64 / f64::from(calls);
        }
    }
    server.join().expect("server thread");
    eprintln!("BENCH idle available(): {idle_ns:.0} ns/call; {}-frame burst: {:.0} ns/burst (incl. the probes)", expected.len(), burst_ns / rounds as f64);
}
