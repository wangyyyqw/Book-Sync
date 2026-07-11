use super::CryptoProvider;
use crate::Result;
use std::path::Path;

#[derive(Debug, Default)]
pub struct NoopCrypto;

impl CryptoProvider for NoopCrypto {
    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        Ok(plaintext.to_vec())
    }

    fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        Ok(ciphertext.to_vec())
    }

    fn encrypt_file(&self, input: &Path, output: &Path) -> Result<()> {
        std::fs::copy(input, output)?;
        Ok(())
    }

    fn decrypt_file(&self, input: &Path, output: &Path) -> Result<()> {
        std::fs::copy(input, output)?;
        Ok(())
    }

    fn is_encrypted(&self) -> bool {
        false
    }

    fn remote_extension(&self, original_ext: &str) -> String {
        original_ext.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_roundtrip() {
        let crypto = NoopCrypto;
        let bytes = b"kmo-sync";
        let encrypted = crypto.encrypt(bytes).unwrap();
        assert_eq!(encrypted, bytes);
        assert_eq!(crypto.decrypt(&encrypted).unwrap(), bytes);
        assert!(!crypto.is_encrypted());
        assert_eq!(crypto.remote_extension("meta"), "meta");
    }
}
