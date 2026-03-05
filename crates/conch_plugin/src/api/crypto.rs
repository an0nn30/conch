//! Rust-backed `crypto` table for Lua plugins.
//!
//! Provides AES encryption/decryption (CBC, GCM, ECB) with PBKDF2 key
//! derivation — matching the Java Conch's Groovy plugin crypto surface.

use aes::Aes128;
use aes::Aes256;
use aes_gcm::aead::Aead;
use aes_gcm::{Aes128Gcm, Aes256Gcm, KeyInit as GcmKeyInit, Nonce as GcmNonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use mlua::{Lua, Result as LuaResult};
use rand::RngCore;

const SALT_LEN: usize = 16;
const GCM_IV_LEN: usize = 12;
const CBC_IV_LEN: usize = 16;
const PBKDF2_ITERS: u32 = 310_000;

type Aes128CbcEnc = cbc::Encryptor<Aes128>;
type Aes128CbcDec = cbc::Decryptor<Aes128>;
type Aes256CbcEnc = cbc::Encryptor<Aes256>;
type Aes256CbcDec = cbc::Decryptor<Aes256>;

type Aes128EcbEnc = ecb::Encryptor<Aes128>;
type Aes128EcbDec = ecb::Decryptor<Aes128>;
type Aes256EcbEnc = ecb::Encryptor<Aes256>;
type Aes256EcbDec = ecb::Decryptor<Aes256>;

/// Parsed algorithm descriptor.
struct Algo {
    key_bits: usize,
    mode: Mode,
}

enum Mode {
    Cbc,
    Gcm,
    Ecb,
}

fn parse_algorithm(s: &str) -> Result<Algo, String> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 3 || parts[0] != "AES" {
        return Err(format!("Unknown algorithm: '{s}'. Expected AES-<bits>-<mode>"));
    }
    let key_bits: usize = parts[1]
        .parse()
        .map_err(|_| format!("Invalid key size: '{}'", parts[1]))?;
    if key_bits != 128 && key_bits != 256 {
        return Err(format!("Unsupported key size: {key_bits}. Use 128 or 256"));
    }
    let mode = match parts[2] {
        "CBC" => Mode::Cbc,
        "GCM" => Mode::Gcm,
        "ECB" => Mode::Ecb,
        other => return Err(format!("Unknown mode: '{other}'. Use CBC, GCM, or ECB")),
    };
    Ok(Algo { key_bits, mode })
}

/// Derive a key from passphrase + salt using PBKDF2-HMAC-SHA256.
fn derive_key(passphrase: &[u8], salt: &[u8], key_bytes: usize) -> Vec<u8> {
    let mut key = vec![0u8; key_bytes];
    pbkdf2::pbkdf2_hmac::<sha2::Sha256>(passphrase, salt, PBKDF2_ITERS, &mut key);
    key
}

/// PKCS7 pad in-place, returning the padded buffer.
fn pkcs7_pad(data: &[u8], block_size: usize) -> Vec<u8> {
    let pad_len = block_size - (data.len() % block_size);
    let mut buf = data.to_vec();
    buf.extend(std::iter::repeat(pad_len as u8).take(pad_len));
    buf
}

/// PKCS7 unpad, returning the unpadded slice length.
fn pkcs7_unpad(data: &[u8]) -> Result<&[u8], String> {
    if data.is_empty() {
        return Err("Decryption failed (wrong passphrase or corrupted data)".into());
    }
    let pad_len = *data.last().unwrap() as usize;
    if pad_len == 0 || pad_len > 16 || pad_len > data.len() {
        return Err("Decryption failed (wrong passphrase or corrupted data)".into());
    }
    if !data[data.len() - pad_len..].iter().all(|&b| b == pad_len as u8) {
        return Err("Decryption failed (wrong passphrase or corrupted data)".into());
    }
    Ok(&data[..data.len() - pad_len])
}

fn ecb_encrypt(key: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let padded = pkcs7_pad(plaintext, 16);
    let len = padded.len();
    let mut buf = padded;
    match key.len() {
        16 => {
            let enc = Aes128EcbEnc::new_from_slice(key).map_err(|e| e.to_string())?;
            enc.encrypt_padded_mut::<ecb::cipher::block_padding::NoPadding>(&mut buf, len)
                .map_err(|e| e.to_string())?;
        }
        32 => {
            let enc = Aes256EcbEnc::new_from_slice(key).map_err(|e| e.to_string())?;
            enc.encrypt_padded_mut::<ecb::cipher::block_padding::NoPadding>(&mut buf, len)
                .map_err(|e| e.to_string())?;
        }
        _ => return Err("Unsupported key size for ECB".into()),
    }
    Ok(buf)
}

fn ecb_decrypt(key: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, String> {
    let mut buf = ciphertext.to_vec();
    match key.len() {
        16 => {
            let dec = Aes128EcbDec::new_from_slice(key).map_err(|e| e.to_string())?;
            dec.decrypt_padded_mut::<ecb::cipher::block_padding::NoPadding>(&mut buf)
                .map_err(|_| "Decryption failed (wrong passphrase or corrupted data)".to_string())?;
        }
        32 => {
            let dec = Aes256EcbDec::new_from_slice(key).map_err(|e| e.to_string())?;
            dec.decrypt_padded_mut::<ecb::cipher::block_padding::NoPadding>(&mut buf)
                .map_err(|_| "Decryption failed (wrong passphrase or corrupted data)".to_string())?;
        }
        _ => return Err("Unsupported key size for ECB".into()),
    }
    let unpadded = pkcs7_unpad(&buf)?;
    Ok(unpadded.to_vec())
}

fn encrypt_impl(plaintext: &str, passphrase: &str, algorithm: &str) -> Result<String, String> {
    let algo = parse_algorithm(algorithm)?;
    let key_bytes = algo.key_bits / 8;

    let mut rng = rand::thread_rng();
    let mut salt = [0u8; SALT_LEN];
    rng.fill_bytes(&mut salt);

    let key = derive_key(passphrase.as_bytes(), &salt, key_bytes);
    let pt = plaintext.as_bytes();

    let (iv, ciphertext): (Vec<u8>, Vec<u8>) = match algo.mode {
        Mode::Gcm => {
            let mut iv = [0u8; GCM_IV_LEN];
            rng.fill_bytes(&mut iv);
            let nonce = GcmNonce::from_slice(&iv);
            let ct = match key_bytes {
                16 => {
                    let cipher = Aes128Gcm::new_from_slice(&key).map_err(|e| e.to_string())?;
                    cipher.encrypt(nonce, pt).map_err(|e| e.to_string())?
                }
                32 => {
                    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| e.to_string())?;
                    cipher.encrypt(nonce, pt).map_err(|e| e.to_string())?
                }
                _ => return Err("Unsupported key size for GCM".into()),
            };
            (iv.to_vec(), ct)
        }
        Mode::Cbc => {
            let mut iv = [0u8; CBC_IV_LEN];
            rng.fill_bytes(&mut iv);
            let ct = match key_bytes {
                16 => Aes128CbcEnc::new_from_slices(&key, &iv)
                    .map_err(|e| e.to_string())?
                    .encrypt_padded_vec_mut::<cbc::cipher::block_padding::Pkcs7>(pt),
                32 => Aes256CbcEnc::new_from_slices(&key, &iv)
                    .map_err(|e| e.to_string())?
                    .encrypt_padded_vec_mut::<cbc::cipher::block_padding::Pkcs7>(pt),
                _ => return Err("Unsupported key size for CBC".into()),
            };
            (iv.to_vec(), ct)
        }
        Mode::Ecb => {
            let ct = ecb_encrypt(&key, pt)?;
            (Vec::new(), ct)
        }
    };

    // Pack: salt || iv || ciphertext -> base64
    let mut buf = Vec::with_capacity(SALT_LEN + iv.len() + ciphertext.len());
    buf.extend_from_slice(&salt);
    buf.extend_from_slice(&iv);
    buf.extend_from_slice(&ciphertext);
    Ok(B64.encode(&buf))
}

fn decrypt_impl(encoded: &str, passphrase: &str, algorithm: &str) -> Result<String, String> {
    let algo = parse_algorithm(algorithm)?;
    let key_bytes = algo.key_bits / 8;

    let raw = B64
        .decode(encoded.trim())
        .map_err(|e| format!("Base64 decode failed: {e}"))?;

    let iv_len = match algo.mode {
        Mode::Gcm => GCM_IV_LEN,
        Mode::Cbc => CBC_IV_LEN,
        Mode::Ecb => 0,
    };

    if raw.len() < SALT_LEN + iv_len + 1 {
        return Err("Input too short to be valid ciphertext".into());
    }

    let salt = &raw[..SALT_LEN];
    let iv = &raw[SALT_LEN..SALT_LEN + iv_len];
    let ct = &raw[SALT_LEN + iv_len..];

    let key = derive_key(passphrase.as_bytes(), salt, key_bytes);

    let plaintext_bytes: Vec<u8> = match algo.mode {
        Mode::Gcm => {
            let nonce = GcmNonce::from_slice(iv);
            match key_bytes {
                16 => {
                    let cipher = Aes128Gcm::new_from_slice(&key).map_err(|e| e.to_string())?;
                    cipher.decrypt(nonce, ct).map_err(|_| "Decryption failed (wrong passphrase or corrupted data)".to_string())?
                }
                32 => {
                    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| e.to_string())?;
                    cipher.decrypt(nonce, ct).map_err(|_| "Decryption failed (wrong passphrase or corrupted data)".to_string())?
                }
                _ => return Err("Unsupported key size for GCM".into()),
            }
        }
        Mode::Cbc => {
            match key_bytes {
                16 => Aes128CbcDec::new_from_slices(&key, iv)
                    .map_err(|e| e.to_string())?
                    .decrypt_padded_vec_mut::<cbc::cipher::block_padding::Pkcs7>(ct)
                    .map_err(|_| "Decryption failed (wrong passphrase or corrupted data)".to_string())?,
                32 => Aes256CbcDec::new_from_slices(&key, iv)
                    .map_err(|e| e.to_string())?
                    .decrypt_padded_vec_mut::<cbc::cipher::block_padding::Pkcs7>(ct)
                    .map_err(|_| "Decryption failed (wrong passphrase or corrupted data)".to_string())?,
                _ => return Err("Unsupported key size for CBC".into()),
            }
        }
        Mode::Ecb => ecb_decrypt(&key, ct)?,
    };

    String::from_utf8(plaintext_bytes).map_err(|e| format!("Decrypted data is not valid UTF-8: {e}"))
}

/// Register the `crypto` table into the Lua state.
pub fn register(lua: &Lua) -> LuaResult<()> {
    let crypto = lua.create_table()?;

    // crypto.encrypt(plaintext, passphrase, algorithm) -> base64 string
    crypto.set(
        "encrypt",
        lua.create_async_function(|_lua, (plaintext, passphrase, algorithm): (String, String, String)| async move {
            let t0 = std::time::Instant::now();
            let result = tokio::task::spawn_blocking(move || {
                encrypt_impl(&plaintext, &passphrase, &algorithm)
            })
            .await
            .map_err(|e| mlua::Error::runtime(e.to_string()))?
            .map_err(mlua::Error::runtime);
            eprintln!("[crypto] encrypt took {:?}", t0.elapsed());
            result
        })?,
    )?;

    // crypto.decrypt(encoded, passphrase, algorithm) -> plaintext string
    crypto.set(
        "decrypt",
        lua.create_async_function(|_lua, (encoded, passphrase, algorithm): (String, String, String)| async move {
            let t0 = std::time::Instant::now();
            let result = tokio::task::spawn_blocking(move || {
                decrypt_impl(&encoded, &passphrase, &algorithm)
            })
            .await
            .map_err(|e| mlua::Error::runtime(e.to_string()))?
            .map_err(mlua::Error::runtime);
            eprintln!("[crypto] decrypt took {:?}", t0.elapsed());
            result
        })?,
    )?;

    // crypto.algorithms() -> list of supported algorithm strings
    crypto.set(
        "algorithms",
        lua.create_function(|lua, ()| {
            let t = lua.create_table()?;
            let algos = [
                "AES-128-CBC", "AES-128-GCM", "AES-128-ECB",
                "AES-256-CBC", "AES-256-GCM", "AES-256-ECB",
            ];
            for (i, a) in algos.iter().enumerate() {
                t.set(i + 1, *a)?;
            }
            Ok(t)
        })?,
    )?;

    lua.globals().set("crypto", crypto)?;
    Ok(())
}
