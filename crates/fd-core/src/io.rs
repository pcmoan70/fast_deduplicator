use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

/// Minimal random-access read abstraction (the future Vfs seam for wasm).
pub trait ReadRange {
    fn read_range(&mut self, offset: u64, len: usize) -> io::Result<Vec<u8>>;
    fn len(&self) -> u64;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// File-backed reader that keeps the header chunk cached, tracks bytes read
/// (for benchmarking), and preads further ranges on demand.
pub struct FileReader {
    file: File,
    size: u64,
    header: Vec<u8>,
    pub bytes_read: u64,
}

/// Size of the initial header read. CR3 `moov` (incl. Canon makernotes)
/// typically fits well within this.
pub const HEADER_READ: usize = 256 * 1024;

impl FileReader {
    pub fn open(path: &Path) -> io::Result<Self> {
        let mut file = File::open(path)?;
        let size = file.metadata()?.len();
        let want = HEADER_READ.min(size as usize);
        let mut header = vec![0u8; want];
        file.read_exact(&mut header)?;
        Ok(FileReader {
            file,
            size,
            bytes_read: want as u64,
            header,
        })
    }

    pub fn header(&self) -> &[u8] {
        &self.header
    }
}

impl ReadRange for FileReader {
    fn read_range(&mut self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let end = offset + len as u64;
        if end <= self.header.len() as u64 {
            let s = offset as usize;
            return Ok(self.header[s..s + len].to_vec());
        }
        if end > self.size {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "range beyond end of file",
            ));
        }
        self.file.seek(SeekFrom::Start(offset))?;
        let mut buf = vec![0u8; len];
        self.file.read_exact(&mut buf)?;
        self.bytes_read += len as u64;
        Ok(buf)
    }

    fn len(&self) -> u64 {
        self.size
    }
}
