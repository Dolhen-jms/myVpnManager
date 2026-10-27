//! Configuration : `~/.config/myvpnmanager/config.toml`, une table `[[vpn]]` par connexion.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;

/// Protocoles d'openconnect (`--protocol`).
pub const PROTOCOLS: [&str; 7] = ["anyconnect", "gp", "nc", "pulse", "f5", "fortinet", "array"];

/// Terminal qui ouvre l'interface (`myvpnmanager open`, clic sur la barre) ; le binaire est ajouté au bout.
pub const DEFAULT_TERMINAL: &str = "kitty --class myvpnmanager --title myvpnmanager -e";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
    /// Coffre du mot de passe maître (sel, paramètres Argon2, contrôle).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault: Option<crate::crypto::Vault>,
    #[serde(default, rename = "vpn")]
    pub vpns: Vec<Vpn>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Vpn {
    pub name: String,
    pub host: String,
    #[serde(default = "default_protocol")]
    pub protocol: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub authgroup: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub user: String,
    /// En clair : chiffré dans `password-enc` (puis effacé) dès que l'app est déverrouillée.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub password: String,
    /// Chiffré avec la clé du mot de passe maître ; vide (et `password` vide) = demandé à la
    /// connexion, puis gardé en mémoire tant que l'app est ouverte.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub password_enc: String,
    /// Commande qui affiche le mot de passe (`pass show …`), si `password` est vide.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub password_command: String,
    /// Arguments openconnect en plus (`--servercert=…`, …).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_args: Vec<String>,
}

fn default_protocol() -> String {
    "anyconnect".into()
}

const HEADER: &str = "\
# myVpnManager -- une table [[vpn]] par connexion (modifiable aussi depuis l'interface : myvpnmanager).
#   name              nom affiché (barre wayle, interface reseau vpn-<name>)
#   host              serveur
#   protocol          anyconnect, gp, nc, pulse, f5, fortinet, array
#   authgroup, user   --authgroup, --user
#   password-enc      mot de passe chiffre (XChaCha20-Poly1305, cle tiree du mot de passe
#                     maitre par Argon2id) ; absent = demande a chaque connexion
#   password          en clair : chiffre dans password-enc puis efface au prochain lancement
#   password-command  commande qui affiche le mot de passe (si password est absent)
#   extra-args        arguments openconnect supplementaires
# [vault] : de quoi reconnaitre le mot de passe maitre. Oublie ? Supprimer [vault] et les
#   password-enc : un nouveau mot de passe maitre sera demande (mots de passe VPN a ressaisir).
# terminal : commande qui ouvre l'interface (clic sur la barre), le binaire est ajoute au bout.
";

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()))
}

fn xdg(var: &str, default_rel: &str) -> PathBuf {
    match std::env::var(var) {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => home().join(default_rel),
    }
}

/// L'app s'appelait « myvpn » : sa configuration est déplacée au premier lancement, et la
/// classe de fenêtre de l'ancien réglage `terminal` mise à jour (sinon `open` ne retrouverait
/// pas la fenêtre déjà ouverte).
pub fn migrate_old_dir() {
    let base = xdg("XDG_CONFIG_HOME", ".config");
    let (old, new) = (base.join("myvpn"), base.join("myvpnmanager"));
    if !old.is_dir() || new.exists() || std::fs::rename(&old, &new).is_err() {
        return;
    }
    let p = new.join("config.toml");
    if let Ok(t) = std::fs::read_to_string(&p) {
        let fixed = t.replace("kitty --class myvpn --title myvpn -e", DEFAULT_TERMINAL);
        if fixed != t {
            let _ = std::fs::write(&p, fixed);
        }
    }
}

pub fn path() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("myvpnmanager/config.toml")
}

/// Journaux d'openconnect : effacés au redémarrage, comme les connexions.
pub fn runtime_dir() -> PathBuf {
    match std::env::var("XDG_RUNTIME_DIR") {
        Ok(v) if !v.is_empty() => PathBuf::from(v).join("myvpnmanager"),
        _ => PathBuf::from(format!("/tmp/myvpnmanager-{}", unsafe { libc::getuid() })),
    }
}

pub fn home_dir() -> PathBuf {
    home()
}

pub fn mtime() -> Option<SystemTime> {
    std::fs::metadata(path()).and_then(|m| m.modified()).ok()
}

/// Sans fichier : configuration vide.
pub fn load() -> Result<Config, String> {
    let p = path();
    match std::fs::read_to_string(&p) {
        Ok(t) => toml::from_str(&t).map_err(|e| format!("{} : {e}", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(format!("{} : {e}", p.display())),
    }
}

/// Réécrit le fichier (mode 600 : il peut contenir des mots de passe).
pub fn save(cfg: &Config) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let p = path();
    let body = toml::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{} : {e}", d.display()))?;
    }
    let tmp = p.with_extension("toml.tmp");
    let r = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .and_then(|mut f| f.write_all(format!("{HEADER}\n{body}").as_bytes()))
        .and_then(|_| std::fs::rename(&tmp, &p));
    r.map_err(|e| format!("{} : {e}", p.display()))
}
