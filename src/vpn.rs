//! openconnect : sessions en cours (lues dans /proc), connexion, déconnexion, sudo.
//!
//! openconnect est lancé par `sudo` dans une nouvelle session (setsid), avec `--background` et
//! sa sortie dans un fichier : il survit à la fermeture de l'interface et de son terminal.
//! L'état n'est gardé nulle part : on retrouve les processus openconnect dans /proc.

use crate::config::{self, Config, Vpn};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub struct Session {
    pub pid: u32,
    /// Nom de la connexion de la config ; None : lancé ailleurs (ancien script…).
    pub vpn: Option<String>,
    pub host: String,
    pub user: String,
    pub iface: Option<String>,
    /// IPv4 de l'interface : présente une fois le tunnel monté.
    pub ip: Option<String>,
    /// Lancement, en secondes depuis l'epoch.
    pub started: Option<u64>,
}

impl Session {
    pub fn up(&self) -> bool {
        self.ip.is_some()
    }
    pub fn label(&self) -> String {
        self.vpn.clone().unwrap_or_else(|| self.host.clone())
    }
}

/// Interface réseau d'une connexion (15 caractères au plus).
pub fn iface_name(name: &str) -> String {
    let s: String = name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).take(11).collect();
    format!("vpn-{s}")
}

pub fn log_path(name: &str) -> PathBuf {
    config::runtime_dir().join(format!("{}.log", iface_name(name)))
}

pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| p.is_file())
}

fn bare_host(h: &str) -> &str {
    let h = h.strip_prefix("https://").unwrap_or(h);
    h.trim_end_matches('/')
}

/// (utilisateur, interface, serveur) d'une ligne de commande openconnect.
fn parse_args(args: &[String]) -> (String, Option<String>, String) {
    let (mut user, mut iface, mut host) = (String::new(), None, String::new());
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        if let Some(v) = a.strip_prefix("--user=") {
            user = v.into();
        } else if a == "--user" || a == "-u" {
            user = args.get(i + 1).cloned().unwrap_or_default();
            i += 1;
        } else if let Some(v) = a.strip_prefix("--interface=") {
            iface = Some(v.into());
        } else if a == "--interface" || a == "-i" {
            iface = args.get(i + 1).cloned();
            i += 1;
        } else if !a.starts_with('-') {
            // le serveur vient en dernier
            host = bare_host(a).to_string();
        }
        i += 1;
    }
    (user, iface, host)
}

/// IPv4 par interface.
fn ipv4_addrs() -> HashMap<String, String> {
    let mut map = HashMap::new();
    unsafe {
        let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifap) != 0 {
            return map;
        }
        let mut p = ifap;
        while !p.is_null() {
            let ifa = &*p;
            if !ifa.ifa_addr.is_null() && (*ifa.ifa_addr).sa_family as i32 == libc::AF_INET {
                let sin = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                let ip = std::net::Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
                let name = std::ffi::CStr::from_ptr(ifa.ifa_name).to_string_lossy().into_owned();
                map.entry(name).or_insert_with(|| ip.to_string());
            }
            p = ifa.ifa_next;
        }
        libc::freeifaddrs(ifap);
    }
    map
}

fn start_time(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // champs après « (comm) » : le 22e (starttime) est le 20e de la suite
    let ticks: u64 = stat[stat.rfind(')')? + 2..].split_whitespace().nth(19)?.parse().ok()?;
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
    let btime: u64 = std::fs::read_to_string("/proc/stat")
        .ok()?
        .lines()
        .find_map(|l| l.strip_prefix("btime ").and_then(|v| v.trim().parse().ok()))?;
    Some(btime + ticks / hz)
}

fn is_openconnect(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| c.trim() == "openconnect")
}

/// Processus openconnect en cours, rapprochés de la config.
pub fn sessions(cfg: &Config) -> Vec<Session> {
    let ips = ipv4_addrs();
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir("/proc") else { return out };
    for e in rd.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        if !is_openconnect(pid) {
            continue;
        }
        let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) else { continue };
        let args: Vec<String> =
            raw.split(|b| *b == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
        let (user, iface, host) = parse_args(&args);
        let vpn = cfg
            .vpns
            .iter()
            .find(|v| iface.as_deref() == Some(iface_name(&v.name).as_str()))
            .or_else(|| cfg.vpns.iter().find(|v| bare_host(&v.host) == host && (v.user.is_empty() || v.user == user)))
            .map(|v| v.name.clone());
        // sans --interface (lancé ailleurs) : la première interface tun
        let iface = iface.or_else(|| ips.keys().filter(|k| k.starts_with("tun")).min().cloned());
        let ip = iface.as_ref().and_then(|i| ips.get(i).cloned());
        out.push(Session { pid, vpn, host, user, iface, ip, started: start_time(pid) });
    }
    // pendant la mise en tâche de fond, le parent et le fils coexistent : on garde le fils
    out.sort_by_key(|s| std::cmp::Reverse(s.pid));
    let mut seen = HashSet::new();
    out.retain(|s| seen.insert((s.vpn.clone(), s.host.clone(), s.user.clone())));
    out.sort_by_key(|s| s.pid);
    out
}

/// « 12 min », « 1 h 05 ».
pub fn since(started: Option<u64>) -> Option<String> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let m = now.saturating_sub(started?) / 60;
    Some(match m {
        0 => "moins d'une minute".into(),
        m if m < 60 => format!("{m} min"),
        m => format!("{} h {:02}", m / 60, m % 60),
    })
}

// ---------------------------------------------------------------- sudo

/// `sudo` sans terminal, dans une nouvelle session. Avec mot de passe : lu sur l'entrée
/// standard (première ligne ; `-k` : toujours demandé, donc toujours lu).
fn sudo(password: Option<&str>) -> Command {
    let mut c = Command::new("sudo");
    match password {
        Some(_) => c.args(["-S", "-k", "-p", ""]),
        None => c.arg("-n"),
    };
    c.arg("--");
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    c
}

/// sudo passe-t-il sans mot de passe (NOPASSWD) ?
pub fn sudo_nopasswd() -> bool {
    sudo(None)
        .arg("true")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Vérifie le mot de passe sudo (avant de le faire suivre du mot de passe VPN :
/// un mot de passe faux ferait lire le suivant à sudo).
pub fn sudo_check(password: &str) -> Result<(), String> {
    let mut child = sudo(Some(password))
        .arg("true")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("sudo : {e}"))?;
    if let Some(mut s) = child.stdin.take() {
        let _ = writeln!(s, "{password}");
    }
    let out = child.wait_with_output().map_err(|e| format!("sudo : {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    Err(if err.contains("sudoers") { err.trim().to_string() } else { "Mot de passe sudo incorrect.".into() })
}

// ---------------------------------------------------------------- connexion

pub fn openconnect_args(v: &Vpn) -> Vec<String> {
    let mut a = vec![format!("--protocol={}", v.protocol)];
    if !v.authgroup.is_empty() {
        a.push(format!("--authgroup={}", v.authgroup));
    }
    if !v.user.is_empty() {
        a.push(format!("--user={}", v.user));
    }
    a.push("--passwd-on-stdin".into());
    // rien ne pourra répondre à une autre question : échouer plutôt qu'attendre
    a.push("--non-inter".into());
    a.push("--background".into());
    a.push(format!("--interface={}", iface_name(&v.name)));
    a.extend(v.extra_args.iter().cloned());
    a
}

/// Dernière ligne parlante du journal.
fn last_error(log: &std::path::Path) -> Option<String> {
    let t = std::fs::read_to_string(log).ok()?;
    let lines: Vec<&str> = t.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let bad = |l: &&str| {
        let l = l.to_lowercase();
        ["fail", "error", "erreur", "échec", "denied", "refused", "incorrect", "invalid", "unable"].iter().any(|w| l.contains(w))
    };
    lines.iter().rev().find(|l| bad(l)).or(lines.last()).map(|l| l.to_string())
}

/// Lance openconnect et attend qu'il soit connecté (il passe alors en tâche de fond).
pub fn connect(v: &Vpn, password: &str, sudo_pw: Option<&str>) -> Result<(), String> {
    let oc = which("openconnect").ok_or("openconnect introuvable dans le PATH")?;
    let dir = config::runtime_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{} : {e}", dir.display()))?;
    let log_path = log_path(&v.name);
    let log = std::fs::File::create(&log_path).map_err(|e| format!("{} : {e}", log_path.display()))?;
    let log2 = log.try_clone().map_err(|e| e.to_string())?;
    let mut child = sudo(sudo_pw)
        .arg(oc)
        .args(openconnect_args(v))
        .arg(&v.host)
        .stdin(Stdio::piped())
        .stdout(log)
        .stderr(log2)
        .spawn()
        .map_err(|e| format!("sudo : {e}"))?;
    if let Some(mut s) = child.stdin.take() {
        let mut input = String::new();
        if let Some(p) = sudo_pw {
            input.push_str(p);
            input.push('\n');
        }
        input.push_str(password);
        input.push('\n');
        let _ = s.write_all(input.as_bytes());
    }
    let st = child.wait().map_err(|e| e.to_string())?;
    if st.success() { Ok(()) } else { Err(last_error(&log_path).unwrap_or_else(|| format!("échec d'openconnect ({st})"))) }
}

fn signal(pids: &[u32], sig: &str, sudo_pw: Option<&str>) -> Result<(), String> {
    let kill = which("kill").map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "kill".into());
    let mut child = sudo(sudo_pw)
        .arg(kill)
        .arg(format!("-{sig}"))
        .args(pids.iter().map(|p| p.to_string()))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("sudo : {e}"))?;
    if let (Some(mut s), Some(p)) = (child.stdin.take(), sudo_pw) {
        let _ = writeln!(s, "{p}");
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    // « No such process » : déjà parti entre-temps
    if out.status.success() || pids.iter().all(|p| !is_openconnect(*p)) {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn wait_gone(pids: &[u32], timeout: Duration) -> bool {
    let t = Instant::now();
    while t.elapsed() < timeout {
        if pids.iter().all(|p| !is_openconnect(*p)) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    false
}

/// SIGINT : openconnect se déconnecte proprement (logout) ; SIGTERM s'il traîne.
pub fn disconnect(pids: &[u32], sudo_pw: Option<&str>) -> Result<(), String> {
    signal(pids, "INT", sudo_pw)?;
    if wait_gone(pids, Duration::from_secs(8)) {
        return Ok(());
    }
    signal(pids, "TERM", sudo_pw)?;
    if wait_gone(pids, Duration::from_secs(3)) { Ok(()) } else { Err("openconnect ne s'arrête pas.".into()) }
}

/// `myvpnmanager disconnect` : sudo demande son mot de passe dans le terminal.
pub fn disconnect_interactive(pids: &[u32]) -> Result<(), String> {
    let st = Command::new("sudo")
        .arg("kill")
        .arg("-INT")
        .args(pids.iter().map(|p| p.to_string()))
        .status()
        .map_err(|e| format!("sudo : {e}"))?;
    if !st.success() {
        return Err(format!("sudo kill : {st}"));
    }
    if wait_gone(pids, Duration::from_secs(8)) { Ok(()) } else { Err("openconnect ne s'arrête pas.".into()) }
}

/// Première ligne de la sortie de `password-command`.
pub fn password_from_command(cmd: &str) -> Result<String, String> {
    let out = Command::new("sh").args(["-c", cmd]).stdin(Stdio::null()).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("password-command : {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let s = String::from_utf8_lossy(&out.stdout);
    match s.lines().next() {
        Some(l) if !l.is_empty() => Ok(l.to_string()),
        _ => Err("password-command n'a rien affiché.".into()),
    }
}
