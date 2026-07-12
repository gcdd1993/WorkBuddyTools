use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use pbkdf2::pbkdf2_hmac;
use rand::{rngs::OsRng, RngCore};
use sha2::Sha256;
use std::{fs, path::Path};

const MAGIC: &[u8; 8] = b"WBZIP01\0";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;
const PBKDF2_ROUNDS: u32 = 120_000;

pub fn encrypt_package(
    plain_zip_path: &Path,
    encrypted_path: &Path,
    passphrase: &str,
) -> Result<(), String> {
    if passphrase.trim().is_empty() {
        return Err("同步加密密码不能为空".to_string());
    }

    let plain = fs::read(plain_zip_path).map_err(|err| format!("读取 ZIP 包失败：{err}"))?;
    let mut salt = [0u8; SALT_LEN];
    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut nonce_bytes);

    let key = derive_key(passphrase, &salt);
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|err| format!("创建加密器失败：{err}"))?;
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), plain.as_ref())
        .map_err(|err| format!("加密 ZIP 包失败：{err}"))?;

    let mut output = Vec::with_capacity(MAGIC.len() + SALT_LEN + NONCE_LEN + ciphertext.len());
    output.extend_from_slice(MAGIC);
    output.extend_from_slice(&salt);
    output.extend_from_slice(&nonce_bytes);
    output.extend_from_slice(&ciphertext);

    if let Some(parent) = encrypted_path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("创建加密包目录失败：{err}"))?;
    }
    fs::write(encrypted_path, output).map_err(|err| format!("写入加密 ZIP 包失败：{err}"))
}

pub fn decrypt_package(
    encrypted_path: &Path,
    plain_zip_path: &Path,
    passphrase: &str,
) -> Result<(), String> {
    if passphrase.trim().is_empty() {
        return Err("同步加密密码不能为空".to_string());
    }

    let encrypted =
        fs::read(encrypted_path).map_err(|err| format!("读取加密 ZIP 包失败：{err}"))?;
    let min_len = MAGIC.len() + SALT_LEN + NONCE_LEN;
    if encrypted.len() <= min_len || &encrypted[..MAGIC.len()] != MAGIC {
        return Err("加密 ZIP 包格式无效".to_string());
    }

    let salt_start = MAGIC.len();
    let nonce_start = salt_start + SALT_LEN;
    let cipher_start = nonce_start + NONCE_LEN;
    let salt = &encrypted[salt_start..nonce_start];
    let nonce = &encrypted[nonce_start..cipher_start];
    let ciphertext = &encrypted[cipher_start..];

    let key = derive_key(passphrase, salt);
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|err| format!("创建解密器失败：{err}"))?;
    let plain = cipher
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|_| "解密 ZIP 包失败，请检查同步密码或远端文件是否损坏".to_string())?;

    if let Some(parent) = plain_zip_path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("创建解密目录失败：{err}"))?;
    }
    fs::write(plain_zip_path, plain).map_err(|err| format!("写入解密 ZIP 包失败：{err}"))
}

fn derive_key(passphrase: &str, salt: &[u8]) -> [u8; KEY_LEN] {
    let mut key = [0u8; KEY_LEN];
    pbkdf2_hmac::<Sha256>(passphrase.as_bytes(), salt, PBKDF2_ROUNDS, &mut key);
    key
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, time::{SystemTime, UNIX_EPOCH}};

    #[test]
    fn encrypted_package_round_trip_restores_plain_zip_bytes() {
        let root = std::env::temp_dir().join(format!(
            "workbuddy-crypto-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("create temp root");
        let plain = root.join("workbuddy-sync.zip");
        let encrypted = root.join("workbuddy-sync.zip.enc");
        let decrypted = root.join("restored.zip");
        fs::write(&plain, b"zip bytes with apiKey-like secret sk-test").expect("write plain");

        encrypt_package(&plain, &encrypted, "correct horse battery staple")
            .expect("encrypt package");
        decrypt_package(&encrypted, &decrypted, "correct horse battery staple")
            .expect("decrypt package");

        assert_ne!(fs::read(&encrypted).expect("read encrypted"), fs::read(&plain).expect("read plain"));
        assert_eq!(fs::read(&decrypted).expect("read decrypted"), b"zip bytes with apiKey-like secret sk-test");
        fs::remove_dir_all(root).ok();
    }
}
