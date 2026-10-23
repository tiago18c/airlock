//! Native messaging host. The browser starts it with no subcommand (or with the
//! extension origin as the only argument) and talks JSON frames on stdin/stdout.

use std::{io, process};

use bridge_host::{install, relay};

fn main() {
    let status = match std::env::args().nth(1).as_deref() {
        None => relay::relay_stdio().map(|()| 0),
        Some("install") => install::install().map(|()| 0),
        Some("uninstall") => install::uninstall().map(|()| 0),
        Some("listen") => listen().map(|()| 0),
        Some("help") | Some("--help") | Some("-h") => {
            print_usage();
            Ok(0)
        }
        Some(arg) if arg.contains("://") || arg.contains('@') => relay::relay_stdio().map(|()| 0),
        Some(_) => {
            print_usage();
            Ok(2)
        }
    };
    match status {
        Ok(code) => process::exit(code),
        Err(err) => {
            eprintln!("bridge-host: {err}");
            process::exit(1);
        }
    }
}

fn listen() -> io::Result<()> {
    let endpoint = bridge_host::endpoint::desktop_endpoint()?;
    bridge_host::endpoint::prepare_directory(&endpoint)?;
    if let Some(path) = &endpoint.path {
        eprintln!("bridge-host: socket {}", path.display());
    }
    relay::serve_dev(&endpoint.name)
}

fn print_usage() {
    eprintln!(
        "\
Airlock bridge-host

  bridge-host            Relay the browser to the desktop app. Chrome launches this.
  bridge-host install    Register the host for the current user.
  bridge-host uninstall  Remove that registration.
  bridge-host listen     Development stand-in. Answers status, rejects everything else.

Quit listen before the desktop app binds the same socket."
    );
}
