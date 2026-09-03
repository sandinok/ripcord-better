//! zlib-stream decompressor for the Discord gateway.
//!
//! Source-of-truth: research/gateway-spec.md §1, §8.
//! Discord sends zlib-compressed payloads with `compress=zlib-stream`:
//!   - ONE zlib stream for the whole connection (context takeover — the
//!     compressor dictionary carries across messages).
//!   - Each message ends with a sync-flush marker: `0x00 0x00 0xFF 0xFF`.
//!     That is not the end of the stream, just a message boundary.
//!
//! So we keep a single persistent `flate2::Decompress` context and, when the
//! buffer ends with the marker, decompress everything we have: the output
//! at that point is exactly one complete JSON payload.

use anyhow::{anyhow, Result};
use flate2::{Decompress, FlushDecompress, Status};

/// Stateful zlib-stream decoder with a persistent inflate context.
pub struct GatewayZlib {
    buf: Vec<u8>,
    decomp: Decompress,
    /// Growing allocation cap. If we exceed this without seeing a terminator
    /// we reset the buffer (Discord wouldn't legitimately send > 16 MiB).
    max_buf: usize,
}

impl GatewayZlib {
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(8 * 1024),
            decomp: Decompress::new(true), // zlib header
            max_buf: 16 * 1024 * 1024,
        }
    }

    /// Append bytes to the rolling buffer; if the buffer ends with the
    /// zlib sync-flush marker, decompress everything with the persistent
    /// context and return the payload. Otherwise return `None`.
    pub fn push_bytes(&mut self, bytes: &[u8]) -> Result<Option<Vec<u8>>> {
        self.buf.extend_from_slice(bytes);
        if self.buf.len() < 4 || !has_zlib_suffix(&self.buf) {
            if self.buf.len() > self.max_buf {
                self.buf.clear();
                return Err(anyhow!(
                    "zlib buffer overflowed max_buf={} - dropped",
                    self.max_buf
                ));
            }
            return Ok(None);
        }

        let mut out: Vec<u8> = Vec::with_capacity(self.buf.len().saturating_mul(4));
        let mut pos = 0usize;
        let mut scratch = vec![0u8; 64 * 1024];
        while pos < self.buf.len() {
            let old_in = self.decomp.total_in();
            let old_out = self.decomp.total_out();
            let status = self
                .decomp
                .decompress(&self.buf[pos..], &mut scratch, FlushDecompress::None)
                .map_err(|e| {
                    self.reset();
                    anyhow!("zlib decode ({} bytes in): {e}", self.buf.len())
                })?;
            let consumed = (self.decomp.total_in() - old_in) as usize;
            let produced = (self.decomp.total_out() - old_out) as usize;
            out.extend_from_slice(&scratch[..produced]);
            pos += consumed;
            if consumed == 0 && produced == 0 && pos < self.buf.len() {
                // No progress: the remaining input can't be decompressed yet.
                break;
            }
            if status == Status::StreamEnd {
                // The zlib stream actually finished (e.g. a Z_FINISH frame
                // in tests). Anything left in the buffer is the suffix
                // marker, not data for a fresh stream - discard it.
                self.decomp = Decompress::new(true);
                break;
            }
        }
        self.buf.clear();
        Ok(Some(out))
    }

    /// Reset internal state for a fresh reconnect.
    pub fn reset(&mut self) {
        self.buf.clear();
        self.decomp = Decompress::new(true);
    }
}

#[inline]
fn has_zlib_suffix(buf: &[u8]) -> bool {
    buf.len() >= 4
        && buf[buf.len() - 4] == 0x00
        && buf[buf.len() - 3] == 0x00
        && buf[buf.len() - 2] == 0xFF
        && buf[buf.len() - 1] == 0xFF
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compress, Compression, FlushCompress};
    use std::io::Write;

    /// Compress `data` on a persistent zlib stream, ending with a sync
    /// flush (the message-boundary marker Discord uses).
    fn sync_chunk(comp: &mut Compress, data: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; 16 * 1024];
        let before_in = comp.total_in();
        let before_out = comp.total_out();
        comp.compress(data, &mut out, FlushCompress::Sync).expect("compress");
        let _ = before_in;
        let produced = (comp.total_out() - before_out) as usize;
        out.truncate(produced);
        assert!(out.ends_with(&[0x00, 0x00, 0xFF, 0xFF]) || produced > 0);
        out
    }

    #[test]
    fn detects_suffix() {
        assert!(has_zlib_suffix(&[0x42, 0x42, 0x00, 0x00, 0xFF, 0xFF]));
        assert!(!has_zlib_suffix(&[0x42, 0x42]));
        assert!(!has_zlib_suffix(&[]));
    }

    #[test]
    fn round_trip_single_message() {
        // A sync-flushed chunk already ends with the 00 00 FF FF marker
        // (exactly like Discord's frames) - appending a second one would
        // corrupt the stream.
        let mut comp = Compress::new(Compression::default(), true);
        let frame = sync_chunk(&mut comp, b"hello discord");
        let mut z = GatewayZlib::new();
        match z.push_bytes(&frame).unwrap() {
            Some(out) => assert_eq!(&out, b"hello discord"),
            None => panic!("expected to detect suffix"),
        }
    }

    #[test]
    fn fragmented_delivery_across_pushes() {
        // A payload delivered in two WS frames must buffer, not error.
        let mut comp = Compress::new(Compression::default(), true);
        let encoded = sync_chunk(&mut comp, b"fragmented payload");
        let mut z = GatewayZlib::new();
        let half = encoded.len() / 2;
        assert!(z.push_bytes(&encoded[..half]).unwrap().is_none());
        match z.push_bytes(&encoded[half..]).unwrap() {
            Some(out) => assert_eq!(&out, b"fragmented payload"),
            None => panic!("expected payload after final fragment"),
        }
    }

    #[test]
    fn context_takeover_across_messages() {
        // Two messages on the SAME zlib stream (context takeover): the
        // second message must decode with the shared dictionary.
        let mut comp = Compress::new(Compression::default(), true);
        let frame1 = sync_chunk(&mut comp, b"first message about basalt");
        let frame2 = sync_chunk(&mut comp, b"second message about basalt too");

        let mut z = GatewayZlib::new();
        let out1 = z.push_bytes(&frame1).unwrap().expect("message 1");
        assert_eq!(&out1, b"first message about basalt");

        let out2 = z.push_bytes(&frame2).unwrap().expect("message 2 with shared context");
        assert_eq!(&out2, b"second message about basalt too");
    }

    #[test]
    fn finishes_stream_and_restarts() {
        // A Z_FINISH frame (StreamEnd) followed by a fresh frame still decodes.
        let mut z = GatewayZlib::new();
        // Message one: finished stream (like a reconnect without reset).
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(b"done stream").unwrap();
        let mut frame1 = encoder.finish().unwrap();
        frame1.extend_from_slice(&[0x00, 0x00, 0xFF, 0xFF]);
        let out1 = z.push_bytes(&frame1).unwrap().expect("finished frame decodes");
        assert_eq!(&out1, b"done stream");
        // Message two on a fresh stream.
        let mut comp = Compress::new(Compression::default(), true);
        let mut frame2 = sync_chunk(&mut comp, b"new stream message");
        frame2.extend_from_slice(&[0x00, 0x00, 0xFF, 0xFF]);
        let out2 = z.push_bytes(&frame2).unwrap().expect("new stream decodes");
        assert_eq!(&out2, b"new stream message");
    }
}
