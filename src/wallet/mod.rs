use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use rand::rngs::OsRng;
use rand::RngCore;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use solana_sdk::signature::{read_keypair_file, Keypair};
use solana_sdk::signer::Signer;
use zeroize::{Zeroize, Zeroizing};

use crate::util::fs_utils::atomic_write;

const KEYSTORE_VERSION: u8 = 1;
const KEYSTORE_AAD: &[u8] = b"lasersell-keystore-v1";
const ARGON2_M_KIB: u32 = 65_536;
const ARGON2_T: u32 = 3;
const ARGON2_P: u32 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalletFileKind {
    EncryptedKeystore,
    PlaintextSolanaJson,
}

#[derive(Debug, Serialize, Deserialize)]
struct KeystoreV1 {
    version: u8,
    pubkey: String,
    kdf: KdfSpec,
    cipher: CipherSpec,
}

#[derive(Debug, Serialize, Deserialize)]
struct KdfSpec {
    name: String,
    params: Argon2Params,
    salt_b64: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Argon2Params {
    m_kib: u32,
    t: u32,
    p: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct CipherSpec {
    name: String,
    nonce_b64: String,
    ciphertext_b64: String,
}

pub fn detect_wallet_file_kind(path: &Path) -> Result<WalletFileKind> {
    let raw = fs::read(path).with_context(|| format!("read wallet file {}", path.display()))?;
    let json: serde_json::Value = serde_json::from_slice(&raw)
        .with_context(|| format!("parse wallet json {}", path.display()))?;
    if json.get("version").is_some() {
        return Ok(WalletFileKind::EncryptedKeystore);
    }
    if let Some(array) = json.as_array() {
        let is_keypair = array.len() == 64
            && array
                .iter()
                .all(|v| v.as_u64().is_some_and(|n| n <= u8::MAX as u64));
        if is_keypair {
            return Ok(WalletFileKind::PlaintextSolanaJson);
        }
    }
    Err(anyhow!(
        "unrecognized wallet file format {}",
        path.display()
    ))
}

pub fn load_keypair_from_path(
    path: &Path,
    mut passphrase_provider: impl FnMut() -> Result<SecretString>,
) -> Result<Keypair> {
    match detect_wallet_file_kind(path)? {
        WalletFileKind::EncryptedKeystore => {
            let passphrase = passphrase_provider()?;
            load_keystore_keypair(path, passphrase)
        }
        WalletFileKind::PlaintextSolanaJson => read_keypair_file(path)
            .map_err(|err| anyhow!("failed to read keypair {}: {err}", path.display())),
    }
}

pub fn write_keystore(path: &Path, keypair: &Keypair, passphrase: &SecretString) -> Result<()> {
    let mut salt = [0u8; SALT_LEN];
    OsRng.fill_bytes(&mut salt);
    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);

    let params = Params::new(ARGON2_M_KIB, ARGON2_T, ARGON2_P, Some(32))
        .map_err(|err| anyhow!("invalid argon2 params: {err}"))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = Zeroizing::new([0u8; 32]);
    argon2
        .hash_password_into(passphrase.expose_secret().as_bytes(), &salt, key.as_mut())
        .map_err(|err| anyhow!("argon2 key derivation failed: {err}"))?;

    let cipher = XChaCha20Poly1305::new(Key::from_slice(key.as_ref()));
    let plaintext = Zeroizing::new(keypair.to_bytes());
    let ciphertext = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plaintext.as_ref(),
                aad: KEYSTORE_AAD,
            },
        )
        .map_err(|err| anyhow!("keystore encryption failed: {err}"))?;

    let keystore = KeystoreV1 {
        version: KEYSTORE_VERSION,
        pubkey: keypair.pubkey().to_string(),
        kdf: KdfSpec {
            name: "argon2id".to_string(),
            params: Argon2Params {
                m_kib: ARGON2_M_KIB,
                t: ARGON2_T,
                p: ARGON2_P,
            },
            salt_b64: STANDARD.encode(salt),
        },
        cipher: CipherSpec {
            name: "xchacha20poly1305".to_string(),
            nonce_b64: STANDARD.encode(nonce),
            ciphertext_b64: STANDARD.encode(ciphertext),
        },
    };
    let raw = serde_json::to_vec_pretty(&keystore).context("serialize keystore")?;
    atomic_write(path, &raw, Some(0o600))
        .with_context(|| format!("write keystore {}", path.display()))?;
    Ok(())
}

pub fn migrate_plaintext_to_keystore(
    plaintext_path: &Path,
    keystore_path: &Path,
    passphrase: SecretString,
    mut update_config_fn: impl FnMut(&Path) -> Result<()>,
) -> Result<()> {
    let keypair = read_keypair_file(plaintext_path)
        .map_err(|err| anyhow!("failed to read keypair {}: {err}", plaintext_path.display()))?;
    write_keystore(keystore_path, &keypair, &passphrase)?;
    update_config_fn(keystore_path)?;
    Ok(())
}

pub fn default_keystore_path(path: &Path) -> PathBuf {
    let mut new_path = path.to_path_buf();
    new_path.set_extension("keystore.json");
    new_path
}

fn load_keystore_keypair(path: &Path, passphrase: SecretString) -> Result<Keypair> {
    let raw = fs::read(path).with_context(|| format!("read keystore {}", path.display()))?;
    let keystore: KeystoreV1 = serde_json::from_slice(&raw)
        .with_context(|| format!("parse keystore {}", path.display()))?;
    if keystore.version != KEYSTORE_VERSION {
        return Err(anyhow!("unsupported keystore version {}", keystore.version));
    }
    if keystore.kdf.name != "argon2id" {
        return Err(anyhow!("unsupported kdf {}", keystore.kdf.name));
    }
    if keystore.cipher.name != "xchacha20poly1305" {
        return Err(anyhow!("unsupported cipher {}", keystore.cipher.name));
    }

    let salt = STANDARD
        .decode(&keystore.kdf.salt_b64)
        .context("decode kdf salt")?;
    if salt.len() < SALT_LEN {
        return Err(anyhow!("invalid kdf salt length"));
    }
    let nonce = STANDARD
        .decode(&keystore.cipher.nonce_b64)
        .context("decode cipher nonce")?;
    if nonce.len() != NONCE_LEN {
        return Err(anyhow!("invalid cipher nonce length"));
    }
    let ciphertext = STANDARD
        .decode(&keystore.cipher.ciphertext_b64)
        .context("decode ciphertext")?;

    let params = Params::new(
        keystore.kdf.params.m_kib,
        keystore.kdf.params.t,
        keystore.kdf.params.p,
        Some(32),
    )
    .map_err(|err| anyhow!("invalid argon2 params: {err}"))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = Zeroizing::new([0u8; 32]);
    argon2
        .hash_password_into(passphrase.expose_secret().as_bytes(), &salt, key.as_mut())
        .map_err(|err| anyhow!("argon2 key derivation failed: {err}"))?;

    let cipher = XChaCha20Poly1305::new(Key::from_slice(key.as_ref()));
    let plaintext = cipher
        .decrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: &ciphertext,
                aad: KEYSTORE_AAD,
            },
        )
        .map_err(|_| anyhow!("invalid passphrase or corrupted keystore"))?;
    let mut plaintext = Zeroizing::new(plaintext);
    if plaintext.len() != 64 {
        return Err(anyhow!("invalid decrypted key length"));
    }
    let keypair = Keypair::try_from(plaintext.as_ref())
        .map_err(|err| anyhow!("invalid keypair bytes: {err}"))?;
    if keypair.pubkey().to_string() != keystore.pubkey {
        return Err(anyhow!("keystore pubkey mismatch"));
    }
    plaintext.zeroize();
    Ok(keypair)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn passphrase(value: &str) -> SecretString {
        SecretString::new(value.to_string())
    }

    #[test]
    fn keystore_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("wallet.keystore.json");
        let keypair = Keypair::new();
        let passphrase_secret = passphrase("correct horse");
        write_keystore(&path, &keypair, &passphrase_secret).unwrap();
        let loaded = load_keypair_from_path(&path, || Ok(passphrase("correct horse"))).unwrap();
        assert_eq!(keypair.pubkey(), loaded.pubkey());
    }

    #[test]
    fn keystore_base58_export_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("wallet.keystore.json");
        let keypair = Keypair::new();
        let passphrase_secret = passphrase("correct horse");
        write_keystore(&path, &keypair, &passphrase_secret).unwrap();
        let loaded = load_keypair_from_path(&path, || Ok(passphrase("correct horse"))).unwrap();

        let bytes = Zeroizing::new(loaded.to_bytes());
        let encoded = bs58::encode(bytes.as_ref()).into_string();
        let decoded = Zeroizing::new(bs58::decode(encoded).into_vec().unwrap());
        let roundtrip = Keypair::try_from(decoded.as_slice()).unwrap();
        assert_eq!(loaded.pubkey(), roundtrip.pubkey());
    }

    #[test]
    fn keystore_wrong_passphrase_fails() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("wallet.keystore.json");
        let keypair = Keypair::new();
        let passphrase_secret = passphrase("correct horse");
        write_keystore(&path, &keypair, &passphrase_secret).unwrap();
        let err = load_keypair_from_path(&path, || Ok(passphrase("tr0ub4dor"))).unwrap_err();
        assert!(err.to_string().contains("invalid passphrase"));
    }

    #[test]
    fn keystore_tamper_detected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("wallet.keystore.json");
        let keypair = Keypair::new();
        let passphrase_secret = passphrase("correct horse");
        write_keystore(&path, &keypair, &passphrase_secret).unwrap();

        let mut keystore: KeystoreV1 = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let mut ciphertext = STANDARD.decode(&keystore.cipher.ciphertext_b64).unwrap();
        ciphertext[0] ^= 0x80;
        keystore.cipher.ciphertext_b64 = STANDARD.encode(ciphertext);
        fs::write(&path, serde_json::to_vec_pretty(&keystore).unwrap()).unwrap();

        let err = load_keypair_from_path(&path, || Ok(passphrase("correct horse"))).unwrap_err();
        assert!(err.to_string().contains("invalid passphrase"));
    }
}
