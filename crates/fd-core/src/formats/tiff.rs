//! Minimal endian-aware TIFF/IFD walker, shared by CR2, CR3 CMTx boxes and
//! JPEG APP1 EXIF. Offsets in entries are relative to the TIFF header start,
//! so the parser owns the full TIFF-scoped byte slice.

#[derive(Debug, thiserror::Error)]
pub enum TiffError {
    #[error("not a TIFF structure")]
    BadMagic,
    #[error("truncated TIFF data")]
    Truncated,
}

#[derive(Debug, Clone, Copy)]
pub struct IfdEntry {
    pub tag: u16,
    pub kind: u16,
    pub count: u32,
    /// Raw 4 value/offset bytes from the entry.
    pub raw: [u8; 4],
}

pub struct Tiff<'a> {
    pub data: &'a [u8],
    pub le: bool,
    pub first_ifd: u32,
}

impl<'a> Tiff<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, TiffError> {
        if data.len() < 8 {
            return Err(TiffError::Truncated);
        }
        let le = match &data[0..2] {
            b"II" => true,
            b"MM" => false,
            _ => return Err(TiffError::BadMagic),
        };
        let t = Tiff {
            data,
            le,
            first_ifd: 0,
        };
        if t.u16(2).ok_or(TiffError::Truncated)? != 42 {
            return Err(TiffError::BadMagic);
        }
        let first_ifd = t.u32(4).ok_or(TiffError::Truncated)?;
        Ok(Tiff { first_ifd, ..t })
    }

    pub fn u16(&self, off: usize) -> Option<u16> {
        let b = self.data.get(off..off + 2)?;
        Some(if self.le {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        })
    }

    pub fn u32(&self, off: usize) -> Option<u32> {
        let b = self.data.get(off..off + 4)?;
        let a = [b[0], b[1], b[2], b[3]];
        Some(if self.le {
            u32::from_le_bytes(a)
        } else {
            u32::from_be_bytes(a)
        })
    }

    fn raw_u16(&self, raw: &[u8; 4]) -> u16 {
        if self.le {
            u16::from_le_bytes([raw[0], raw[1]])
        } else {
            u16::from_be_bytes([raw[0], raw[1]])
        }
    }

    fn raw_u32(&self, raw: &[u8; 4]) -> u32 {
        if self.le {
            u32::from_le_bytes(*raw)
        } else {
            u32::from_be_bytes(*raw)
        }
    }

    /// Parse the IFD at `offset`; returns entries and the next-IFD offset
    /// (0 = none). Tolerates truncation by returning what fits.
    pub fn ifd(&self, offset: u32) -> (Vec<IfdEntry>, u32) {
        let off = offset as usize;
        let Some(n) = self.u16(off) else {
            return (Vec::new(), 0);
        };
        let mut entries = Vec::with_capacity(n as usize);
        for i in 0..n as usize {
            let e = off + 2 + i * 12;
            let (Some(tag), Some(kind), Some(count)) =
                (self.u16(e), self.u16(e + 2), self.u32(e + 4))
            else {
                break;
            };
            let Some(raw) = self.data.get(e + 8..e + 12) else {
                break;
            };
            entries.push(IfdEntry {
                tag,
                kind,
                count,
                raw: [raw[0], raw[1], raw[2], raw[3]],
            });
        }
        let next = self.u32(off + 2 + entries.len() * 12).unwrap_or(0);
        (entries, next)
    }

    fn type_size(kind: u16) -> usize {
        match kind {
            1 | 2 | 6 | 7 => 1, // BYTE, ASCII, SBYTE, UNDEFINED
            3 | 8 => 2,         // SHORT, SSHORT
            4 | 9 | 11 => 4,    // LONG, SLONG, FLOAT
            5 | 10 | 12 => 8,   // RATIONAL, SRATIONAL, DOUBLE
            _ => 1,
        }
    }

    /// Slice holding the entry's value payload (inline or offset-addressed).
    pub fn value_bytes(&self, e: &IfdEntry) -> Option<&'a [u8]> {
        let size = Self::type_size(e.kind) * e.count as usize;
        if size <= 4 {
            // Inline: raw bytes ARE the value; can't borrow from raw (local),
            // so reconstruct from data via the entry position isn't possible
            // here — instead handle inline values in the typed getters.
            None
        } else {
            let off = self.raw_u32(&e.raw) as usize;
            self.data.get(off..off + size)
        }
    }

    pub fn ascii(&self, e: &IfdEntry) -> Option<String> {
        if e.kind != 2 {
            return None;
        }
        let bytes: &[u8] = if e.count as usize <= 4 {
            &e.raw[..e.count as usize]
        } else {
            self.value_bytes(e)?
        };
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        Some(String::from_utf8_lossy(&bytes[..end]).trim().to_string())
    }

    /// First value as u32 (SHORT or LONG).
    pub fn uint(&self, e: &IfdEntry) -> Option<u32> {
        match e.kind {
            3 => Some(self.raw_u16(&e.raw) as u32),
            4 => Some(self.raw_u32(&e.raw)),
            _ => None,
        }
    }

    /// First value as unsigned rational (num, den).
    pub fn rational(&self, e: &IfdEntry) -> Option<(u32, u32)> {
        if e.kind != 5 && e.kind != 10 {
            return None;
        }
        let v = self.value_bytes(e)?;
        let num = if self.le {
            u32::from_le_bytes([v[0], v[1], v[2], v[3]])
        } else {
            u32::from_be_bytes([v[0], v[1], v[2], v[3]])
        };
        let den = if self.le {
            u32::from_le_bytes([v[4], v[5], v[6], v[7]])
        } else {
            u32::from_be_bytes([v[4], v[5], v[6], v[7]])
        };
        Some((num, den))
    }
}
