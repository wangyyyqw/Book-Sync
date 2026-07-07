use super::CryptoProvider;
use crate::{Result, SyncError};
use age::x25519::Identity;
use std::io::{Read, Write};
use std::str::FromStr;

#[derive(Clone)]
pub struct AgeCrypto {
    identity: Identity,
}

impl AgeCrypto {
    pub fn from_identity(identity: Identity) -> Self {
        Self { identity }
    }

    pub fn from_identity_string(identity: &str) -> Result<Self> {
        let identity = Identity::from_str(identity.trim())
            .map_err(|err| SyncError::Crypto(format!("invalid age identity: {err}")))?;
        Ok(Self::from_identity(identity))
    }
}

impl CryptoProvider for AgeCrypto {
    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let recipient = self.identity.to_public();
        let encryptor = age::Encryptor::with_recipients(std::iter::once(&recipient as _))
            .map_err(|err| SyncError::Crypto(err.to_string()))?;

        let mut encrypted = Vec::new();
        let mut writer = encryptor
            .wrap_output(&mut encrypted)
            .map_err(|err| SyncError::Crypto(err.to_string()))?;
        writer.write_all(plaintext)?;
        writer
            .finish()
            .map_err(|err| SyncError::Crypto(err.to_string()))?;
        Ok(encrypted)
    }

    fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        let decryptor =
            age::Decryptor::new(ciphertext).map_err(|err| SyncError::Crypto(err.to_string()))?;
        let mut reader = decryptor
            .decrypt(std::iter::once(&self.identity as _))
            .map_err(|err| SyncError::Crypto(err.to_string()))?;
        let mut plaintext = Vec::new();
        reader.read_to_end(&mut plaintext)?;
        Ok(plaintext)
    }

    fn is_encrypted(&self) -> bool {
        true
    }

    fn remote_extension(&self, original_ext: &str) -> String {
        format!("{original_ext}.enc")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use age::secrecy::ExposeSecret;

    #[test]
    fn age_roundtrip_and_extension() {
        let identity = Identity::generate();
        let crypto = AgeCrypto::from_identity(identity);
        let plaintext = b"kmo-sync encrypted meta";
        let encrypted = crypto.encrypt(plaintext).unwrap();

        assert_ne!(encrypted, plaintext);
        assert_eq!(crypto.decrypt(&encrypted).unwrap(), plaintext);
        assert!(crypto.is_encrypted());
        assert_eq!(crypto.remote_extension("meta"), "meta.enc");
    }

    #[test]
    fn wrong_identity_fails_to_decrypt() {
        let crypto_a = AgeCrypto::from_identity(Identity::generate());
        let crypto_b = AgeCrypto::from_identity(Identity::generate());
        let encrypted = crypto_a.encrypt(b"secret").unwrap();
        assert!(crypto_b.decrypt(&encrypted).is_err());
    }

    #[test]
    fn identity_string_parses() {
        let identity = Identity::generate();
        let identity_string = identity.to_string().expose_secret().to_string();
        let crypto = AgeCrypto::from_identity_string(&identity_string).unwrap();
        let encrypted = crypto.encrypt(b"hello").unwrap();
        assert_eq!(crypto.decrypt(&encrypted).unwrap(), b"hello");
    }
}
