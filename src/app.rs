//! État de l'interface et actions : connexion, déconnexion, édition des connexions.

use crate::config::{self, Config, Vpn};
use crate::crypto::{Key, Vault};
use crate::input::Input;
use crate::vpn::{self, Session};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant, SystemTime};

pub type Msg = Box<dyn FnOnce(&mut App) + Send>;
pub type OnText = Box<dyn FnOnce(&mut App, String) + Send>;
pub type OnYes = Box<dyn FnOnce(&mut App) + Send>;
type OnSudo = Box<dyn FnOnce(&mut App, Option<String>) + Send>;

#[derive(Clone, Copy, PartialEq)]
pub enum Severity {
    Info,
    Ok,
    Error,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Focus {
    List,
    Buttons,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Button {
    Toggle,
    Edit,
    Add,
    Delete,
    Quit,
}
pub const BUTTONS: [Button; 5] = [Button::Toggle, Button::Edit, Button::Add, Button::Delete, Button::Quit];

#[derive(Clone, Copy, PartialEq)]
pub enum BusyKind {
    Connecting,
    Disconnecting,
}

pub struct Busy {
    pub name: String,
    pub kind: BusyKind,
    pub since: Instant,
}

pub struct Prompt {
    pub title: String,
    pub text: String,
    pub input: Input,
    pub mask: bool,
    pub error: Option<String>,
    on_ok: Option<OnText>,
    /// Échap ; par défaut la fenêtre se ferme simplement.
    on_cancel: Option<OnYes>,
}

pub struct Confirm {
    pub title: String,
    pub text: String,
    pub yes_focused: bool,
    on_yes: Option<OnYes>,
}

/// Champs du formulaire : libellé, aide.
pub const FORM_FIELDS: [(&str, &str); 8] = [
    ("Nom", "affiché dans la barre ; nomme aussi l'interface réseau (vpn-<nom>)"),
    ("Serveur", "adresse du serveur, ex. vpn.exemple.fr"),
    ("Protocole", "anyconnect, gp, nc, pulse, f5, fortinet ou array"),
    ("Groupe", "--authgroup ; vide : aucun"),
    ("Utilisateur", "--user"),
    ("Mot de passe", "chiffré dans config.toml avec le mot de passe maître ; vide : demandé à chaque connexion"),
    ("Commande mdp", "commande qui affiche le mot de passe (ex. pass show vpn/ul), si le champ précédent est vide"),
    ("Options", "arguments openconnect en plus, séparés par des espaces (ex. --servercert=pin-sha256:…)"),
];
pub const FORM_PASSWORD: usize = 5;
/// Index de focus des boutons du formulaire.
pub const FORM_SAVE: usize = FORM_FIELDS.len();
pub const FORM_CANCEL: usize = FORM_FIELDS.len() + 1;

pub struct Form {
    /// Nom de la connexion modifiée ; None : nouvelle connexion.
    pub editing: Option<String>,
    pub inputs: Vec<Input>,
    pub focus: usize,
    pub error: Option<String>,
}

pub enum Modal {
    Prompt(Prompt),
    Confirm(Confirm),
    Form(Form),
    Help,
}

/// Zones cliquables, relevées à chaque dessin.
#[derive(Default)]
pub struct Areas {
    pub list: Rect,
    pub list_offset: usize,
    pub buttons: Vec<(Rect, usize)>,
    pub modal_buttons: Vec<(Rect, usize)>,
}

pub struct App {
    pub cfg: Config,
    pub cfg_error: Option<String>,
    cfg_mtime: Option<SystemTime>,
    pub selected: usize,
    pub focus: Focus,
    pub button: usize,
    pub sessions: Vec<Session>,
    pub busy: Option<Busy>,
    cancelled: bool,
    pub modals: Vec<Modal>,
    pub message: Option<(String, Severity, Instant)>,
    pub log: Vec<String>,
    pub areas: Areas,
    pub quit: bool,
    tx: Sender<Msg>,
    sudo_nopasswd: Option<bool>,
    sudo_pw: Option<String>,
    /// Mots de passe VPN saisis : gardés tant que l'app est ouverte.
    vpn_pw: HashMap<String, String>,
    /// Clé tirée du mot de passe maître ; None tant que l'app n'est pas déverrouillée.
    key: Option<Key>,
    unlocking: bool,
    last_click: Option<(usize, Instant)>,
}

impl App {
    pub fn new(tx: Sender<Msg>) -> App {
        let mut app = App {
            cfg: Config::default(),
            cfg_error: None,
            cfg_mtime: None,
            selected: 0,
            focus: Focus::List,
            button: 0,
            sessions: Vec::new(),
            busy: None,
            cancelled: false,
            modals: Vec::new(),
            message: None,
            log: Vec::new(),
            areas: Areas::default(),
            quit: false,
            tx,
            sudo_nopasswd: None,
            sudo_pw: None,
            vpn_pw: HashMap::new(),
            key: None,
            unlocking: false,
            last_click: None,
        };
        app.reload();
        // sur la connexion active, s'il y en a une
        if let Some(i) = app.cfg.vpns.iter().position(|v| app.session(&v.name).is_some()) {
            app.selected = i;
        }
        app.read_log();
        app
    }

    // ------------------------------------------------------------ état

    pub fn current(&self) -> Option<&Vpn> {
        self.cfg.vpns.get(self.selected)
    }

    pub fn session(&self, name: &str) -> Option<&Session> {
        self.sessions.iter().find(|s| s.vpn.as_deref() == Some(name))
    }

    pub fn reload(&mut self) {
        self.cfg_mtime = config::mtime();
        match config::load() {
            Ok(c) => {
                self.cfg = c;
                self.cfg_error = None;
            }
            Err(e) => self.cfg_error = Some(e),
        }
        self.selected = self.selected.min(self.cfg.vpns.len().saturating_sub(1));
        self.refresh();
        self.ensure_unlocked();
        self.encrypt_plain_passwords();
    }

    pub fn refresh(&mut self) {
        self.sessions = vpn::sessions(&self.cfg);
    }

    fn read_log(&mut self) {
        self.log = match self.current() {
            Some(v) => std::fs::read_to_string(vpn::log_path(&v.name))
                .map(|t| {
                    let l: Vec<String> = t.lines().map(String::from).collect();
                    l[l.len().saturating_sub(500)..].to_vec()
                })
                .unwrap_or_default(),
            None => Vec::new(),
        };
    }

    /// Toutes les secondes : processus, config modifiée à la main, journal.
    pub fn tick(&mut self) {
        let editing = self.modals.iter().any(|m| matches!(m, Modal::Form(_)));
        if !editing && config::mtime() != self.cfg_mtime {
            self.reload();
        } else {
            self.refresh();
        }
        self.read_log();
        if self.message.as_ref().is_some_and(|m| m.2.elapsed() > Duration::from_secs(8)) {
            self.message = None;
        }
    }

    pub fn notify(&mut self, text: &str, sev: Severity) {
        self.message = Some((text.to_string(), sev, Instant::now()));
    }

    fn spawn(&self, f: impl FnOnce() -> Msg + Send + 'static) {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
    }

    fn select(&mut self, i: usize) {
        if i < self.cfg.vpns.len() && i != self.selected {
            self.selected = i;
            self.read_log();
        }
    }

    pub fn toggle_label(&self) -> &'static str {
        let Some(v) = self.current() else { return "Connecter" };
        match &self.busy {
            Some(b) if b.name == v.name && b.kind == BusyKind::Connecting => "Annuler",
            _ if self.session(&v.name).is_some() => "Déconnecter",
            _ => "Connecter",
        }
    }

    // ------------------------------------------------------------ fenêtres

    pub fn prompt(&mut self, title: &str, text: &str, mask: bool, error: Option<String>, on_ok: OnText) {
        self.modals.push(Modal::Prompt(Prompt {
            title: title.into(),
            text: text.into(),
            input: Input::default(),
            mask,
            error,
            on_ok: Some(on_ok),
            on_cancel: None,
        }));
    }

    /// Échap sur la dernière fenêtre de saisie ouverte.
    fn on_cancel(&mut self, f: OnYes) {
        if let Some(Modal::Prompt(p)) = self.modals.last_mut() {
            p.on_cancel = Some(f);
        }
    }

    // ------------------------------------------------------------ mot de passe maître

    /// Au lancement (et quand config.toml redevient lisible) : déverrouiller, ou créer le coffre.
    fn ensure_unlocked(&mut self) {
        if self.key.is_some() || self.unlocking || self.cfg_error.is_some() {
            return;
        }
        self.unlocking = true;
        match self.cfg.vault.clone() {
            Some(v) => self.ask_master(v, None),
            None => self.ask_new_master(false, None),
        }
    }

    fn ask_master(&mut self, vault: Vault, error: Option<String>) {
        self.prompt(
            "myVpnManager",
            "Mot de passe maître :",
            true,
            error,
            Box::new(move |app, pw| {
                app.notify("Déverrouillage…", Severity::Info);
                app.spawn(move || -> Msg {
                    let r = vault.unlock(&pw);
                    Box::new(move |app: &mut App| match r {
                        Ok(key) => {
                            app.message = None;
                            app.key = Some(key);
                            app.unlocking = false;
                            app.encrypt_plain_passwords();
                        }
                        Err(e) => app.ask_master(vault, Some(e)),
                    })
                });
            }),
        );
        self.on_cancel(Box::new(|app| app.quit = true));
    }

    /// Nouveau mot de passe maître, saisi deux fois. `changing` : remplace l'actuel (sinon,
    /// premier lancement : Échap quitte).
    fn ask_new_master(&mut self, changing: bool, error: Option<String>) {
        let text = if changing {
            "Tous les mots de passe enregistrés seront rechiffrés.\nNouveau mot de passe maître :"
        } else {
            "Il chiffre les mots de passe des connexions dans config.toml\net sera demandé à chaque lancement.\nNouveau mot de passe maître :"
        };
        self.prompt(
            "Mot de passe maître",
            text,
            true,
            error,
            Box::new(move |app, pw| {
                if pw.is_empty() {
                    return app.ask_new_master(changing, Some("Le mot de passe maître ne peut pas être vide.".into()));
                }
                app.prompt(
                    "Mot de passe maître",
                    "Confirmer le mot de passe maître :",
                    true,
                    None,
                    Box::new(move |app, pw2| {
                        if pw2 != pw {
                            return app.ask_new_master(changing, Some("Les deux saisies diffèrent.".into()));
                        }
                        app.notify("Création de la clé…", Severity::Info);
                        app.spawn(move || -> Msg {
                            let r = Vault::create(&pw);
                            Box::new(move |app: &mut App| match r {
                                Ok((vault, key)) => app.install_vault(vault, key),
                                Err(e) => app.ask_new_master(changing, Some(e)),
                            })
                        });
                    }),
                );
                if !changing {
                    app.on_cancel(Box::new(|app| app.quit = true));
                }
            }),
        );
        if !changing {
            self.on_cancel(Box::new(|app| app.quit = true));
        }
    }

    /// Range le nouveau coffre et (re)chiffre tous les mots de passe avec la nouvelle clé.
    fn install_vault(&mut self, vault: Vault, key: Key) {
        let old = self.key.take();
        let mut cfg = self.cfg.clone();
        let mut lost = Vec::new();
        for v in &mut cfg.vpns {
            let plain = if !v.password.is_empty() {
                Some(std::mem::take(&mut v.password))
            } else if !v.password_enc.is_empty() {
                // sans l'ancienne clé (coffre supprimé, mot de passe oublié) : perdu
                old.as_ref().and_then(|k| k.decrypt(&v.password_enc).ok())
            } else {
                None
            };
            v.password_enc = match plain {
                Some(p) => key.encrypt(&p),
                None => {
                    if !v.password_enc.is_empty() {
                        lost.push(v.name.clone());
                    }
                    String::new()
                }
            };
        }
        cfg.vault = Some(vault);
        if let Err(e) = config::save(&cfg) {
            self.key = old;
            self.unlocking = false;
            self.notify(&e, Severity::Error);
            return;
        }
        self.cfg = cfg;
        self.cfg_mtime = config::mtime();
        self.key = Some(key);
        self.unlocking = false;
        if lost.is_empty() {
            self.notify("Mot de passe maître enregistré.", Severity::Ok);
        } else {
            let l = lost.join(", ");
            self.notify(&format!("Mot de passe maître enregistré ; mots de passe illisibles effacés : {l}"), Severity::Error);
        }
    }

    /// `password = "…"` en clair (écrit à la main) : chiffré dans `password-enc`, puis effacé.
    fn encrypt_plain_passwords(&mut self) {
        let Some(key) = &self.key else { return };
        if self.cfg_error.is_some() || self.cfg.vpns.iter().all(|v| v.password.is_empty()) {
            return;
        }
        let mut cfg = self.cfg.clone();
        let mut n = 0;
        for v in cfg.vpns.iter_mut().filter(|v| !v.password.is_empty()) {
            v.password_enc = key.encrypt(&std::mem::take(&mut v.password));
            n += 1;
        }
        match config::save(&cfg) {
            Ok(()) => {
                self.cfg = cfg;
                self.cfg_mtime = config::mtime();
                self.notify(&format!("{n} mot(s) de passe en clair chiffré(s) dans config.toml."), Severity::Ok);
            }
            Err(e) => self.notify(&e, Severity::Error),
        }
    }

    pub fn change_master(&mut self) {
        if self.key.is_some() && self.config_ok() {
            self.ask_new_master(true, None);
        }
    }

    pub fn confirm(&mut self, title: &str, text: &str, on_yes: OnYes) {
        self.modals.push(Modal::Confirm(Confirm {
            title: title.into(),
            text: text.into(),
            yes_focused: false,
            on_yes: Some(on_yes),
        }));
    }

    // ------------------------------------------------------------ sudo et mots de passe

    fn with_sudo(&mut self, k: OnSudo) {
        if self.sudo_nopasswd.is_none() {
            self.sudo_nopasswd = Some(vpn::sudo_nopasswd());
        }
        if self.sudo_nopasswd == Some(true) {
            return k(self, None);
        }
        if let Some(pw) = self.sudo_pw.clone() {
            return k(self, Some(pw));
        }
        self.ask_sudo(k, None);
    }

    fn ask_sudo(&mut self, k: OnSudo, error: Option<String>) {
        let user = std::env::var("USER").unwrap_or_default();
        let text = format!("openconnect a besoin des droits root.\nMot de passe sudo de {user} :");
        self.prompt(
            "sudo",
            &text,
            true,
            error,
            Box::new(move |app, pw| {
                app.notify("Vérification du mot de passe sudo…", Severity::Info);
                app.spawn(move || -> Msg {
                    let r = vpn::sudo_check(&pw);
                    Box::new(move |app: &mut App| match r {
                        Ok(()) => {
                            app.message = None;
                            app.sudo_pw = Some(pw.clone());
                            k(app, Some(pw));
                        }
                        Err(e) => app.ask_sudo(k, Some(e)),
                    })
                });
            }),
        );
    }

    fn with_password(&mut self, v: &Vpn, k: OnText) {
        // déchiffré au dernier moment
        if !v.password_enc.is_empty() {
            let r = match &self.key {
                Some(key) => key.decrypt(&v.password_enc),
                None => Err("app verrouillée".into()),
            };
            return match r {
                Ok(p) => k(self, p),
                Err(e) => self.notify(&format!("« {} » : {e}", v.name), Severity::Error),
            };
        }
        if !v.password.is_empty() {
            return k(self, v.password.clone());
        }
        if !v.password_command.is_empty() {
            return match vpn::password_from_command(&v.password_command) {
                Ok(p) => k(self, p),
                Err(e) => self.notify(&e, Severity::Error),
            };
        }
        if let Some(p) = self.vpn_pw.get(&v.name).cloned() {
            return k(self, p);
        }
        let name = v.name.clone();
        let who = if v.user.is_empty() { v.host.clone() } else { format!("{} sur {}", v.user, v.host) };
        self.prompt(
            &format!("Connexion « {} »", v.name),
            &format!("Mot de passe de {who} :"),
            true,
            None,
            Box::new(move |app, pw| {
                app.vpn_pw.insert(name, pw.clone());
                k(app, pw);
            }),
        );
    }

    // ------------------------------------------------------------ actions

    /// Bouton principal : connecter, déconnecter, ou annuler la connexion en cours.
    pub fn toggle(&mut self) {
        let Some(v) = self.current().cloned() else { return };
        if let Some(b) = &self.busy {
            if b.kind == BusyKind::Connecting && b.name == v.name {
                self.disconnect(None);
            } else {
                self.notify("Une opération est déjà en cours…", Severity::Info);
            }
            return;
        }
        if self.session(&v.name).is_some() {
            self.disconnect(Some(v.name));
        } else {
            self.connect(v);
        }
    }

    fn connect(&mut self, v: Vpn) {
        let v2 = v.clone();
        self.with_password(
            &v2,
            Box::new(move |app, pw| app.with_sudo(Box::new(move |app, sudo| app.start_connect(v, pw, sudo)))),
        );
    }

    fn start_connect(&mut self, v: Vpn, pw: String, sudo: Option<String>) {
        // un VPN à la fois : l'actif est coupé d'abord
        let others: Vec<u32> = self.sessions.iter().map(|s| s.pid).collect();
        self.busy = Some(Busy { name: v.name.clone(), kind: BusyKind::Connecting, since: Instant::now() });
        self.cancelled = false;
        self.notify(&format!("Connexion à « {} »…", v.name), Severity::Info);
        self.log.clear();
        self.spawn(move || -> Msg {
            if !others.is_empty() {
                if let Err(e) = vpn::disconnect(&others, sudo.as_deref()) {
                    return Box::new(move |app: &mut App| {
                        app.busy = None;
                        app.notify(&format!("Déconnexion du VPN actif : {e}"), Severity::Error);
                    });
                }
            }
            let r = vpn::connect(&v, &pw, sudo.as_deref());
            Box::new(move |app: &mut App| {
                app.busy = None;
                app.refresh();
                app.read_log();
                match r {
                    Ok(()) => app.notify(&format!("« {} » connecté.", v.name), Severity::Ok),
                    Err(_) if app.cancelled => app.notify("Connexion annulée.", Severity::Info),
                    Err(e) => {
                        // peut-être un mauvais mot de passe : redemandé la prochaine fois
                        app.vpn_pw.remove(&v.name);
                        app.notify(&format!("« {} » : {e}", v.name), Severity::Error);
                    }
                }
            })
        });
    }

    /// Coupe la connexion `only` (toutes si None, ce qui annule aussi une connexion en cours).
    pub fn disconnect(&mut self, only: Option<String>) {
        let pids: Vec<u32> =
            self.sessions.iter().filter(|s| only.is_none() || s.vpn == only).map(|s| s.pid).collect();
        if pids.is_empty() {
            self.notify("Aucun VPN actif.", Severity::Info);
            return;
        }
        let cancelling = self.busy.as_ref().is_some_and(|b| b.kind == BusyKind::Connecting);
        self.with_sudo(Box::new(move |app, sudo| {
            if cancelling {
                app.cancelled = true;
            } else {
                let name = only.clone().unwrap_or_default();
                app.busy = Some(Busy { name, kind: BusyKind::Disconnecting, since: Instant::now() });
            }
            app.spawn(move || -> Msg {
                let r = vpn::disconnect(&pids, sudo.as_deref());
                Box::new(move |app: &mut App| {
                    if !cancelling {
                        app.busy = None;
                    }
                    app.refresh();
                    match r {
                        Ok(()) if !cancelling => app.notify("VPN déconnecté.", Severity::Ok),
                        Ok(()) => {}
                        Err(e) => app.notify(&e, Severity::Error),
                    }
                })
            });
        }));
    }

    fn config_ok(&mut self) -> bool {
        if let Some(e) = &self.cfg_error {
            let e = format!("config.toml illisible, à corriger d'abord : {e}");
            self.notify(&e, Severity::Error);
            return false;
        }
        true
    }

    pub fn add(&mut self) {
        if !self.config_ok() {
            return;
        }
        let v = Vpn { protocol: "anyconnect".into(), ..Default::default() };
        self.open_form(None, &v);
    }

    pub fn edit(&mut self) {
        if !self.config_ok() {
            return;
        }
        if let Some(v) = self.current().cloned() {
            self.open_form(Some(v.name.clone()), &v);
        }
    }

    fn open_form(&mut self, editing: Option<String>, v: &Vpn) {
        let password = match (&self.key, v.password_enc.is_empty()) {
            (Some(k), false) => match k.decrypt(&v.password_enc) {
                Ok(p) => p,
                Err(e) => return self.notify(&format!("« {} » : {e}", v.name), Severity::Error),
            },
            _ => v.password.clone(),
        };
        let vals = [
            &v.name,
            &v.host,
            &v.protocol,
            &v.authgroup,
            &v.user,
            &password,
            &v.password_command,
            &v.extra_args.join(" "),
        ];
        let inputs = vals.iter().map(|s| Input::new(s)).collect();
        self.modals.push(Modal::Form(Form { editing, inputs, focus: 0, error: None }));
    }

    /// Enregistre ; en cas d'erreur, rend le formulaire avec le message.
    fn save_form(&mut self, mut f: Form) -> Option<Modal> {
        let val = |i: usize| f.inputs[i].text.trim().to_string();
        let v = Vpn {
            name: val(0),
            host: val(1),
            protocol: val(2),
            authgroup: val(3),
            user: val(4),
            password: String::new(),
            password_enc: String::new(),
            password_command: val(6),
            extra_args: val(7).split_whitespace().map(String::from).collect(),
        };
        let taken = self.cfg.vpns.iter().any(|o| o.name == v.name && Some(&o.name) != f.editing.as_ref());
        let error = if v.name.is_empty() {
            Some((0, "Le nom est obligatoire.".to_string()))
        } else if taken {
            Some((0, format!("« {} » existe déjà.", v.name)))
        } else if v.host.is_empty() {
            Some((1, "Le serveur est obligatoire.".to_string()))
        } else if !config::PROTOCOLS.contains(&v.protocol.as_str()) {
            Some((2, format!("Protocole inconnu : {}", config::PROTOCOLS.join(", "))))
        } else {
            None
        };
        if let Some((i, e)) = error {
            f.focus = i;
            f.error = Some(e);
            return Some(Modal::Form(f));
        }
        let mut v = v;
        // un mot de passe peut commencer ou finir par une espace : pas de trim
        let pw = &f.inputs[FORM_PASSWORD].text;
        if !pw.is_empty() {
            match &self.key {
                Some(k) => v.password_enc = k.encrypt(pw),
                None => {
                    f.error = Some("App verrouillée : mot de passe non enregistré.".into());
                    return Some(Modal::Form(f));
                }
            }
        }
        let mut cfg = self.cfg.clone();
        let idx = match f.editing.as_ref().and_then(|n| cfg.vpns.iter().position(|o| &o.name == n)) {
            Some(i) => {
                cfg.vpns[i] = v.clone();
                i
            }
            None => {
                cfg.vpns.push(v.clone());
                cfg.vpns.len() - 1
            }
        };
        if let Err(e) = config::save(&cfg) {
            f.error = Some(e);
            return Some(Modal::Form(f));
        }
        if let Some(old) = &f.editing {
            self.vpn_pw.remove(old);
        }
        self.cfg = cfg;
        self.cfg_mtime = config::mtime();
        self.selected = idx;
        self.refresh();
        self.read_log();
        self.notify(&format!("« {} » enregistré.", v.name), Severity::Ok);
        None
    }

    pub fn delete(&mut self) {
        if !self.config_ok() {
            return;
        }
        let Some(name) = self.current().map(|v| v.name.clone()) else { return };
        self.confirm(
            "Supprimer",
            &format!("Supprimer « {name} » de la configuration ?"),
            Box::new(move |app| {
                let mut cfg = app.cfg.clone();
                cfg.vpns.retain(|v| v.name != name);
                match config::save(&cfg) {
                    Ok(()) => {
                        app.cfg = cfg;
                        app.cfg_mtime = config::mtime();
                        app.selected = app.selected.min(app.cfg.vpns.len().saturating_sub(1));
                        app.read_log();
                        app.notify(&format!("« {name} » supprimé."), Severity::Ok);
                    }
                    Err(e) => app.notify(&e, Severity::Error),
                }
            }),
        );
    }

    fn press(&mut self, b: Button) {
        match b {
            Button::Toggle => self.toggle(),
            Button::Edit => self.edit(),
            Button::Add => self.add(),
            Button::Delete => self.delete(),
            Button::Quit => self.quit = true,
        }
    }

    // ------------------------------------------------------------ clavier

    pub fn on_key(&mut self, k: KeyEvent) {
        if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return;
        }
        if let Some(m) = self.modals.pop() {
            if let Some(m) = self.modal_key(m, k) {
                self.modals.push(m);
            }
            return;
        }
        let n = self.cfg.vpns.len();
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Right | KeyCode::Left | KeyCode::Char('l') | KeyCode::Char('h') => {
                self.focus = if self.focus == Focus::List { Focus::Buttons } else { Focus::List };
            }
            KeyCode::Up | KeyCode::Char('k') => match self.focus {
                Focus::List => self.select(self.selected.saturating_sub(1)),
                Focus::Buttons => self.button = self.button.saturating_sub(1),
            },
            KeyCode::Down | KeyCode::Char('j') => match self.focus {
                Focus::List => self.select((self.selected + 1).min(n.saturating_sub(1))),
                Focus::Buttons => self.button = (self.button + 1).min(BUTTONS.len() - 1),
            },
            KeyCode::Home | KeyCode::Char('g') => self.select(0),
            KeyCode::End | KeyCode::Char('G') => self.select(n.saturating_sub(1)),
            KeyCode::Enter | KeyCode::Char(' ') => match self.focus {
                Focus::List => self.toggle(),
                Focus::Buttons => self.press(BUTTONS[self.button]),
            },
            KeyCode::Char('c') => self.toggle(),
            KeyCode::Char('d') => self.disconnect(None),
            KeyCode::Char('a') | KeyCode::Char('n') => self.add(),
            KeyCode::Char('e') => self.edit(),
            KeyCode::Char('x') | KeyCode::Delete => self.delete(),
            KeyCode::Char('r') => {
                self.reload();
                self.read_log();
                self.notify("Configuration relue.", Severity::Info);
            }
            KeyCode::Char('m') => self.change_master(),
            KeyCode::Char('?') => self.modals.push(Modal::Help),
            _ => {}
        }
    }

    /// Touche dans la fenêtre du dessus ; Some : elle reste ouverte.
    fn modal_key(&mut self, m: Modal, k: KeyEvent) -> Option<Modal> {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match m {
            Modal::Help => None,
            Modal::Prompt(mut p) => match k.code {
                KeyCode::Esc => {
                    if let Some(f) = p.on_cancel.take() {
                        f(self);
                    }
                    None
                }
                KeyCode::Enter => {
                    if let Some(f) = p.on_ok.take() {
                        f(self, p.input.text.clone());
                    }
                    None
                }
                _ => {
                    p.input.key(k);
                    Some(Modal::Prompt(p))
                }
            },
            Modal::Confirm(mut c) => match k.code {
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('q') => None,
                KeyCode::Char('o') | KeyCode::Char('y') => {
                    self.confirm_yes(c);
                    None
                }
                KeyCode::Enter | KeyCode::Char(' ') => {
                    if c.yes_focused {
                        self.confirm_yes(c);
                    }
                    None
                }
                KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab | KeyCode::Char('h') | KeyCode::Char('l') => {
                    c.yes_focused = !c.yes_focused;
                    Some(Modal::Confirm(c))
                }
                _ => Some(Modal::Confirm(c)),
            },
            Modal::Form(mut f) => {
                let last = FORM_CANCEL;
                match k.code {
                    KeyCode::Esc => return None,
                    KeyCode::Char('s') if ctrl => return self.save_form(f),
                    KeyCode::Tab | KeyCode::Down => f.focus = (f.focus + 1).min(last),
                    KeyCode::BackTab | KeyCode::Up => f.focus = f.focus.saturating_sub(1),
                    KeyCode::Enter => match f.focus {
                        FORM_SAVE => return self.save_form(f),
                        FORM_CANCEL => return None,
                        i => f.focus = i + 1,
                    },
                    KeyCode::Left if f.focus == FORM_CANCEL => f.focus = FORM_SAVE,
                    KeyCode::Right if f.focus == FORM_SAVE => f.focus = FORM_CANCEL,
                    _ if f.focus < FORM_SAVE => {
                        f.input_key(k);
                    }
                    _ => {}
                }
                Some(Modal::Form(f))
            }
        }
    }

    fn confirm_yes(&mut self, mut c: Confirm) {
        if let Some(f) = c.on_yes.take() {
            f(self);
        }
    }

    // ------------------------------------------------------------ souris

    pub fn on_mouse(&mut self, m: MouseEvent) {
        let pos = Position::new(m.column, m.row);
        if !self.modals.is_empty() {
            if m.kind != MouseEventKind::Down(MouseButton::Left) {
                return;
            }
            let Some(&(_, id)) = self.areas.modal_buttons.iter().find(|(r, _)| r.contains(pos)) else { return };
            let modal = self.modals.pop().unwrap();
            let back = match modal {
                Modal::Confirm(c) => {
                    if id == 0 {
                        self.confirm_yes(c);
                    }
                    None
                }
                Modal::Form(mut f) => match id {
                    FORM_SAVE => self.save_form(f),
                    FORM_CANCEL => None,
                    i => {
                        f.focus = i;
                        Some(Modal::Form(f))
                    }
                },
                other => Some(other),
            };
            if let Some(b) = back {
                self.modals.push(b);
            }
            return;
        }
        let list = self.areas.list;
        match m.kind {
            MouseEventKind::ScrollUp if list.contains(pos) => self.select(self.selected.saturating_sub(1)),
            MouseEventKind::ScrollDown if list.contains(pos) => {
                self.select((self.selected + 1).min(self.cfg.vpns.len().saturating_sub(1)))
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(&(_, i)) = self.areas.buttons.iter().find(|(r, _)| r.contains(pos)) {
                    self.focus = Focus::Buttons;
                    self.button = i;
                    self.press(BUTTONS[i]);
                } else if list.contains(pos) {
                    let i = self.areas.list_offset + (m.row - list.y) as usize;
                    if i < self.cfg.vpns.len() {
                        self.focus = Focus::List;
                        self.select(i);
                        // double clic : connecter / déconnecter
                        let double = self.last_click.is_some_and(|(j, t)| j == i && t.elapsed() < Duration::from_millis(400));
                        self.last_click = Some((i, Instant::now()));
                        if double {
                            self.last_click = None;
                            self.toggle();
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

impl Form {
    fn input_key(&mut self, k: KeyEvent) {
        // protocole : ←/→ sur un champ vide ou connu fait défiler la liste
        if self.focus == 2 && matches!(k.code, KeyCode::Left | KeyCode::Right) {
            let cur = self.inputs[2].text.trim();
            let i = config::PROTOCOLS.iter().position(|p| *p == cur);
            if i.is_some() || cur.is_empty() {
                let n = config::PROTOCOLS.len();
                let j = match (i, k.code) {
                    (Some(i), KeyCode::Right) => (i + 1) % n,
                    (Some(i), _) => (i + n - 1) % n,
                    (None, _) => 0,
                };
                self.inputs[2] = Input::new(config::PROTOCOLS[j]);
                return;
            }
        }
        self.inputs[self.focus].key(k);
    }
}
