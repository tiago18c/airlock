//! Lists the hardware wallets this machine sees, the Solana app's version
//! on each Ledger and the firmware of each Trezor:
//! `cargo run -p hw-transport --example list`.

use hw_protocol::link::{exchange_apdu, exchange_message, DeviceKind};
use hw_protocol::{ledger, trezor};

fn main() {
    let Some(hub) = hw_transport::hub() else {
        println!("No device hub in this build.");
        return;
    };
    futures_lite::future::block_on(async {
        let devices = match hub.list().await {
            Ok(devices) => devices,
            Err(error) => return println!("Listing failed: {error}"),
        };
        if devices.is_empty() {
            println!("No hardware wallet is connected.");
        }
        for device in devices {
            println!("{:?} {:?}", device.kind, device.model);
            let mut link = match hub.open(&device.id).await {
                Ok(link) => link,
                Err(error) => {
                    println!("  cannot open: {error}");
                    continue;
                }
            };
            if device.kind == DeviceKind::Trezor {
                match exchange_message(link.as_mut(), &trezor::initialize()).await {
                    Ok(reply) => match trezor::parse_reply(&reply) {
                        Ok(trezor::Reply::Features(features)) => println!(
                            "  {:?} ({:?}), firmware {:?}",
                            features.model, features.internal_model, features.version
                        ),
                        other => println!("  {other:?}"),
                    },
                    Err(error) => println!("  no answer: {error}"),
                }
                continue;
            }
            match exchange_apdu(link.as_mut(), &ledger::get_app_and_version().to_bytes()).await {
                Ok(reply) => match ledger::check_reply(&reply).map(ledger::parse_app_and_version) {
                    Ok(Ok(app)) => println!("  open app: {} {}", app.name, app.version),
                    other => println!("  {other:?}"),
                },
                Err(error) => println!("  no answer: {error}"),
            }
        }
    });
}
