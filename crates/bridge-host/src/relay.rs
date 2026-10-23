//! Forward framed messages between the browser's stdin/stdout and the desktop socket.
//!
//! The browser keeps this process alive for the whole native-messaging port. If the
//! desktop app is not listening, each request gets `app_not_running` and the next
//! request tries again.

use std::{
    io::{self, Read, Write},
    process, thread,
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
                duplex(stream)?;
                tracing::info!(target: "airlock::bridge", "desktop app disconnected");
                process::exit(0);
            }
            Err(_) => {
                let message = {
                    let mut input = io::stdin().lock();
                    let Some(message) = read_frame(&mut input)? else {
                        return Ok(());
                    };
                    message
                };
                tracing::warn!(target: "airlock::bridge", "desktop app is not running");
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

fn duplex(stream: interprocess::local_socket::Stream) -> io::Result<()> {
    let (mut reader, mut writer) = stream.split();
    let stdout_thread = thread::spawn(move || -> io::Result<()> {
        let mut output = io::stdout().lock();
        loop {
            match read_frame(&mut reader) {
                Ok(Some(message)) => write_frame(&mut output, &message)?,
                Ok(None) => return Ok(()),
                Err(err) => return Err(err),
            }
        }
    });

    let mut input = io::stdin().lock();
    loop {
        match read_frame(&mut input) {
            Ok(Some(message)) => {
                if write_frame(&mut writer, &message).is_err() {
                    process::exit(0);
                }
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }
    drop(writer);
    let _ = stdout_thread.join();
    Ok(())
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
    serve_connections(listener, move || {
        let on_message = on_message.clone();
        Box::new(move |message: &[u8]| on_message(message))
    })
}

/// Answers the frames of one connection, in order. Dropped when it closes.
pub type Connection = Box<dyn FnMut(&[u8]) -> Vec<u8> + Send>;

/// Like [`serve_listener`], with a handler made for each connection by
/// `accept`, so the caller can tell when one opens and closes.
pub fn serve_connections(
    listener: endpoint::Listener,
    accept: impl Fn() -> Connection,
) -> io::Result<()> {
    loop {
        let mut connection = interprocess::local_socket::traits::Listener::accept(&listener)?;
        let mut on_message = accept();
        thread::spawn(move || {
            while let Ok(Some(message)) = read_frame(&mut connection) {
                let reply = on_message(&message);
                if write_frame(&mut connection, &reply).is_err() {
                    break;
                }
            }
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
}
