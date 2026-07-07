pub mod age_crypto;
pub mod envelope;
pub mod noop;

use crate::Result;

pub trait CryptoProvider: Send + Sync {
    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>>;
    fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>>;
    fn is_encrypted(&self) -> bool;
    fn uses_envelope_encryption(&self) -> bool {
        false
    }
    fn remote_extension(&self, original_ext: &str) -> String;
}
