use super::CryptoProvider;
use crate::{Result, SyncError};
use aes_gcm::aead::rand_core::RngCore;
use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

const ENVELOPE_MAGIC: &[u8; 8] = b"KMOENV1\0";
const HEADER_LEN_SIZE: usize = 4;
const KEY_SIZE: usize = 32;
const NONCE_SIZE: usize = 12;
const SALT_SIZE: usize = 16;
const DEFAULT_ARGON2_M_COST: u32 = 19 * 1024;
const DEFAULT_ARGON2_T_COST: u32 = 2;
const DEFAULT_ARGON2_P_COST: u32 = 1;

#[derive(Debug, Clone)]
pub struct EnvelopeCrypto {
    kek_material: KekMaterial,
    kek_id: String,
    kek_version: u32,
    kdf_params: EnvelopeKdfParams,
}

#[derive(Debug, Clone)]
enum KekMaterial {
    Raw([u8; KEY_SIZE]),
    Passphrase(String),
}

impl Drop for KekMaterial {
    fn drop(&mut self) {
        if let Self::Raw(kek) = self {
            kek.zeroize();
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EnvelopeKdfParams {
    pub memory_cost: u32,
    pub time_cost: u32,
    pub parallelism: u32,
}

impl Default for EnvelopeKdfParams {
    fn default() -> Self {
        Self {
            memory_cost: DEFAULT_ARGON2_M_COST,
            time_cost: DEFAULT_ARGON2_T_COST,
            parallelism: DEFAULT_ARGON2_P_COST,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct EnvelopeHeader {
    version: u32,
    kek_id: String,
    kek_version: u32,
    mode: EnvelopeMode,
    kdf: Option<EnvelopeKdfHeader>,
    edek_nonce_hex: String,
    edek_hex: String,
    data_nonce_hex: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum EnvelopeMode {
    RawKek,
    Argon2id,
}

#[derive(Debug, Deserialize, Serialize)]
struct EnvelopeKdfHeader {
    salt_hex: String,
    memory_cost: u32,
    time_cost: u32,
    parallelism: u32,
}

impl EnvelopeCrypto {
    pub fn from_passphrase(
        passphrase: impl Into<String>,
        kek_id: Option<String>,
        kek_version: Option<u32>,
        kdf_params: Option<EnvelopeKdfParams>,
    ) -> Result<Self> {
        let passphrase = passphrase.into();
        if passphrase.is_empty() {
            return Err(SyncError::InvalidArg(
                "envelope encryption requires passphrase".to_string(),
            ));
        }
        Ok(Self {
            kek_material: KekMaterial::Passphrase(passphrase),
            kek_id: normalized_kek_id(kek_id),
            kek_version: kek_version.unwrap_or(1),
            kdf_params: kdf_params.unwrap_or_default(),
        })
    }

    pub fn from_kek_hex(
        kek_hex: &str,
        kek_id: Option<String>,
        kek_version: Option<u32>,
    ) -> Result<Self> {
        let bytes = hex::decode(kek_hex.trim())
            .map_err(|err| SyncError::Crypto(format!("invalid envelope kek_hex: {err}")))?;
        if bytes.len() != KEY_SIZE {
            return Err(SyncError::InvalidArg(format!(
                "envelope kek_hex must decode to {KEY_SIZE} bytes"
            )));
        }
        let mut kek = [0u8; KEY_SIZE];
        kek.copy_from_slice(&bytes);
        Ok(Self {
            kek_material: KekMaterial::Raw(kek),
            kek_id: normalized_kek_id(kek_id),
            kek_version: kek_version.unwrap_or(1),
            kdf_params: EnvelopeKdfParams::default(),
        })
    }

    fn derive_or_copy_kek(
        &self,
        header: Option<&EnvelopeHeader>,
    ) -> Result<([u8; KEY_SIZE], Option<Vec<u8>>)> {
        match &self.kek_material {
            KekMaterial::Raw(kek) => Ok((*kek, None)),
            KekMaterial::Passphrase(passphrase) => {
                let (salt, params) = if let Some(header) = header {
                    let kdf = header.kdf.as_ref().ok_or_else(|| {
                        SyncError::Crypto("envelope header is missing kdf parameters".to_string())
                    })?;
                    (
                        hex::decode(&kdf.salt_hex).map_err(|err| {
                            SyncError::Crypto(format!("invalid envelope kdf salt: {err}"))
                        })?,
                        EnvelopeKdfParams {
                            memory_cost: kdf.memory_cost,
                            time_cost: kdf.time_cost,
                            parallelism: kdf.parallelism,
                        },
                    )
                } else {
                    let mut salt = vec![0u8; SALT_SIZE];
                    OsRng.fill_bytes(&mut salt);
                    (salt, self.kdf_params.clone())
                };
                let mut kek = [0u8; KEY_SIZE];
                derive_argon2id(passphrase.as_bytes(), &salt, &params, &mut kek)?;
                Ok((kek, Some(salt)))
            }
        }
    }

    pub fn rewrap_ciphertext(
        &self,
        new_crypto: &EnvelopeCrypto,
        ciphertext: &[u8],
    ) -> Result<Vec<u8>> {
        let (header, header_end) = parse_envelope(ciphertext)?;
        let mut old_kek = self.derive_or_copy_kek(Some(&header))?.0;
        let old_edek_nonce = fixed_nonce_from_hex(&header.edek_nonce_hex, "edek nonce")?;
        let old_edek = hex::decode(&header.edek_hex)
            .map_err(|err| SyncError::Crypto(format!("invalid envelope edek: {err}")))?;
        let old_kek_cipher = Aes256Gcm::new_from_slice(&old_kek)
            .map_err(|_| SyncError::Crypto("invalid envelope kek".to_string()))?;
        let mut dek = old_kek_cipher
            .decrypt(Nonce::from_slice(&old_edek_nonce), old_edek.as_slice())
            .map_err(|_| SyncError::Crypto("envelope dek unwrap failed".to_string()))?;
        old_kek.zeroize();
        if dek.len() != KEY_SIZE {
            dek.zeroize();
            return Err(SyncError::Crypto(
                "envelope dek has invalid length".to_string(),
            ));
        }

        let mut new_edek_nonce = [0u8; NONCE_SIZE];
        OsRng.fill_bytes(&mut new_edek_nonce);
        let (mut new_kek, salt) = new_crypto.derive_or_copy_kek(None)?;
        let new_kek_cipher = Aes256Gcm::new_from_slice(&new_kek)
            .map_err(|_| SyncError::Crypto("invalid envelope kek".to_string()))?;
        let new_edek = new_kek_cipher
            .encrypt(Nonce::from_slice(&new_edek_nonce), dek.as_slice())
            .map_err(|_| SyncError::Crypto("envelope dek wrapping failed".to_string()))?;
        dek.zeroize();
        new_kek.zeroize();

        let (mode, kdf) = match (&new_crypto.kek_material, salt) {
            (KekMaterial::Raw(_), _) => (EnvelopeMode::RawKek, None),
            (KekMaterial::Passphrase(_), Some(salt)) => (
                EnvelopeMode::Argon2id,
                Some(EnvelopeKdfHeader {
                    salt_hex: hex::encode(salt),
                    memory_cost: new_crypto.kdf_params.memory_cost,
                    time_cost: new_crypto.kdf_params.time_cost,
                    parallelism: new_crypto.kdf_params.parallelism,
                }),
            ),
            (KekMaterial::Passphrase(_), None) => {
                return Err(SyncError::Crypto(
                    "envelope passphrase rewrap missing salt".to_string(),
                ));
            }
        };
        let new_header = EnvelopeHeader {
            version: 1,
            kek_id: new_crypto.kek_id.clone(),
            kek_version: new_crypto.kek_version,
            mode,
            kdf,
            edek_nonce_hex: hex::encode(new_edek_nonce),
            edek_hex: hex::encode(new_edek),
            data_nonce_hex: header.data_nonce_hex,
        };
        encode_envelope(&new_header, &ciphertext[header_end..])
    }
}

impl CryptoProvider for EnvelopeCrypto {
    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let mut dek = [0u8; KEY_SIZE];
        let mut data_nonce = [0u8; NONCE_SIZE];
        let mut edek_nonce = [0u8; NONCE_SIZE];
        OsRng.fill_bytes(&mut dek);
        OsRng.fill_bytes(&mut data_nonce);
        OsRng.fill_bytes(&mut edek_nonce);

        let (mut kek, salt) = self.derive_or_copy_kek(None)?;
        let data_cipher = Aes256Gcm::new_from_slice(&dek)
            .map_err(|_| SyncError::Crypto("invalid envelope dek".to_string()))?;
        let ciphertext = data_cipher
            .encrypt(Nonce::from_slice(&data_nonce), plaintext)
            .map_err(|_| SyncError::Crypto("envelope data encryption failed".to_string()))?;

        let kek_cipher = Aes256Gcm::new_from_slice(&kek)
            .map_err(|_| SyncError::Crypto("invalid envelope kek".to_string()))?;
        let edek = kek_cipher
            .encrypt(Nonce::from_slice(&edek_nonce), dek.as_slice())
            .map_err(|_| SyncError::Crypto("envelope dek wrapping failed".to_string()))?;
        dek.zeroize();
        kek.zeroize();

        let (mode, kdf) = match (&self.kek_material, salt) {
            (KekMaterial::Raw(_), _) => (EnvelopeMode::RawKek, None),
            (KekMaterial::Passphrase(_), Some(salt)) => (
                EnvelopeMode::Argon2id,
                Some(EnvelopeKdfHeader {
                    salt_hex: hex::encode(salt),
                    memory_cost: self.kdf_params.memory_cost,
                    time_cost: self.kdf_params.time_cost,
                    parallelism: self.kdf_params.parallelism,
                }),
            ),
            (KekMaterial::Passphrase(_), None) => {
                return Err(SyncError::Crypto(
                    "envelope passphrase encryption missing salt".to_string(),
                ));
            }
        };
        let header = EnvelopeHeader {
            version: 1,
            kek_id: self.kek_id.clone(),
            kek_version: self.kek_version,
            mode,
            kdf,
            edek_nonce_hex: hex::encode(edek_nonce),
            edek_hex: hex::encode(edek),
            data_nonce_hex: hex::encode(data_nonce),
        };
        let header_json = serde_json::to_vec(&header)?;
        let header_len = u32::try_from(header_json.len())
            .map_err(|_| SyncError::Crypto("envelope header is too large to encode".to_string()))?;

        let mut output = Vec::with_capacity(
            ENVELOPE_MAGIC.len() + HEADER_LEN_SIZE + header_json.len() + ciphertext.len(),
        );
        output.extend_from_slice(ENVELOPE_MAGIC);
        output.extend_from_slice(&header_len.to_be_bytes());
        output.extend_from_slice(&header_json);
        output.extend_from_slice(&ciphertext);
        Ok(output)
    }

    fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        let (header, header_end) = parse_envelope(ciphertext)?;

        let mut kek = self.derive_or_copy_kek(Some(&header))?.0;
        let edek_nonce = fixed_nonce_from_hex(&header.edek_nonce_hex, "edek nonce")?;
        let data_nonce = fixed_nonce_from_hex(&header.data_nonce_hex, "data nonce")?;
        let edek = hex::decode(&header.edek_hex)
            .map_err(|err| SyncError::Crypto(format!("invalid envelope edek: {err}")))?;

        let kek_cipher = Aes256Gcm::new_from_slice(&kek)
            .map_err(|_| SyncError::Crypto("invalid envelope kek".to_string()))?;
        let mut dek = kek_cipher
            .decrypt(Nonce::from_slice(&edek_nonce), edek.as_slice())
            .map_err(|_| SyncError::Crypto("envelope dek unwrap failed".to_string()))?;
        kek.zeroize();
        if dek.len() != KEY_SIZE {
            dek.zeroize();
            return Err(SyncError::Crypto(
                "envelope dek has invalid length".to_string(),
            ));
        }

        let data_cipher = Aes256Gcm::new_from_slice(&dek)
            .map_err(|_| SyncError::Crypto("invalid envelope dek".to_string()))?;
        let plaintext = data_cipher
            .decrypt(Nonce::from_slice(&data_nonce), &ciphertext[header_end..])
            .map_err(|_| SyncError::Crypto("envelope data decryption failed".to_string()))?;
        dek.zeroize();
        Ok(plaintext)
    }

    fn is_encrypted(&self) -> bool {
        true
    }

    fn uses_envelope_encryption(&self) -> bool {
        true
    }

    fn remote_extension(&self, original_ext: &str) -> String {
        format!("{original_ext}.env")
    }
}

fn normalized_kek_id(kek_id: Option<String>) -> String {
    let kek_id = kek_id.unwrap_or_else(|| "default".to_string());
    if kek_id.trim().is_empty() {
        "default".to_string()
    } else {
        kek_id
    }
}

fn parse_envelope(ciphertext: &[u8]) -> Result<(EnvelopeHeader, usize)> {
    if ciphertext.len() < ENVELOPE_MAGIC.len() + HEADER_LEN_SIZE {
        return Err(SyncError::Crypto(
            "envelope ciphertext is too short".to_string(),
        ));
    }
    if &ciphertext[..ENVELOPE_MAGIC.len()] != ENVELOPE_MAGIC {
        return Err(SyncError::Crypto(
            "invalid envelope ciphertext magic".to_string(),
        ));
    }
    let header_len_start = ENVELOPE_MAGIC.len();
    let header_len_end = header_len_start + HEADER_LEN_SIZE;
    let header_len = u32::from_be_bytes(
        ciphertext[header_len_start..header_len_end]
            .try_into()
            .map_err(|_| SyncError::Crypto("invalid envelope header length".to_string()))?,
    ) as usize;
    let header_end = header_len_end
        .checked_add(header_len)
        .ok_or_else(|| SyncError::Crypto("envelope header length overflow".to_string()))?;
    if ciphertext.len() < header_end {
        return Err(SyncError::Crypto(
            "envelope header length exceeds ciphertext".to_string(),
        ));
    }

    let header: EnvelopeHeader = serde_json::from_slice(&ciphertext[header_len_end..header_end])?;
    if header.version != 1 {
        return Err(SyncError::Crypto(format!(
            "unsupported envelope version {}",
            header.version
        )));
    }
    Ok((header, header_end))
}

fn encode_envelope(header: &EnvelopeHeader, encrypted_body: &[u8]) -> Result<Vec<u8>> {
    let header_json = serde_json::to_vec(header)?;
    let header_len = u32::try_from(header_json.len())
        .map_err(|_| SyncError::Crypto("envelope header is too large to encode".to_string()))?;
    let mut output = Vec::with_capacity(
        ENVELOPE_MAGIC.len() + HEADER_LEN_SIZE + header_json.len() + encrypted_body.len(),
    );
    output.extend_from_slice(ENVELOPE_MAGIC);
    output.extend_from_slice(&header_len.to_be_bytes());
    output.extend_from_slice(&header_json);
    output.extend_from_slice(encrypted_body);
    Ok(output)
}

fn derive_argon2id(
    passphrase: &[u8],
    salt: &[u8],
    params: &EnvelopeKdfParams,
    output: &mut [u8; KEY_SIZE],
) -> Result<()> {
    let params = Params::new(
        params.memory_cost,
        params.time_cost,
        params.parallelism,
        Some(KEY_SIZE),
    )
    .map_err(|err| SyncError::Crypto(format!("invalid envelope argon2 params: {err}")))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    argon2
        .hash_password_into(passphrase, salt, output)
        .map_err(|err| SyncError::Crypto(format!("envelope argon2 failed: {err}")))
}

fn fixed_nonce_from_hex(hex_value: &str, name: &str) -> Result<[u8; NONCE_SIZE]> {
    let bytes = hex::decode(hex_value)
        .map_err(|err| SyncError::Crypto(format!("invalid envelope {name}: {err}")))?;
    if bytes.len() != NONCE_SIZE {
        return Err(SyncError::Crypto(format!(
            "envelope {name} must decode to {NONCE_SIZE} bytes"
        )));
    }
    let mut nonce = [0u8; NONCE_SIZE];
    nonce.copy_from_slice(&bytes);
    Ok(nonce)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_kdf_params() -> EnvelopeKdfParams {
        EnvelopeKdfParams {
            memory_cost: 256,
            time_cost: 1,
            parallelism: 1,
        }
    }

    #[test]
    fn envelope_passphrase_roundtrip_and_extension() {
        let crypto =
            EnvelopeCrypto::from_passphrase("correct horse", None, None, Some(test_kdf_params()))
                .unwrap();
        let plaintext = b"kmo-sync envelope encryption";
        let encrypted = crypto.encrypt(plaintext).unwrap();

        assert_ne!(encrypted, plaintext);
        assert_eq!(crypto.decrypt(&encrypted).unwrap(), plaintext);
        assert!(crypto.is_encrypted());
        assert!(crypto.uses_envelope_encryption());
        assert_eq!(crypto.remote_extension("meta"), "meta.env");
    }

    #[test]
    fn envelope_wrong_passphrase_fails() {
        let crypto_a =
            EnvelopeCrypto::from_passphrase("pass-a", None, None, Some(test_kdf_params())).unwrap();
        let crypto_b =
            EnvelopeCrypto::from_passphrase("pass-b", None, None, Some(test_kdf_params())).unwrap();

        let encrypted = crypto_a.encrypt(b"secret").unwrap();
        assert!(crypto_b.decrypt(&encrypted).is_err());
    }

    #[test]
    fn envelope_encrypts_same_plaintext_to_different_ciphertext() {
        let crypto =
            EnvelopeCrypto::from_passphrase("correct horse", None, None, Some(test_kdf_params()))
                .unwrap();

        let first = crypto.encrypt(b"same").unwrap();
        let second = crypto.encrypt(b"same").unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn envelope_raw_kek_roundtrip() {
        let kek_hex = hex::encode([7u8; KEY_SIZE]);
        let crypto =
            EnvelopeCrypto::from_kek_hex(&kek_hex, Some("kek-a".to_string()), Some(2)).unwrap();
        let encrypted = crypto.encrypt(b"raw key secret").unwrap();
        assert_eq!(crypto.decrypt(&encrypted).unwrap(), b"raw key secret");
    }

    #[test]
    fn envelope_rewrap_changes_kek_without_reencrypting_body() {
        let old_crypto =
            EnvelopeCrypto::from_passphrase("old-pass", None, Some(1), Some(test_kdf_params()))
                .unwrap();
        let new_crypto =
            EnvelopeCrypto::from_passphrase("new-pass", None, Some(2), Some(test_kdf_params()))
                .unwrap();
        let encrypted = old_crypto.encrypt(b"rotate me").unwrap();
        let old_body = encrypted[parse_envelope(&encrypted).unwrap().1..].to_vec();

        let rewrapped = old_crypto
            .rewrap_ciphertext(&new_crypto, &encrypted)
            .unwrap();
        let new_body = rewrapped[parse_envelope(&rewrapped).unwrap().1..].to_vec();

        assert_eq!(old_body, new_body);
        assert!(old_crypto.decrypt(&rewrapped).is_err());
        assert_eq!(new_crypto.decrypt(&rewrapped).unwrap(), b"rotate me");
    }
}
