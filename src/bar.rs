//! Barre wayle : état en JSON (`status --json`, ou en continu avec `--watch`) et ouverture de
//! l'interface (`open`).

use crate::config::{self, Config};
use crate::vpn;
use serde_json::json;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::Duration;

/// `{text, alt, class, tooltip}` : alt = off | connecting | on (icon-map de wayle).
fn bar_json(cfg: &Config) -> String {
    let ss = vpn::sessions(cfg);
    let s = ss.iter().find(|s| s.up()).or(ss.first());
    let v = match s {
        None => json!({
            "text": "", "alt": "off", "class": "vpn-off",
            "tooltip": "VPN déconnecté\nclic : myVpnManager",
        }),
        Some(s) => {
            let who = if s.user.is_empty() { s.host.clone() } else { format!("{} · {}", s.user, s.host) };
            if s.up() {
                let mut tip = format!("VPN « {} » connecté\n{who}", s.label());
                if let Some(ip) = &s.ip {
                    tip.push_str(&format!("\nIP {ip}"));
                }
                if let Some(x) = vpn::since(s.started) {
                    tip.push_str(&format!("\ndepuis {x}"));
                }
                tip.push_str("\nclic : myVpnManager");
                json!({ "text": s.label(), "alt": "on", "class": "vpn-on", "tooltip": tip })
            } else {
                json!({
                    "text": format!("{}…", s.label()), "alt": "connecting", "class": "vpn-connecting",
                    "tooltip": format!("VPN « {} » : connexion en cours\n{who}", s.label()),
                })
            }
        }
    };
    v.to_string()
}

fn human(cfg: &Config) {
    let ss = vpn::sessions(cfg);
    if ss.is_empty() {
        println!("déconnecté");
    }
    for s in ss {
        let state = if s.up() { "connecté" } else { "connexion en cours" };
        let mut d = vec![s.host.clone()];
        d.extend(s.ip.clone().map(|ip| format!("IP {ip}")));
        d.extend(vpn::since(s.started).map(|x| format!("depuis {x}")));
        println!("{} : {state} ({})", s.label(), d.join(", "));
    }
}

pub fn status(args: &[String]) {
    let has = |a: &str| args.iter().any(|x| x == a);
    let mut cfg = config::load().unwrap_or_default();
    if !has("--watch") {
        if has("--json") { println!("{}", bar_json(&cfg)) } else { human(&cfg) }
        return;
    }
    // une ligne à chaque changement ; wayle redessine le bouton aussitôt
    let mut mtime = config::mtime();
    let mut last = String::new();
    let mut out = std::io::stdout().lock();
    loop {
        let m = config::mtime();
        if m != mtime {
            mtime = m;
            cfg = config::load().unwrap_or(cfg);
        }
        let j = bar_json(&cfg);
        if j != last {
            // barre fermée : on s'arrête
            if writeln!(out, "{j}").and_then(|_| out.flush()).is_err() {
                return;
            }
            last = j;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Ouvre l'interface dans un terminal, ou met au premier plan celle déjà ouverte (Hyprland).
pub fn open() {
    let open_already = Command::new("hyprctl")
        .args(["clients", "-j"])
        .stderr(Stdio::null())
        .output()
        .ok()
        .and_then(|o| serde_json::from_slice::<serde_json::Value>(&o.stdout).ok())
        .and_then(|v| v.as_array().map(|a| a.iter().any(|c| c["class"] == "myvpnmanager")))
        .unwrap_or(false);
    if open_already {
        let _ = Command::new("hyprctl").args(["dispatch", "focuswindow", "class:^(myvpnmanager)$"]).stdout(Stdio::null()).status();
        return;
    }
    let cfg = config::load().unwrap_or_default();
    let term = cfg.terminal.unwrap_or_else(|| config::DEFAULT_TERMINAL.into());
    let mut parts = term.split_whitespace();
    let Some(prog) = parts.next() else { return };
    // argv[0] et pas current_exe : sous Nix, celui-ci serait le binaire caché derrière le
    // wrapper (`.myvpnmanager-wrapped`), sans le PATH qu'il ajoute
    let exe = match std::env::args().next() {
        Some(a) if a.contains('/') => std::path::PathBuf::from(a),
        _ => vpn::which("myvpnmanager").or_else(|| std::env::current_exe().ok()).unwrap_or_else(|| "myvpnmanager".into()),
    };
    let mut c = Command::new(prog);
    c.args(parts).arg(exe).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    if let Err(e) = c.spawn() {
        eprintln!("myvpnmanager: {prog} : {e}");
        std::process::exit(1);
    }
}
