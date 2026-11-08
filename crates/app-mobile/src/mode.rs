//! Whether the app runs online or offline. Each mode has its own data
//! folder, so its own vault: an offline vault never sits in a folder the
//! online app opens. The choice is kept in the private files folder, beside
//! the two folders, and read at launch before anything else opens.

use std::path::PathBuf;

/// How the app runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The network, balances, Stake, History and sending.
    Online,
    /// No node is asked anything. Signing and QR transfer only, with the
    /// offline vault.
    Offline,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Mode::Online => "online",
            Mode::Offline => "offline",
        }
    }

    fn parse(text: &str) -> Self {
        match text.trim() {
            "offline" => Mode::Offline,
            _ => Mode::Online,
        }
    }
}

const FILE: &str = "mode";

fn path() -> Option<PathBuf> {
    crate::files_dir().map(|dir| dir.join(FILE))
}

/// The mode the app was left in: online when none was ever chosen.
pub fn load() -> Mode {
    path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| Mode::parse(&text))
        .unwrap_or(Mode::Online)
}

/// Keep `mode` for the next launch.
pub fn save(mode: Mode) -> Result<(), String> {
    let path = path().ok_or("no private files folder")?;
    std::fs::write(path, mode.as_str()).map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anything_but_offline_reads_as_online() {
        assert_eq!(Mode::parse("offline\n"), Mode::Offline);
        assert_eq!(Mode::parse("online"), Mode::Online);
        assert_eq!(Mode::parse(""), Mode::Online);
        assert_eq!(Mode::parse(Mode::Offline.as_str()), Mode::Offline);
    }
}
