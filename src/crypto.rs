//! Coffre : le mot de passe maître, demandé au lancement, donne une clé (Argon2id) ; les mots de
//! de passe des connexions sont chiffrés avec XChaCha20-Poly1305 (chiffrement authentifié,
//! nonce aléatoire de 24 octets) et rangés chiffrés dans config.toml (`password-enc`).
//! Ils ne sont déchiffrés qu'au moment de lancer openconnect.

use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

// Constantes internes du format : gardées de l'époque « myvpn », les changer rendrait
// illisibles les mots de passe déjà chiffrés.
/// Préfixe de version des valeurs chiffrées : `v1:` + base64(nonce ‖ chiffré ‖ tag).
const PREFIX: &str = "v1:";
const AAD: &[u8] = b"myvpn/v1";
const NONCE: usize = 24;
const TAG: usize = 16;
/// Chiffré dans `[vault] check` : sert à reconnaître le bon mot de passe maître.
const CHECK: &[u8] = b"myvpn: mot de passe maitre correct";

/// Argon2id, 64 Mio, 3 passes : quelques dixièmes de seconde au lancement.
const M_COST: u32 = 64 * 1024;
const T_COST: u32 = 3;
const P_COST: u32 = 1;

/// Section `[vault]` de config.toml : de quoi refaire la clé à partir du mot de passe maître.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Vault {
    pub kdf: String,
    pub m_cost: u32,
    pub t_cost: u32,
    pub p_cost: u32,
    pub salt: String,
    pub check: String,
}

/// Clé de chiffrement, effacée de la mémoire quand on la lâche.
#[derive(Clone)]
pub struct Key(Zeroizing<[u8; 32]>);

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::getrandom(&mut b).expect("getrandom");
    b
}

fn derive(password: &str, salt: &[u8], m: u32, t: u32, p: u32) -> Result<Key, String> {
    let params = Params::new(m, t, p, Some(32)).map_err(|e| format!("argon2 : {e}"))?;
    let mut out = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut *out)
        .map_err(|e| format!("argon2 : {e}"))?;
    Ok(Key(out))
}

impl Vault {
    /// Nouveau coffre (sel neuf) pour ce mot de passe maître.
    pub fn create(password: &str) -> Result<(Vault, Key), String> {
        let salt = random::<16>();
        let key = derive(password, &salt, M_COST, T_COST, P_COST)?;
        let vault = Vault {
            kdf: "argon2id".into(),
            m_cost: M_COST,
            t_cost: T_COST,
            p_cost: P_COST,
            salt: B64.encode(salt),
            check: key.encrypt_bytes(CHECK),
        };
        Ok((vault, key))
    }

    pub fn unlock(&self, password: &str) -> Result<Key, String> {
        if self.kdf != "argon2id" {
            return Err(format!("[vault] kdf inconnu : {}", self.kdf));
        }
        let salt = B64.decode(&self.salt).map_err(|_| "[vault] salt illisible".to_string())?;
        let key = derive(password, &salt, self.m_cost, self.t_cost, self.p_cost)?;
        match key.decrypt_bytes(&self.check) {
            Ok(c) if c.as_slice() == CHECK => Ok(key),
            _ => Err("Mot de passe maître incorrect.".into()),
        }
    }
}

impl Key {
    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new(chacha20poly1305::Key::from_slice(&self.0[..]))
    }

    fn encrypt_bytes(&self, data: &[u8]) -> String {
        let nonce = random::<NONCE>();
        let ct = self
            .cipher()
            .encrypt(XNonce::from_slice(&nonce), Payload { msg: data, aad: AAD })
            .expect("chiffrement");
        let mut v = nonce.to_vec();
        v.extend(ct);
        format!("{PREFIX}{}", B64.encode(v))
    }

    fn decrypt_bytes(&self, s: &str) -> Result<Zeroizing<Vec<u8>>, String> {
        let raw = s
            .strip_prefix(PREFIX)
            .and_then(|b| B64.decode(b.trim()).ok())
            .filter(|r| r.len() >= NONCE + TAG)
            .ok_or("valeur chiffrée illisible")?;
        let (nonce, ct) = raw.split_at(NONCE);
        self.cipher()
            .decrypt(XNonce::from_slice(nonce), Payload { msg: ct, aad: AAD })
            .map(Zeroizing::new)
            .map_err(|_| "déchiffrement impossible (chiffré avec un autre mot de passe maître ?)".into())
    }

    pub fn encrypt(&self, password: &str) -> String {
        self.encrypt_bytes(password.as_bytes())
    }

    pub fn decrypt(&self, s: &str) -> Result<String, String> {
        let b = self.decrypt_bytes(s)?;
        String::from_utf8(b.to_vec()).map_err(|_| "valeur déchiffrée invalide".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let (vault, key) = Vault::create("maître").unwrap();
        let enc = key.encrypt("secret VPN");
        assert_ne!(enc, key.encrypt("secret VPN"), "nonce aléatoire");
        let key2 = vault.unlock("maître").unwrap();
        assert_eq!(key2.decrypt(&enc).unwrap(), "secret VPN");
        assert!(vault.unlock("autre").is_err());
        // altération détectée
        let mut bad = enc.clone();
        bad.replace_range(10..11, if &enc[10..11] == "A" { "B" } else { "A" });
        assert!(key2.decrypt(&bad).is_err());
    }
}
