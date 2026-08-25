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

pub(crate) fn read_file_with_limit(
    path: impl AsRef<Path>,
    max_bytes: u64,
) -> Result<Vec<u8>, BoundedFileReadError> {
    let mut file = File::open(path).map_err(BoundedFileReadError::Io)?;
    let metadata = file.metadata().map_err(BoundedFileReadError::Io)?;
    if !metadata.is_file() {
        return Err(BoundedFileReadError::NotAFile);
    }
    if metadata.len() > max_bytes {
        return Err(BoundedFileReadError::TooLarge);
    }

    let max_bytes_usize = usize::try_from(max_bytes).unwrap_or(usize::MAX);
    let mut bytes = Vec::with_capacity(
        usize::try_from(metadata.len())
            .unwrap_or(usize::MAX)
            .min(max_bytes_usize),
    );
    file.take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(BoundedFileReadError::Io)?;
    if bytes.len() > max_bytes_usize {
        return Err(BoundedFileReadError::TooLarge);
    }

    Ok(bytes)
}
