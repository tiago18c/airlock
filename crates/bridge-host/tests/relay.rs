//! The host as a browser starts it, with `bridge-host listen` standing in for
//! the desktop app. Once connected it must end as soon as either side does: a
//! browser cannot tell a stuck host from a live one, and every site's requests
//! would wait out their whole timeout.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use bridge_protocol::frame::{read_frame, write_frame, MAX_MESSAGE};
use serde_json::{json, Value};

const PATIENCE: Duration = Duration::from_secs(20);

/// A child that does not outlive its test.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Running {
    /// How it ended, once it has. Fails the test if it does not end.
    fn ended(&mut self) -> ExitStatus {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "the host is still running");
            thread::sleep(Duration::from_millis(20));
        }
    }
}

/// The socket and logs of one test, kept apart from an app that is running.
struct Place {
    dir: PathBuf,
    user: String,
}

impl Place {
    fn new(test: &str) -> Self {
        let user = format!("airlock_test_{}_{test}", std::process::id());
        let dir = std::env::temp_dir().join(&user);
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir, user }
    }

    fn host(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_bridge-host"));
        command
            .env("USERNAME", &self.user)
            .env("XDG_RUNTIME_DIR", &self.dir)
            .env("HOME", &self.dir)
            .env("AIRLOCK_DATA_DIR", &self.dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }
}

/// The host relaying to the stand-in, as the browser sees it.
struct Relay {
    app: Running,
    host: Running,
    stdin: Option<ChildStdin>,
    replies: mpsc::Receiver<Vec<u8>>,
    place: Place,
}

fn connected(test: &str) -> Relay {
    let place = Place::new(test);
    let app = Running(place.host().arg("listen").spawn().unwrap());
    let mut host = place
        .host()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = host.stdin.take().unwrap();
    let mut stdout = host.stdout.take().unwrap();
    let (send, replies) = mpsc::channel();
    thread::spawn(move || {
        while let Ok(Some(frame)) = read_frame(&mut stdout) {
            if send.send(frame).is_err() {
                return;
            }
        }
    });

    // A host that starts first says the app is not running, and tries again.
    let status = serde_json::to_vec(&json!({ "v": 1, "id": "s", "method": "status" })).unwrap();
    let deadline = Instant::now() + PATIENCE;
    loop {
        write_frame(&mut stdin, &status).unwrap();
        let reply: Value =
            serde_json::from_slice(&replies.recv_timeout(PATIENCE).unwrap()).unwrap();
        if reply["result"]["role"] == "dev-stub" {
            break;
        }
        assert!(Instant::now() < deadline, "never reached the app: {reply}");
        thread::sleep(Duration::from_millis(20));
    }
    Relay {
        app,
        host: Running(host),
        stdin: Some(stdin),
        replies,
        place,
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        let _ = self.app.0.kill();
        let _ = self.app.0.wait();
        let _ = self.host.0.kill();
        let _ = self.host.0.wait();
        let _ = std::fs::remove_dir_all(&self.place.dir);
    }
}

#[test]
fn a_frame_it_cannot_read_ends_it() {
    let mut relay = connected("bad_frame");
    let too_long = u32::try_from(MAX_MESSAGE + 1).unwrap();
    let stdin = relay.stdin.as_mut().unwrap();
    stdin.write_all(&too_long.to_le_bytes()).unwrap();
    stdin.flush().unwrap();
    assert_eq!(relay.host.ended().code(), Some(1));
    assert!(matches!(
        relay.replies.recv_timeout(PATIENCE),
        Err(mpsc::RecvTimeoutError::Disconnected)
    ));
}

#[test]
fn the_browser_going_away_ends_it() {
    let mut relay = connected("browser_gone");
    drop(relay.stdin.take());
    assert!(relay.host.ended().success());
}

#[test]
fn the_app_going_away_ends_it_with_nothing_sent() {
    let mut relay = connected("app_gone");
    relay.app.0.kill().unwrap();
    relay.app.0.wait().unwrap();
    assert!(relay.host.ended().success());
}
