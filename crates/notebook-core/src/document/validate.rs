//! Structural check of a format-1 payload before Yrs decodes it.
//!
//! Yrs trusts the structure of what it decodes: counts size allocations up
//! front, clock arithmetic is unchecked, and its integration assumes the
//! shapes its own encoder produces. Every payload is walked here first with
//! a strict reader of our own, and anything outside what a note document
//! writes is rejected before Yrs sees it. Some Yrs integration bugs remain
//! reachable by a crafted update from a key holder; that risk is accepted
//! (see the design).

use std::collections::HashSet;

use super::doc::{ROOT_BODY, ROOT_META};

/// The largest plaintext accepted for one update or snapshot.
pub const MAX_PLAINTEXT_LEN: usize = 16 * 1024 * 1024;

/// The highest clock any block, origin, or deleted range may reach. Far
/// beyond what one client writes to one note, and low enough that Yrs's
/// `i32` clock offsets can't overflow.
const MAX_CLOCK: u32 = i32::MAX as u32;

/// How deeply `Any` arrays and maps may nest.
const MAX_ANY_DEPTH: u32 = 32;

/// The longest lib0 signed varint accepted: 6 + 7 * 8 = 62 value bits, so
/// Yrs's decoder can neither overflow its shift nor its negation.
const MAX_SIGNED_VAR_BYTES: usize = 9;

const BLOCK_GC: u8 = 0;
const BLOCK_SKIP: u8 = 10;
const CONTENT_DELETED: u8 = 1;
const CONTENT_STRING: u8 = 4;
const CONTENT_ANY: u8 = 8;
const HAS_ORIGIN: u8 = 0b1000_0000;
const HAS_RIGHT_ORIGIN: u8 = 0b0100_0000;
const HAS_PARENT_SUB: u8 = 0b0010_0000;

/// Marker for a payload that fails the check.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Invalid;

/// Checks that `payload` is a Yrs v1 update shaped the way a note document
/// writes one:
///
/// - every unsigned varint is at most `u32::MAX` in at most five bytes, so
///   Yrs reads it the same whatever width it uses;
/// - every count fits in the bytes that remain, and every clock and clock
///   range ends at or below [`MAX_CLOCK`];
/// - each client appears once in the block section and once in the delete
///   set, and its deleted ranges are sorted, disjoint, and non-empty;
/// - no block is empty;
/// - items name their parent only as the `body` or `meta` root (never by
///   ID, since a note document has no nested types);
/// - item content is a string, `Any` values, or a deleted range;
/// - nothing follows the delete set, and the whole payload is at most
///   [`MAX_PLAINTEXT_LEN`] bytes.
pub(crate) fn check_update(payload: &[u8]) -> Result<(), Invalid> {
    if payload.len() > MAX_PLAINTEXT_LEN {
        return Err(Invalid);
    }
    let mut r = Reader { buf: payload };
    let mut seen = HashSet::new();
    for _ in 0..r.count()? {
        let blocks = r.count()?;
        if !seen.insert(r.var()?) {
            return Err(Invalid);
        }
        let mut clock = r.clock()?;
        for _ in 0..blocks {
            let len = r.block()?;
            if len == 0 {
                return Err(Invalid);
            }
            clock = end(clock, len)?;
        }
    }
    r.delete_set()?;
    if !r.buf.is_empty() {
        return Err(Invalid);
    }
    Ok(())
}

/// The end of the clock range `start..start + len`, if within bounds.
fn end(start: u32, len: u32) -> Result<u32, Invalid> {
    start
        .checked_add(len)
        .filter(|&end| end <= MAX_CLOCK)
        .ok_or(Invalid)
}

struct Reader<'a> {
    buf: &'a [u8],
}

impl<'a> Reader<'a> {
    fn u8(&mut self) -> Result<u8, Invalid> {
        let (&byte, rest) = self.buf.split_first().ok_or(Invalid)?;
        self.buf = rest;
        Ok(byte)
    }

    /// An unsigned lib0 varint of at most `u32::MAX`.
    fn var(&mut self) -> Result<u32, Invalid> {
        let mut value: u64 = 0;
        for shift in (0..35).step_by(7) {
            let byte = self.u8()?;
            value |= u64::from(byte & 0x7f) << shift;
            if byte < 0x80 {
                return u32::try_from(value).map_err(|_| Invalid);
            }
        }
        Err(Invalid)
    }

    /// Skips a signed lib0 varint of at most [`MAX_SIGNED_VAR_BYTES`].
    fn signed_var(&mut self) -> Result<(), Invalid> {
        for _ in 0..MAX_SIGNED_VAR_BYTES {
            if self.u8()? < 0x80 {
                return Ok(());
            }
        }
        Err(Invalid)
    }

    /// A count of things that each take at least one byte.
    fn count(&mut self) -> Result<u32, Invalid> {
        let count = self.var()?;
        if count as usize > self.buf.len() {
            return Err(Invalid);
        }
        Ok(count)
    }

    fn bytes(&mut self, len: usize) -> Result<&'a [u8], Invalid> {
        if len > self.buf.len() {
            return Err(Invalid);
        }
        let (bytes, rest) = self.buf.split_at(len);
        self.buf = rest;
        Ok(bytes)
    }

    fn buf(&mut self) -> Result<&'a [u8], Invalid> {
        let len = self.var()?;
        self.bytes(len as usize)
    }

    fn string(&mut self) -> Result<&'a str, Invalid> {
        std::str::from_utf8(self.buf()?).map_err(|_| Invalid)
    }

    fn id(&mut self) -> Result<(), Invalid> {
        self.var()?;
        self.clock()?;
        Ok(())
    }

    fn clock(&mut self) -> Result<u32, Invalid> {
        let clock = self.var()?;
        end(clock, 0)
    }

    /// Reads one block and returns how far it advances its client's clock.
    fn block(&mut self) -> Result<u32, Invalid> {
        let info = self.u8()?;
        if info == BLOCK_SKIP || info == BLOCK_GC {
            return self.var();
        }
        if info & HAS_ORIGIN != 0 {
            self.id()?;
        }
        if info & HAS_RIGHT_ORIGIN != 0 {
            self.id()?;
        }
        if info & (HAS_ORIGIN | HAS_RIGHT_ORIGIN) == 0 {
            if self.var()? != 1 || !matches!(self.string()?, ROOT_BODY | ROOT_META) {
                return Err(Invalid);
            }
            if info & HAS_PARENT_SUB != 0 {
                self.string()?;
            }
        }
        match info & 0b1111 {
            CONTENT_DELETED => self.var(),
            CONTENT_STRING => {
                let len = self.string()?.encode_utf16().count();
                u32::try_from(len).map_err(|_| Invalid)
            }
            CONTENT_ANY => {
                let count = self.count()?;
                for _ in 0..count {
                    self.any(0)?;
                }
                Ok(count)
            }
            _ => Err(Invalid),
        }
    }

    /// Skips one lib0 `Any` value.
    fn any(&mut self, depth: u32) -> Result<(), Invalid> {
        if depth > MAX_ANY_DEPTH {
            return Err(Invalid);
        }
        match self.u8()? {
            // undefined, null, false, true
            127 | 126 | 121 | 120 => {}
            // integer
            125 => self.signed_var()?,
            // float32, float64, bigint
            124 => drop(self.bytes(4)?),
            123 | 122 => drop(self.bytes(8)?),
            // string, buffer
            119 | 116 => drop(self.buf()?),
            // map
            118 => {
                for _ in 0..self.count()? {
                    self.string()?;
                    self.any(depth + 1)?;
                }
            }
            // array
            117 => {
                for _ in 0..self.count()? {
                    self.any(depth + 1)?;
                }
            }
            _ => return Err(Invalid),
        }
        Ok(())
    }

    fn delete_set(&mut self) -> Result<(), Invalid> {
        let mut seen = HashSet::new();
        for _ in 0..self.count()? {
            if !seen.insert(self.var()?) {
                return Err(Invalid);
            }
            let mut previous_end = 0;
            for _ in 0..self.count()? {
                let clock = self.clock()?;
                let len = self.var()?;
                if len == 0 || clock < previous_end {
                    return Err(Invalid);
                }
                previous_end = end(clock, len)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        let text: String = text.split_whitespace().collect();
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn empty_update_passes() {
        assert_eq!(check_update(&[0, 0]), Ok(()));
    }

    #[test]
    fn insert_into_body_passes() {
        // client 66 inserts "z" into root `body`.
        assert_eq!(
            check_update(&hex("01 01 42 00 04 01 04 626f6479 01 7a 00")),
            Ok(())
        );
    }

    #[test]
    fn rejects() {
        let cases = [
            ("", "empty"),
            ("ffffffff0f", "huge client count"),
            ("01 ffffffff0f 01 00", "huge block count"),
            ("01 02 01 00 0a ffffffff0f 0a 01 00", "skip past u32::MAX"),
            ("01 01 01 00 0a ffffffff0f 00", "skip of u32::MAX"),
            (
                "01 01 01 00 84 01 ffffffff0f 01 04 626f6479 01 7a 00",
                "origin clock",
            ),
            ("01 01 01 feffffff0f 00 05 00", "gc past u32::MAX"),
            ("00 01 01 01 feffffff0f 05", "delete range past u32::MAX"),
            (
                "01 01 01 feffffff0f 04 01 04 626f6479 03 616263 00",
                "string past u32::MAX",
            ),
            ("01 01 02 00 04 00 63 00 01 78 00", "explicit ID parent"),
            ("01 01 02 00 04 01 03 666f6f 01 78 00", "foreign root"),
            ("01 01 02 00 05 01 04 626f6479 01 7b 00", "embed content"),
            ("01 01 02 00 07 01 04 626f6479 00 00", "type content"),
            ("01 01 02 00 08 01 04 6d657461 01 ff 00", "unknown any tag"),
            (
                "01 01 02 00 04 01 04 626f6479 01 7a 00 deadbeef",
                "trailing bytes",
            ),
            ("01 01 02 00 04 01 04 626f6479 05 7a 00", "string past end"),
            ("01 01 05 00 04 01 04 626f6479 00 00", "empty string"),
            ("01 01 05 00 00 00 00", "empty gc"),
            ("01 01 05 00 0a 00 00", "empty skip"),
            ("01 01 05 00 01 01 04 626f6479 00 00", "empty deleted"),
            ("01 01 05 00 08 01 04 6d657461 00 00", "empty any"),
            ("01 01 05 8080808008 00 01 00", "clock 2^31"),
            (
                "01 01 05 ffffffff07 04 01 04 626f6479 01 78 00",
                "range to 2^31",
            ),
            ("01 01 05 00 0a 8080808010 00", "varint above u32"),
            ("01 01 05 00 0a 8180808080 01 00", "six-byte varint"),
            (
                "02 01 05 00 04 01 04 626f6479 01 42 01 05 00 04 01 04 626f6479 01 41 00",
                "client repeated in blocks",
            ),
            (
                "02 01 e807 0f 04 01 04 626f6479 01 61 01 e807 02 00 03 00",
                "repeated, backwards",
            ),
            (
                "00 02 01 01 00 01 01 01 02 01",
                "client repeated in deletes",
            ),
            ("00 01 01 02 05 01 02 01", "deletes unsorted"),
            ("00 01 01 02 01 03 02 01", "deletes overlapping"),
            ("00 01 01 01 02 00", "empty delete range"),
            (
                "01 01 42 00 08 01 04 6d657461 01 75 8080808010 00",
                "any array count 2^32",
            ),
            (
                "01 01 01 00 08 01 04 6d657461 01 76 81 80 80 80 10 01 61 7e 00",
                "any map count wrapping to 1",
            ),
            (
                "01 01 01 00 08 01 04 6d657461 01 7d bf ff ff ff ff ff ff ff ff ff ff 00 00",
                "overlong any integer",
            ),
            (
                "01 01 01 00 08 01 04 6d657461 01 7d c0 80 80 80 80 80 80 80 80 02 00",
                "any integer i64::MIN",
            ),
        ];
        for (bytes, why) in cases {
            assert_eq!(check_update(&hex(bytes)), Err(Invalid), "{why}");
        }
    }

    #[test]
    fn rejects_deeply_nested_any() {
        let mut payload = hex("01 01 02 00 08 01 04 6d657461 01");
        payload.extend(std::iter::repeat_n([0x75, 0x01], 40).flatten());
        payload.extend([0x7e, 0x00]);
        assert_eq!(check_update(&payload), Err(Invalid));
    }

    #[test]
    fn rejects_oversized_payload() {
        let mut payload = vec![0; MAX_PLAINTEXT_LEN + 1];
        payload[1] = 0;
        assert_eq!(check_update(&payload), Err(Invalid));
    }
}
