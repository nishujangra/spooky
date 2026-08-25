use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

#[derive(Debug)]
pub(crate) enum BoundedFileReadError {
    Io(io::Error),
    NotAFile,
    TooLarge,
}

fn read_with_limit<R: Read>(
    reader: R,
    initial_capacity: usize,
    max_bytes: u64,
) -> Result<Vec<u8>, BoundedFileReadError> {
    let max_bytes_usize = usize::try_from(max_bytes).unwrap_or(usize::MAX);
    let mut bytes = Vec::with_capacity(initial_capacity.min(max_bytes_usize));
    reader
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(BoundedFileReadError::Io)?;
    if bytes.len() > max_bytes_usize {
        return Err(BoundedFileReadError::TooLarge);
    }

    Ok(bytes)
}

pub(crate) fn read_file_with_limit(
    path: impl AsRef<Path>,
    max_bytes: u64,
) -> Result<Vec<u8>, BoundedFileReadError> {
    let file = File::open(path).map_err(BoundedFileReadError::Io)?;
    let metadata = file.metadata().map_err(BoundedFileReadError::Io)?;
    if !metadata.is_file() {
        return Err(BoundedFileReadError::NotAFile);
    }
    if metadata.len() > max_bytes {
        return Err(BoundedFileReadError::TooLarge);
    }

    read_with_limit(
        file,
        usize::try_from(metadata.len()).unwrap_or(usize::MAX),
        max_bytes,
    )
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::{self, Read},
    };

    use tempfile::tempdir;

    use super::{BoundedFileReadError, read_file_with_limit, read_with_limit};

    struct ChunkedReader {
        chunks: Vec<Vec<u8>>,
        chunk_index: usize,
        offset: usize,
    }

    impl ChunkedReader {
        fn new(chunks: Vec<Vec<u8>>) -> Self {
            Self {
                chunks,
                chunk_index: 0,
                offset: 0,
            }
        }
    }

    impl Read for ChunkedReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.chunk_index >= self.chunks.len() {
                return Ok(0);
            }

            let chunk = &self.chunks[self.chunk_index];
            let remaining = &chunk[self.offset..];
            let len = remaining.len().min(buf.len());
            buf[..len].copy_from_slice(&remaining[..len]);
            self.offset += len;
            if self.offset == chunk.len() {
                self.chunk_index += 1;
                self.offset = 0;
            }
            Ok(len)
        }
    }

    #[test]
    fn read_file_with_limit_accepts_file_at_limit() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("bounded.txt");
        fs::write(&path, vec![b'x'; 64]).expect("write");

        let bytes = read_file_with_limit(&path, 64).expect("file at size limit should load");

        assert_eq!(bytes.len(), 64);
    }

    #[test]
    fn read_with_limit_rejects_stream_that_exceeds_limit_during_read() {
        let reader = ChunkedReader::new(vec![vec![b'a'; 32], vec![b'b'; 33]]);

        let err = read_with_limit(reader, 32, 64).expect_err("stream must exceed bounded limit");

        assert!(matches!(err, BoundedFileReadError::TooLarge));
    }
}
