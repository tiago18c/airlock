//! Ledgers and Trezors on the phone's USB-C port, through `UsbBridge` in the
//! app's `MainActivity.kt`: Android lists them and asks the user to let
//! Airlock use one, and `hw_transport::android` talks to it from the file
//! descriptor Android opened.

use hw_protocol::ledger;
use hw_protocol::link::{Connected, DeviceKind, LinkError, PacketLink};
use hw_protocol::trezor;

/// How long the user may take on Android's "Allow Airlock to access…".
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(120);
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
const POLL: std::time::Duration = std::time::Duration::from_millis(150);

/// A device as the bridge lists it, `<name>|<vendor>|<product>`, as the
/// core sees it: its id names its kind and Android's name for it.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn connected(line: &str) -> Option<Connected> {
    let mut parts = line.trim().split('|');
    let name = parts.next().filter(|name| !name.is_empty())?;
    let vendor: u16 = parts.next()?.parse().ok()?;
    let product: u16 = parts.next()?.parse().ok()?;
    let (kind, tag, model) = if vendor == ledger::VENDOR_ID {
        (DeviceKind::Ledger, "ledger", ledger::model(product))
    } else if vendor == trezor::VENDOR_ID && product == trezor::PRODUCT_ID {
        (DeviceKind::Trezor, "trezor", None)
    } else {
        return None;
    };
    Some(Connected {
        id: format!("{tag}:{name}"),
        kind,
        model: model.map(str::to_string),
    })
}

/// The kind and Android's name of the device `id` names.
fn named(id: &str) -> Option<(DeviceKind, &str)> {
    let (tag, name) = id.split_once(':')?;
    let kind = match tag {
        "ledger" => DeviceKind::Ledger,
        "trezor" => DeviceKind::Trezor,
        _ => return None,
    };
    Some((kind, name))
}

/// What opening answered: a file descriptor, or why not.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn opened(result: &str) -> Result<i32, LinkError> {
    if let Some(fd) = result.strip_prefix("ok:") {
        return fd
            .trim()
            .parse()
            .map_err(|_| LinkError::Io(format!("Android gave back {fd:?}")));
    }
    let rest = result.strip_prefix("err:").unwrap_or(result);
    let (code, text) = rest.split_once(':').unwrap_or((rest, ""));
    Err(match code {
        "denied" => LinkError::Denied("Airlock was not allowed to use it".into()),
        "gone" => LinkError::Disconnected,
        _ => LinkError::Io(text.to_string()),
    })
}

pub(crate) fn list() -> Result<Vec<Connected>, LinkError> {
    #[cfg(target_os = "android")]
    {
        let text = crate::android::usb_list().map_err(LinkError::Io)?;
        Ok(text.lines().filter_map(connected).collect())
    }
    #[cfg(not(target_os = "android"))]
    {
        Ok(Vec::new())
    }
}

pub(crate) async fn open(id: &str) -> Result<Box<dyn PacketLink>, LinkError> {
    let (kind, name) = named(id).ok_or(LinkError::Disconnected)?;
    #[cfg(target_os = "android")]
    {
        use std::os::fd::{FromRawFd, OwnedFd};
        static NEXT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1);
        let request = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        crate::android::usb_open(request, name).map_err(LinkError::Io)?;
        let end = std::time::Instant::now() + PATIENCE;
        let result = loop {
            if let Some(result) = crate::android::usb_take(request).map_err(LinkError::Io)? {
                break result;
            }
            if std::time::Instant::now() > end {
                return Err(LinkError::Denied(
                    "Android's question about the device was not answered".into(),
                ));
            }
            tokio::time::sleep(POLL).await;
        };
        let fd = opened(&result)?;
        // SAFETY: the bridge detached this descriptor for Rust and keeps no
        // copy of it; it is open, and closed only when this OwnedFd drops.
        #[allow(unsafe_code)] // A raw descriptor from Java, owned from here on.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        hw_transport::android::open_fd(kind, fd).await
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (kind, name);
        Err(LinkError::Denied("not on Android".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devices_read_as_the_bridge_lists_them() {
        let ledger = connected("/dev/bus/usb/001/002|11415|20480").unwrap();
        assert_eq!(ledger.kind, DeviceKind::Ledger);
        assert_eq!(ledger.id, "ledger:/dev/bus/usb/001/002");
        assert_eq!(ledger.model.as_deref(), Some("Nano S Plus"));
        let trezor = connected("/dev/bus/usb/001/003|4617|21441").unwrap();
        assert_eq!(trezor.kind, DeviceKind::Trezor);
        assert_eq!(
            named(&trezor.id),
            Some((DeviceKind::Trezor, "/dev/bus/usb/001/003"))
        );
        // A Trezor in its bootloader, something else, and junk.
        assert_eq!(connected("/dev/bus/usb/001/004|4617|21440"), None);
        assert_eq!(connected("/dev/bus/usb/001/005|1234|1"), None);
        assert_eq!(connected(""), None);
        assert_eq!(named("hid:whatever"), None);
    }

    #[test]
    fn what_opening_answered_reads_as_a_descriptor_or_a_reason() {
        assert_eq!(opened("ok:42"), Ok(42));
        assert!(matches!(opened("err:denied:"), Err(LinkError::Denied(_))));
        assert_eq!(opened("err:gone:"), Err(LinkError::Disconnected));
        assert_eq!(opened("err:failed:no"), Err(LinkError::Io("no".into())));
    }
}
