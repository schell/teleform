//! Utilities for working with `teleform`.

/// Returns the sha256 digest of the file at the given path *if it exists*.
/// If the file does _not_ exist it returns `Ok(None)`.
pub fn sha256_digest(path: impl AsRef<std::path::Path>) -> anyhow::Result<Option<String>> {
    log::trace!("determining sha256 of {}", path.as_ref().display());
    if !path.as_ref().exists() {
        return Ok(None);
    }

    fn sha256<R: std::io::Read>(mut reader: R) -> anyhow::Result<ring::digest::Digest> {
        let mut context = ring::digest::Context::new(&ring::digest::SHA256);
        let mut buffer = [0; 1024];

        loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            context.update(&buffer[..count]);
        }

        Ok(context.finish())
    }

    let input = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(input);
    let digest = sha256(reader)?;
    Ok(Some(data_encoding::HEXUPPER.encode(digest.as_ref())))
}
