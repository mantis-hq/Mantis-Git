//! A tiny binary format for cache entries: little-endian integers, and strings and byte
//! runs behind a length. Reading never panics and never trusts a length, so a damaged
//! entry is just a `None`.

#[derive(Default)]
pub struct Writer(Vec<u8>);

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn u8(&mut self, value: u8) {
        self.0.push(value);
    }

    pub fn bool(&mut self, value: bool) {
        self.u8(u8::from(value));
    }

    pub fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    pub fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    /// A count or length. Entries are far below 4 GiB, so a longer one is a bug.
    pub fn len(&mut self, value: usize) {
        self.u32(u32::try_from(value).expect("cache entry part under 4 GiB"));
    }

    pub fn str(&mut self, value: &str) {
        self.len(value.len());
        self.0.extend_from_slice(value.as_bytes());
    }

    pub fn bytes(&mut self, value: &[u8]) {
        self.len(value.len());
        self.0.extend_from_slice(value);
    }

    pub fn opt_u32(&mut self, value: Option<u32>) {
        self.bool(value.is_some());
        if let Some(value) = value {
            self.u32(value);
        }
    }

    pub fn opt_str(&mut self, value: Option<&str>) {
        self.bool(value.is_some());
        if let Some(value) = value {
            self.str(value);
        }
    }

    pub fn finish(self) -> Vec<u8> {
        self.0
    }
}

pub struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let (head, rest) = self.bytes.split_at_checked(count)?;
        self.bytes = rest;
        Some(head)
    }

    pub fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    pub fn bool(&mut self) -> Option<bool> {
        match self.u8()? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }

    pub fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    pub fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    pub fn len(&mut self) -> Option<usize> {
        usize::try_from(self.u32()?).ok()
    }

    pub fn str(&mut self) -> Option<&'a str> {
        let length = self.len()?;
        std::str::from_utf8(self.take(length)?).ok()
    }

    pub fn bytes(&mut self) -> Option<&'a [u8]> {
        let length = self.len()?;
        self.take(length)
    }

    pub fn opt_u32(&mut self) -> Option<Option<u32>> {
        if self.bool()? {
            Some(Some(self.u32()?))
        } else {
            Some(None)
        }
    }

    pub fn opt_str(&mut self) -> Option<Option<&'a str>> {
        if self.bool()? {
            Some(Some(self.str()?))
        } else {
            Some(None)
        }
    }

    /// How many bytes are still to be read.
    pub fn remaining(&self) -> usize {
        self.bytes.len()
    }

    /// Whether every byte was read. A decoder should check this, so that an entry with
    /// something extra on the end is refused rather than half-understood.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_written_reads_back() {
        let mut out = Writer::new();
        out.u8(7);
        out.bool(true);
        out.u32(1_000_000);
        out.u64(u64::MAX);
        out.str("héllo");
        out.opt_str(None);
        out.opt_str(Some("x"));
        let bytes = out.finish();

        let mut input = Reader::new(&bytes);
        assert_eq!(input.u8(), Some(7));
        assert_eq!(input.bool(), Some(true));
        assert_eq!(input.u32(), Some(1_000_000));
        assert_eq!(input.u64(), Some(u64::MAX));
        assert_eq!(input.str(), Some("héllo"));
        assert_eq!(input.opt_str(), Some(None));
        assert_eq!(input.opt_str(), Some(Some("x")));
        assert!(input.is_empty());
    }

    #[test]
    fn short_or_lying_input_is_none_not_a_panic() {
        assert_eq!(Reader::new(&[1, 2]).u32(), None);
        // A string that claims four billion bytes.
        assert_eq!(Reader::new(&[255, 255, 255, 255, b'a']).str(), None);
        assert_eq!(Reader::new(&[2]).bool(), None);
        assert_eq!(Reader::new(&[1, 0, 0, 0, 0xff]).str(), None);
    }
}
