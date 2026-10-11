use std::{
    fs::File,
    io::{BufReader, Read, Seek},
};

use anyhow::{Context, Result, bail, ensure};

#[derive(Clone, Copy, Debug)]
pub(super) struct Range {
    pub(super) start: u64,
    pub(super) len: u64,
}

#[derive(Debug)]
pub(super) enum Value {
    Integer(u64),
    Bytes(Range),
    Fixed,
}

// A bounded, seekable protobuf reader. Length-delimited fields are represented
// by file offsets: indexing a country never allocates its CIDRs or domains.
pub(super) struct Reader<'a> {
    input: BufReader<&'a File>,
    position: u64,
    end: u64,
}

impl<'a> Reader<'a> {
    pub(super) fn new(file: &'a File) -> Result<Self> {
        let mut input = BufReader::new(file);
        input.rewind()?;
        Ok(Self {
            input,
            position: 0,
            end: file.metadata()?.len(),
        })
    }

    fn integer(&mut self) -> Result<u64> {
        let mut value = 0;
        for shift in (0..70).step_by(7) {
            ensure!(self.position < self.end, "truncated protobuf integer");
            let mut byte = [0];
            self.input.read_exact(&mut byte)?;
            self.position += 1;
            ensure!(shift < 63 || byte[0] <= 1, "protobuf integer overflow");
            value |= u64::from(byte[0] & 0x7f) << shift;
            if byte[0] & 0x80 == 0 {
                return Ok(value);
            }
        }
        bail!("protobuf integer overflow")
    }

    fn seek(&mut self, position: u64) -> Result<()> {
        let delta = i64::try_from(i128::from(position) - i128::from(self.position))?;
        self.input.seek_relative(delta)?;
        self.position = position;
        Ok(())
    }

    fn skip(&mut self, len: u64) -> Result<()> {
        let end = self
            .position
            .checked_add(len)
            .context("protobuf offset overflow")?;
        ensure!(end <= self.end, "truncated protobuf field");
        self.seek(end)
    }

    pub(super) fn field(&mut self) -> Result<Option<(u32, Value)>> {
        if self.position == self.end {
            return Ok(None);
        }
        let key = self.integer()?;
        let number = u32::try_from(key >> 3)?;
        ensure!(
            (1..=0x1fff_ffff).contains(&number),
            "invalid protobuf field number"
        );
        let value = match key & 7 {
            0 => Value::Integer(self.integer()?),
            1 => {
                self.skip(8)?;
                Value::Fixed
            }
            2 => {
                let len = self.integer()?;
                let range = Range {
                    start: self.position,
                    len,
                };
                self.skip(len)?;
                Value::Bytes(range)
            }
            5 => {
                self.skip(4)?;
                Value::Fixed
            }
            _ => bail!("unsupported protobuf wire type"),
        };
        Ok(Some((number, value)))
    }

    pub(super) fn within<T>(
        &mut self,
        range: Range,
        read: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        let position = self.position;
        let end = self.end;
        self.seek(range.start)?;
        self.end = range.start + range.len;
        let result = read(self);
        self.end = end;
        self.seek(position)?;
        result
    }

    pub(super) fn bytes<const N: usize>(&mut self, range: Range) -> Result<[u8; N]> {
        ensure!(range.len == N as u64, "invalid protobuf byte length");
        self.within(range, |reader| {
            let mut bytes = [0; N];
            reader.input.read_exact(&mut bytes)?;
            reader.position += N as u64;
            Ok(bytes)
        })
    }

    pub(super) fn string(&mut self, range: Range, limit: usize) -> Result<String> {
        ensure!(range.len <= limit as u64, "protobuf string is too large");
        self.within(range, |reader| {
            let mut bytes = vec![0; usize::try_from(range.len)?];
            reader.input.read_exact(&mut bytes)?;
            reader.position += range.len;
            String::from_utf8(bytes).context("invalid protobuf UTF-8")
        })
    }
}
