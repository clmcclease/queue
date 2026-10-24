//! A loopback fixture's `accept`, bounded (#100): a client that never connects fails the test
//! after 10 s instead of hanging it (the shape #91's fixtures already used).
#![allow(dead_code)] // each test crate uses one of the two

use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

/// How long a fixture waits for its one client.
pub const DEADLINE: Duration = Duration::from_secs(10);

/// The next client, as a blocking stream; panics if none connects within [`DEADLINE`].
pub fn accept(listener: &TcpListener) -> TcpStream {
    accept_until(listener, || false).expect("the client never connected")
}

/// The next client, or `None` once `stop()` says so or [`DEADLINE`] passes (for a fixture that
/// a test may never call).
pub fn accept_until(listener: &TcpListener, stop: impl Fn() -> bool) -> Option<TcpStream> {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + DEADLINE;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).unwrap();
                return Some(stream);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if stop() || Instant::now() >= deadline {
                    return None;
                }
                thread::sleep(Duration::from_millis(2));
            }
            Err(e) => panic!("fixture accept: {e}"),
        }
    }
}
