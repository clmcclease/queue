//! Independent #100 part 1 QA: real sockets and a watchdog for the shared fixture helper.
#[path = "support/accept.rs"]
mod accept;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

#[test]
fn accept_without_a_client_panics_after_ten_seconds_instead_of_hanging() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (done, result) = mpsc::channel();
    let started = Instant::now();
    let worker = thread::spawn(move || {
        let outcome = std::panic::catch_unwind(|| accept::accept(&listener));
        done.send(outcome.is_err()).unwrap();
    });
    // A regression to blocking accept must fail this test without blocking the test process.
    assert!(result.recv_timeout(Duration::from_secs(13)).unwrap());
    worker.join().unwrap();
    assert!(started.elapsed() >= Duration::from_secs(10));
}

#[test]
fn accept_returns_a_blocking_stream_even_when_the_listener_was_nonblocking() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let (accepted, connected) = mpsc::channel();
    let writer = thread::spawn(move || {
        let mut stream = TcpStream::connect(address).unwrap();
        connected.recv_timeout(Duration::from_secs(3)).unwrap();
        // No bytes are ready when accept returns: a nonblocking stream would yield WouldBlock.
        thread::sleep(Duration::from_millis(100));
        stream.write_all(b"ok").unwrap();
    });
    let mut stream = accept::accept(&listener);
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    accepted.send(()).unwrap();
    let mut bytes = [0; 2];
    stream.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"ok");
    writer.join().unwrap();
}
