//! Putting scanned frames back together.

use super::*;

#[derive(Debug, Clone)]
pub struct Assembler {
    transfer_id: Option<[u8; 8]>,
    count: u16,
    total_len: u32,
    crc32: u32,
    chunks: Vec<Option<Vec<u8>>>,
}

impl Default for Assembler {
    fn default() -> Self {
        Self::new()
    }
}

impl Assembler {
    pub fn new() -> Self {
        Self {
            transfer_id: None,
            count: 0,
            total_len: 0,
            crc32: 0,
            chunks: Vec::new(),
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    pub fn transfer_id(&self) -> Option<[u8; 8]> {
        self.transfer_id
    }

    pub fn expected(&self) -> usize {
        self.count as usize
    }

    pub fn received(&self) -> usize {
        self.chunks.iter().filter(|c| c.is_some()).count()
    }

    pub fn have(&self) -> Vec<bool> {
        self.chunks.iter().map(|c| c.is_some()).collect()
    }

    /// Push a frame. Returns `Some(payload)` when the transfer is complete.
    pub fn push(&mut self, frame: Frame) -> Result<Option<Vec<u8>>, ProtocolError> {
        match self.transfer_id {
            None => {
                self.transfer_id = Some(frame.transfer_id);
                self.count = frame.count;
                self.total_len = frame.total_len;
                self.crc32 = frame.crc32;
                self.chunks = vec![None; frame.count as usize];
            }
            Some(id) if id != frame.transfer_id => {
                // A new transfer replaces an incomplete one.
                *self = Self::new();
                return self.push(frame);
            }
            Some(_) => {
                if frame.count != self.count
                    || frame.total_len != self.total_len
                    || frame.crc32 != self.crc32
                {
                    return Err(ProtocolError::TransferMismatch);
                }
            }
        }

        match self.chunks.get_mut(frame.index as usize) {
            // The same code seen again.
            Some(Some(held)) if *held == frame.chunk => {}
            // Another code at the same place: one of the two is not this
            // transfer's. Neither is kept, and the transfer starts over.
            Some(Some(_)) => {
                *self = Self::new();
                return Err(ProtocolError::ConflictingFrame);
            }
            Some(slot) => *slot = Some(frame.chunk),
            None => return Err(ProtocolError::BadHeader),
        }

        if self.received() != self.expected() {
            return Ok(None);
        }

        let mut payload = Vec::with_capacity(self.total_len as usize);
        for chunk in &self.chunks {
            payload.extend_from_slice(chunk.as_ref().unwrap());
        }
        // The last chunk is padded. What is past `total_len` is not payload.
        if payload.len() < self.total_len as usize {
            *self = Self::new();
            return Err(ProtocolError::CorruptPayload);
        }
        payload.truncate(self.total_len as usize);
        // The id is a hash of the payload, so a code from another transfer
        // that took one's place, or a misread one, is caught here. Not a
        // defence against a forged transfer: the device that signs decides
        // from what it reads.
        if crc32(&payload) != self.crc32 || Some(transfer_id(&payload)) != self.transfer_id {
            *self = Self::new();
            return Err(ProtocolError::CorruptPayload);
        }
        Ok(Some(payload))
    }
}

/// One scanned frame, for a progress line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameSeen {
    pub index: u16,
    pub count: u16,
    pub transfer_id: [u8; 8],
}

/// What one scanned QR did to the transfer in progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Collected {
    /// Not a QRS1 frame. Other codes in view are ignored.
    Unrelated,
    /// A frame of a transfer that still has frames missing.
    Collecting {
        frame: FrameSeen,
        received: usize,
        expected: usize,
    },
    /// The last missing frame. The payload is handed over once.
    Complete { frame: FrameSeen, payload: Vec<u8> },
    /// A frame of a transfer that was already handed over. The code is still in view.
    Repeat { frame: FrameSeen },
}

/// Feeds scanned QR contents to an [`Assembler`] and hands each finished
/// transfer over exactly once, however long its code stays in front of the camera.
#[derive(Debug, Clone, Default)]
pub struct Collector {
    assembler: Assembler,
    delivered: Vec<[u8; 8]>,
}

impl Collector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Which frames of the current transfer have arrived.
    pub fn have(&self) -> Vec<bool> {
        self.assembler.have()
    }

    pub fn push(&mut self, bytes: &[u8]) -> Result<Collected, ProtocolError> {
        let Ok(frame) = parse_frame(bytes) else {
            return Ok(Collected::Unrelated);
        };
        let seen = FrameSeen {
            index: frame.index,
            count: frame.count,
            transfer_id: frame.transfer_id,
        };
        match self.assembler.push(frame)? {
            Some(_) if self.delivered.contains(&seen.transfer_id) => {
                Ok(Collected::Repeat { frame: seen })
            }
            Some(payload) => {
                self.delivered.push(seen.transfer_id);
                Ok(Collected::Complete {
                    frame: seen,
                    payload,
                })
            }
            None => Ok(Collected::Collecting {
                frame: seen,
                received: self.assembler.received(),
                expected: self.assembler.expected(),
            }),
        }
    }
}
