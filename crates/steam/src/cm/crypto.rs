//! Chiffrement du canal CM : RSA-OAEP/SHA-1 et AES-256-CBC.
//!
//! Chaque message est enveloppé dans un IV chiffré en AES-ECB, suivi du
//! message chiffré en AES-CBC/PKCS#7. Quand le CM impose un nonce, l'IV porte
//! aussi un HMAC-SHA1 tronqué : 13 octets de HMAC puis 3 octets aléatoires,
//! le HMAC couvrant ces 3 octets et le message. C'est le mode utilisé par les
//! CM actuels ; sans nonce, le canal historique s'en passe.

use super::Error;
use aes::cipher::{
    block_padding::Pkcs7, generic_array::GenericArray, BlockDecrypt, BlockDecryptMut, BlockEncrypt,
    BlockEncryptMut, KeyInit, KeyIvInit,
};
use aes::Aes256;
use cbc::{Decryptor, Encryptor};
use rand_core::{OsRng, RngCore};
use rsa::{BigUint, Oaep, RsaPublicKey};
use sha1::{Digest, Sha1};

/// Exposant de la clé de l'univers Public. Valve utilise 17, pas 65537 :
/// chiffrer avec le mauvais exposant produit un cryptogramme que le CM rejette.
const PUBLIC_EXPONENT: u64 = 17;

/// Module RSA de l'univers Public (clé présente dans tous les clients Steam).
const PUBLIC_MODULUS: &str = concat!(
    "dfec1ad62c10662c17353a14b07c59117f9dd3d82b7ae3e015cd191e46e87b87",
    "74a2184631a9031479828ee945a24912a923687389cf69a1b16146bdc1bebfd6",
    "011bd881d4dc90fbfe4f527366cb9570d7c58eba1c7a3375a1623446bb60b780",
    "68fa13a77a8a374b9ec6f45d5f3a99f99ec43ae963a2bb881928e0e714c04289",
);

fn decode_hex(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).expect("hexadécimal ASCII");
            u8::from_str_radix(text, 16).expect("hexadécimal valide")
        })
        .collect()
}

pub fn public_key() -> Result<RsaPublicKey, Error> {
    let modulus = BigUint::from_bytes_be(&decode_hex(PUBLIC_MODULUS));
    RsaPublicKey::new(modulus, BigUint::from(PUBLIC_EXPONENT))
        .map_err(|e| Error::Crypto(format!("clé publique Steam : {e}")))
}

/// Chiffre le secret de session (clé de 32 octets + nonce du CM) avec
/// RSA-OAEP/SHA-1, comme le client de Valve.
pub fn encrypt_session_key(plaintext: &[u8]) -> Result<Vec<u8>, Error> {
    public_key()?
        .encrypt(&mut OsRng, Oaep::new::<Sha1>(), plaintext)
        .map_err(|e| Error::Crypto(format!("RSA-OAEP : {e}")))
}

/// HMAC-SHA1 (RFC 2104) — le canal CM en authentifie ses IV.
fn hmac_sha1(key: &[u8], message: &[u8]) -> [u8; 20] {
    const BLOCK: usize = 64;
    let mut padded = [0u8; BLOCK];
    if key.len() > BLOCK {
        padded[..20].copy_from_slice(&Sha1::digest(key));
    } else {
        padded[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36u8; BLOCK];
    let mut outer_pad = [0x5cu8; BLOCK];
    for index in 0..BLOCK {
        inner_pad[index] ^= padded[index];
        outer_pad[index] ^= padded[index];
    }
    let mut inner = Sha1::new();
    inner.update(inner_pad);
    inner.update(message);
    let inner = inner.finalize();
    let mut outer = Sha1::new();
    outer.update(outer_pad);
    outer.update(inner);
    let digest = outer.finalize();
    let mut out = [0u8; 20];
    out.copy_from_slice(&digest);
    out
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

pub fn crc32(bytes: &[u8]) -> u32 {
    crc32fast::hash(bytes)
}

fn ecb_encrypt(key: &[u8; 32], iv: [u8; 16]) -> [u8; 16] {
    let mut block = GenericArray::clone_from_slice(&iv);
    Aes256::new(key.into()).encrypt_block(&mut block);
    let mut out = [0u8; 16];
    out.copy_from_slice(&block);
    out
}

fn ecb_decrypt(key: &[u8; 32], block: &[u8]) -> Result<[u8; 16], Error> {
    let mut data = GenericArray::clone_from_slice(
        block
            .get(..16)
            .ok_or_else(|| Error::Crypto("bloc IV incomplet".into()))?,
    );
    Aes256::new(key.into()).decrypt_block(&mut data);
    let mut out = [0u8; 16];
    out.copy_from_slice(&data);
    Ok(out)
}

/// Canal symétrique après l'échange de clés.
#[derive(Clone)]
pub struct Channel {
    key: [u8; 32],
    /// Secret HMAC, présent quand le CM a imposé un nonce.
    hmac: Option<[u8; 16]>,
}

impl Channel {
    pub fn new(key: [u8; 32], hmac: Option<[u8; 16]>) -> Self {
        Self { key, hmac }
    }

    /// IV (aléatoire, ou HMAC + 3 octets aléatoires), IV chiffré en tête, puis
    /// AES-CBC/PKCS#7.
    pub fn encrypt(&self, plaintext: &[u8]) -> Vec<u8> {
        let iv = match &self.hmac {
            None => random_bytes::<16>(),
            Some(secret) => {
                let prefix: [u8; 3] = random_bytes();
                let mut signed = Vec::with_capacity(3 + plaintext.len());
                signed.extend_from_slice(&prefix);
                signed.extend_from_slice(plaintext);
                let digest = hmac_sha1(secret, &signed);
                let mut iv = [0u8; 16];
                iv[..13].copy_from_slice(&digest[..13]);
                iv[13..].copy_from_slice(&prefix);
                iv
            }
        };
        let encrypted_iv = ecb_encrypt(&self.key, iv);
        let ciphertext = Encryptor::<Aes256>::new((&self.key).into(), (&iv).into())
            .encrypt_padded_vec_mut::<Pkcs7>(plaintext);
        let mut out = Vec::with_capacity(16 + ciphertext.len());
        out.extend_from_slice(&encrypted_iv);
        out.extend_from_slice(&ciphertext);
        out
    }

    pub fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>, Error> {
        if data.len() < 16 || (data.len() - 16) % 16 != 0 {
            return Err(Error::Crypto(format!(
                "message chiffré de taille invalide : {}",
                data.len()
            )));
        }
        let iv = ecb_decrypt(&self.key, &data[..16])?;
        let plaintext = Decryptor::<Aes256>::new((&self.key).into(), (&iv).into())
            .decrypt_padded_vec_mut::<Pkcs7>(&data[16..])
            .map_err(|_| Error::Crypto("rembourrage AES invalide".into()))?;
        if let Some(secret) = &self.hmac {
            let mut signed = Vec::with_capacity(3 + plaintext.len());
            signed.extend_from_slice(&iv[13..]);
            signed.extend_from_slice(&plaintext);
            let digest = hmac_sha1(secret, &signed);
            if digest[..13] != iv[..13] {
                return Err(Error::Crypto("HMAC du canal invalide".into()));
            }
        }
        Ok(plaintext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modulus_is_a_1024_bit_key() {
        use rsa::traits::PublicKeyParts;
        let bytes = decode_hex(PUBLIC_MODULUS);
        assert_eq!(bytes.len(), 128);
        assert_eq!(bytes[0], 0xdf);
        let key = public_key().unwrap();
        assert_eq!(key.size(), 128);
        assert_eq!(key.e(), &BigUint::from(17u32));
    }

    #[test]
    fn oaep_encrypts_a_session_key() {
        // 128 octets = clé de 1024 bits, comme le renvoie un CM.
        let encrypted = encrypt_session_key(&[0x42; 48]).unwrap();
        assert_eq!(encrypted.len(), 128);
        assert_ne!(encrypted, encrypt_session_key(&[0x42; 48]).unwrap());
    }

    #[test]
    fn hmac_sha1_matches_the_reference_vector() {
        // RFC 2202, cas 1.
        let digest = hmac_sha1(&[0x0b; 20], b"Hi There");
        assert_eq!(
            digest,
            [
                0xb6, 0x17, 0x31, 0x86, 0x55, 0x05, 0x72, 0x64, 0xe2, 0x8b, 0xc0, 0xb6, 0xfb, 0x37,
                0x8c, 0x8e, 0xf1, 0x46, 0xbe, 0x00
            ]
        );
    }

    #[test]
    fn crc_matches_the_reference_vector() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn channel_round_trips_every_length() {
        for hmac in [None, Some([0x44; 16])] {
            let channel = Channel::new([0x11; 32], hmac);
            for plaintext in [&b""[..], b"a", b"seize octets !!", &[0x5a; 1000]] {
                let encrypted = channel.encrypt(plaintext);
                assert_eq!((encrypted.len() - 16) % 16, 0);
                assert_eq!(channel.decrypt(&encrypted).unwrap(), plaintext);
            }
        }
    }

    #[test]
    fn channel_uses_a_fresh_iv() {
        let channel = Channel::new([0x22; 32], Some([0x55; 16]));
        let first = channel.encrypt(b"message");
        let second = channel.encrypt(b"message");
        assert_ne!(first, second);
        assert_eq!(channel.decrypt(&first).unwrap(), b"message");
        assert_eq!(channel.decrypt(&second).unwrap(), b"message");
    }

    #[test]
    fn channel_rejects_a_tampered_hmac() {
        let channel = Channel::new([0x66; 32], Some([0x77; 16]));
        let mut encrypted = channel.encrypt(b"message authentifie");
        encrypted[1] ^= 0x01;
        assert!(matches!(channel.decrypt(&encrypted), Err(Error::Crypto(_))));
    }

    #[test]
    fn channel_rejects_misaligned_ciphertext() {
        let channel = Channel::new([0x33; 32], None);
        assert!(channel.decrypt(b"court").is_err());
        assert!(channel.decrypt(&[0u8; 17]).is_err());
        assert!(channel.decrypt(&[0u8; 15]).is_err());
    }
}
