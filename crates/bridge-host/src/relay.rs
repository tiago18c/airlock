//! Forward framed messages between the browser's stdin/stdout and the desktop socket.
//!
//! The browser keeps this process alive for the whole native-messaging port. If the
//! desktop app is not listening, each request gets `app_not_running` and the next
//! request tries again. Once connected it lives only as long as both sides: the
//! browser sees it exit, tells whoever was waiting, and starts a new one for the
//! next request.

use std::{
    io::{self, Read, Write},
    process,
    sync::{Arc, Mutex, PoisonError},
    thread,
};

use interprocess::local_socket::{traits::Stream as _, Name};

use crate::endpoint;
use bridge_protocol::frame::{read_frame, write_frame};
use bridge_protocol::message::error_reply;

pub fn relay_stdio() -> io::Result<()> {
    crate::log::init();
    binary_stdio();
    let endpoint = endpoint::desktop_endpoint()?;
    loop {
        match endpoint::connect(&endpoint.name) {
            Ok(stream) => {
                tracing::info!(target: "airlock::bridge", "connected to the desktop app");
                duplex(stream);
            }
            // To a site, a socket someone else holds is an app that is not
            // running. The log says which.
            Err(err) => {
                let message = {
                    let mut input = io::stdin().lock();
                    let Some(message) = read_frame(&mut input)? else {
                        return Ok(());
                    };
                    message
                };
                tracing::warn!(
                    target: "airlock::bridge",
                    error = %err,
                    "desktop app is not running"
                );
                let mut output = io::stdout().lock();
                write_frame(
                    &mut output,
                    &error_reply(
                        &message,
                        "app_not_running",
                        "The Airlock desktop app is not running.",
                    ),
                )?;
            }
        }
    }
}

/// Relay frames both ways until either side stops, then exit. Nothing here can
/// wait for the other direction: a port with a dead host looks alive to the
/// browser, and every request sent to it would wait out its whole timeout.
fn duplex(stream: interprocess::local_socket::Stream) -> ! {
    let (mut reader, mut writer) = stream.split();
    thread::spawn(move || {
        let mut output = io::stdout().lock();
        while let Ok(Some(message)) = read_frame(&mut reader) {
            if write_frame(&mut output, &message).is_err() {
                process::exit(0);
            }
        }
        tracing::info!(target: "airlock::bridge", "desktop app disconnected");
        process::exit(0);
    });

    let mut input = io::stdin().lock();
    loop {
        match read_frame(&mut input) {
            Ok(Some(message)) => {
                if write_frame(&mut writer, &message).is_err() {
                    process::exit(0);
                }
            }
            Ok(None) => process::exit(0),
            // The length that failed says where the next frame starts, so
            // the stream cannot be read past it.
            Err(err) => {
                tracing::warn!(target: "airlock::bridge", error = %err, "unreadable frame");
                process::exit(1);
            }
        }
    }
}

pub fn reject_until_eof(input: &mut impl Read, output: &mut impl Write) -> io::Result<()> {
    while let Some(message) = read_frame(input)? {
        write_frame(
            output,
            &error_reply(
                &message,
                "app_not_running",
                "The Airlock desktop app is not running.",
            ),
        )?;
    }
    Ok(())
}

/// Turns one request frame into its reply frame.
pub type Handler = std::sync::Arc<dyn Fn(&[u8]) -> Vec<u8> + Send + Sync>;

/// Accept connections and answer each frame with `on_message`.
pub fn serve_app(on_message: Handler) -> io::Result<()> {
    let endpoint = endpoint::desktop_endpoint()?;
    endpoint::prepare_directory(&endpoint)?;
    let listener = endpoint::bind_endpoint(&endpoint)?;
    serve_listener(listener, on_message)
}

/// Serve on an already-bound listener (used so the online shell can claim the
/// socket before the UI starts, for single-instance).
pub fn serve_listener(listener: endpoint::Listener, on_message: Handler) -> io::Result<()> {
    struct Plain(Handler);
    impl Connection for Plain {
        fn answer(&self, message: &[u8]) -> Answer {
            Answer::Now((self.0)(message))
        }
    }
    serve_connections(listener, move |_| Arc::new(Plain(on_message.clone())))
}

/// What becomes of one frame.
pub enum Answer {
    /// The reply, ready now.
    Now(Vec<u8>),
    /// A reply that waits for someone. It gets a thread of its own, so it holds
    /// up nothing else: match replies by their id.
    Later(Box<dyn FnOnce() -> Vec<u8> + Send>),
}

/// One open connection to the socket.
pub trait Connection: Send + Sync {
    /// Take in one frame. Frames come here one at a time, in the order they
    /// were sent, so whatever a frame refers to has been taken in. Anything
    /// that has to wait belongs in [`Answer::Later`]. There is a thread for
    /// each of those, so what makes one must be limited.
    fn answer(&self, message: &[u8]) -> Answer;

    /// The other end went away, after every frame it sent was taken in. A
    /// reply still waiting goes nowhere.
    fn closed(&self) {}
}

/// Writes whole frames to a connection, from any thread.
#[derive(Clone)]
pub struct FrameSender(Arc<Mutex<interprocess::local_socket::SendHalf>>);

impl FrameSender {
    pub fn send(&self, frame: &[u8]) -> io::Result<()> {
        let mut writer = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        write_frame(&mut *writer, frame)
    }
}

/// Like [`serve_listener`], with a handler made for each connection by
/// `accept`, which also gets the way to send it frames of the app's own.
pub fn serve_connections(
    listener: endpoint::Listener,
    accept: impl Fn(FrameSender) -> Arc<dyn Connection>,
) -> io::Result<()> {
    loop {
        let stream = interprocess::local_socket::traits::Listener::accept(&listener)?;
        let (mut reader, writer) = stream.split();
        let sender = FrameSender(Arc::new(Mutex::new(writer)));
        let connection = accept(sender.clone());
        thread::spawn(move || {
            while let Ok(Some(message)) = read_frame(&mut reader) {
                match connection.answer(&message) {
                    Answer::Now(reply) => {
                        let _ = sender.send(&reply);
                    }
                    Answer::Later(wait) => {
                        let sender = sender.clone();
                        thread::spawn(move || {
                            let _ = sender.send(&wait());
                        });
                    }
                }
            }
            connection.closed();
        });
    }
}

pub fn serve_dev(name: &Name<'_>) -> io::Result<()> {
    crate::log::init();
    let listener = endpoint::bind(name)?;
    tracing::info!(
        target: "airlock::bridge",
        "listening (development stand-in; answers status only)"
    );
    loop {
        let mut connection = interprocess::local_socket::traits::Listener::accept(&listener)?;
        thread::spawn(move || {
            while let Ok(Some(message)) = read_frame(&mut connection) {
                let reply = crate::message::dev_stub_reply(&message);
                if write_frame(&mut connection, &reply).is_err() {
                    break;
                }
            }
        });
    }
}

#[allow(unsafe_code)] // The C runtime's _setmode, which std does not wrap.
fn binary_stdio() {
    #[cfg(windows)]
    unsafe {
        unsafe extern "C" {
            fn _setmode(fd: i32, mode: i32) -> i32;
        }
        const O_BINARY: i32 = 0x8000;
        _setmode(0, O_BINARY);
        _setmode(1, O_BINARY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endpoint::{self, prepare_directory};
    use crate::frame::{read_frame, write_frame};
    use serde_json::Value;
    use std::thread;

    #[test]
    fn missing_desktop_replies_with_the_request_id() {
        let frame = {
            let mut buf = Vec::new();
            write_frame(&mut buf, br#"{"v":1,"id":"abc","method":"connect"}"#).unwrap();
            buf
        };
        let mut output = Vec::new();
        reject_until_eof(&mut &frame[..], &mut output).unwrap();
        let reply = read_frame(&mut &output[..]).unwrap().unwrap();
        let value: Value = serde_json::from_slice(&reply).unwrap();
        assert_eq!(value["id"], "abc");
        assert_eq!(value["error"]["code"], "app_not_running");
    }

    #[test]
    fn socket_roundtrip() {
        let endpoint = endpoint::test_endpoint("roundtrip").unwrap();
        prepare_directory(&endpoint).unwrap();
        let listener = endpoint::bind(&endpoint.name).unwrap();
        let server = thread::spawn(move || {
            let mut stream =
                interprocess::local_socket::traits::Listener::accept(&listener).unwrap();
            let request = read_frame(&mut stream).unwrap().unwrap();
            let value: Value = serde_json::from_slice(&request).unwrap();
            assert_eq!(value["method"], "status");
            write_frame(&mut stream, &crate::message::dev_stub_reply(&request)).unwrap();
        });

        let mut client = endpoint::connect(&endpoint.name).unwrap();
        write_frame(&mut client, br#"{"v":1,"id":"s","method":"status"}"#).unwrap();
        let reply = read_frame(&mut client).unwrap().unwrap();
        let value: Value = serde_json::from_slice(&reply).unwrap();
        assert_eq!(value["result"]["role"], "dev-stub");
        server.join().unwrap();
    }

    /// Answers "wait" later and everything else now, and notes what it took in.
    struct Recording {
        release: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
        seen: Arc<Mutex<Vec<String>>>,
    }

    impl Connection for Recording {
        fn answer(&self, message: &[u8]) -> Answer {
            let text = String::from_utf8(message.to_vec()).unwrap();
            self.seen.lock().unwrap().push(text.clone());
            if text != "wait" {
                return Answer::Now(text.into_bytes());
            }
            let release = self.release.lock().unwrap().take().unwrap();
            Answer::Later(Box::new(move || {
                release.recv().unwrap();
                b"waited".to_vec()
            }))
        }

        fn closed(&self) {
            self.seen.lock().unwrap().push("closed".into());
        }
    }

    #[test]
    fn a_reply_that_waits_holds_up_nothing_and_frames_stay_in_order() {
        let endpoint = endpoint::test_endpoint("waiting").unwrap();
        prepare_directory(&endpoint).unwrap();
        let listener = endpoint::bind(&endpoint.name).unwrap();
        let (release, released) = std::sync::mpsc::channel();
        let released = Mutex::new(Some(released));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recording = seen.clone();
        thread::spawn(move || {
            serve_connections(listener, move |_| {
                Arc::new(Recording {
                    release: Mutex::new(released.lock().unwrap().take()),
                    seen: recording.clone(),
                })
            })
        });

        let mut client = endpoint::connect(&endpoint.name).unwrap();
        for frame in [&b"first"[..], b"wait", b"after"] {
            write_frame(&mut client, frame).unwrap();
        }
        assert_eq!(read_frame(&mut client).unwrap().unwrap(), b"first");
        assert_eq!(read_frame(&mut client).unwrap().unwrap(), b"after");
        release.send(()).unwrap();
        assert_eq!(read_frame(&mut client).unwrap().unwrap(), b"waited");

        drop(client);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while seen.lock().unwrap().len() < 4 {
            assert!(std::time::Instant::now() < deadline, "never closed");
            thread::yield_now();
        }
        assert_eq!(
            *seen.lock().unwrap(),
            ["first", "wait", "after", "closed"].map(str::to_owned)
        );
    }
}
