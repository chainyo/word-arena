use std::{
    fmt::Write as _,
    fs::File,
    io::{BufReader, Read},
    path::Path,
};

use sha2::{Digest, Sha256};

use crate::BuilderError;

const BUFFER_SIZE: usize = 64 * 1024;

pub(crate) fn sha256_file(path: &Path) -> Result<String, BuilderError> {
    let file = File::open(path).map_err(|source| BuilderError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; BUFFER_SIZE].into_boxed_slice();
    loop {
        let bytes_read = reader
            .read(&mut buffer)
            .map_err(|source| BuilderError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }
    Ok(digest_hex(hasher))
}

fn digest_hex(hasher: Sha256) -> String {
    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

pub(crate) fn diagnostic_tail(bytes: &[u8]) -> String {
    let value = String::from_utf8_lossy(bytes);
    let lines = value.lines().collect::<Vec<_>>();
    lines[lines.len().saturating_sub(40)..].join("\n")
}
