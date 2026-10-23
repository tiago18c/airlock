//! How long a recent blockhash has left, as pure arithmetic.
//!
//! The Sign screen and the request modal share these numbers. Polling the
//! node and drawing the countdown live elsewhere.

/// Blocks a fresh blockhash is accepted for, when the node does not say.
pub const FRESH_BLOCKHASH_BLOCKS: u64 = 150;

/// About how long one block takes, for the countdown display. Every
/// network runs 200 ms slots, so 150 blocks is about 30 seconds.
pub const SECONDS_PER_BLOCK: f64 = 0.2;

/// Blocks still left until `last_valid_height`, given the current height.
pub fn blocks_left(last_valid_height: u64, height: u64) -> u64 {
    last_valid_height.saturating_sub(height)
}

/// Whole seconds left for `blocks`, at [`SECONDS_PER_BLOCK`].
pub fn seconds_left(blocks: u64) -> u64 {
    (blocks as f64 * SECONDS_PER_BLOCK).round() as u64
}

/// Blocks left after `elapsed_secs` of wall time since the height was read,
/// assuming about [`SECONDS_PER_BLOCK`] per block.
pub fn blocks_left_after(blocks_at_read: u64, elapsed_secs: f64) -> u64 {
    let spent = (elapsed_secs / SECONDS_PER_BLOCK).floor() as u64;
    blocks_at_read.saturating_sub(spent)
}

/// What a blockhash countdown shows at one moment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockReading {
    pub seconds_left: u64,
    pub expired: bool,
}

/// The countdown `elapsed_secs` after the node said the chain was at
/// `height`. Expiry is only ever what the node reported: it said the hash is
/// no longer valid, or, when `last_valid_height` is `exact`, a height past it.
/// An estimate running out on wall time alone shows 0:00 but is not expired.
pub fn read_clock(
    last_valid_height: u64,
    height: u64,
    elapsed_secs: f64,
    exact: bool,
    valid_on_chain: Option<bool>,
) -> ClockReading {
    let expired = valid_on_chain == Some(false) || (exact && height > last_valid_height);
    let blocks = blocks_left_after(blocks_left(last_valid_height, height), elapsed_secs);
    ClockReading {
        seconds_left: if expired { 0 } else { seconds_left(blocks) },
        expired,
    }
}

/// `m:ss` for a remaining second count.
pub fn format_countdown(seconds: u64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_and_seconds_count_down() {
        assert_eq!(blocks_left(150, 0), 150);
        assert_eq!(blocks_left(150, 150), 0);
        assert_eq!(blocks_left(150, 200), 0);
        assert_eq!(seconds_left(150), 30);
        assert_eq!(seconds_left(1), 0);
        assert_eq!(seconds_left(5), 1);
        assert_eq!(format_countdown(42), "0:42");
        assert_eq!(format_countdown(90), "1:30");
    }

    #[test]
    fn wall_time_eats_blocks() {
        assert_eq!(blocks_left_after(150, 0.0), 150);
        assert_eq!(blocks_left_after(150, 0.2), 149);
        assert_eq!(blocks_left_after(150, 30.0), 0);
    }

    #[test]
    fn expiry_follows_what_the_node_said() {
        let live = read_clock(100, 50, 0.0, true, Some(true));
        assert_eq!(
            live,
            ClockReading {
                seconds_left: 10,
                expired: false
            }
        );
        assert!(read_clock(100, 101, 0.0, true, Some(true)).expired);
        assert!(read_clock(100, 50, 0.0, true, Some(false)).expired);
        assert!(read_clock(100, 50, 0.0, false, Some(false)).expired);
        // An estimate past its height is not expired until the node says so.
        assert!(!read_clock(100, 101, 0.0, false, Some(true)).expired);
    }

    #[test]
    fn wall_time_runs_the_display_down_without_expiring_it() {
        let reading = read_clock(150, 0, 45.0, true, None);
        assert_eq!(
            reading,
            ClockReading {
                seconds_left: 0,
                expired: false
            }
        );
        assert_eq!(read_clock(150, 0, 10.0, false, None).seconds_left, 20);
    }
}
