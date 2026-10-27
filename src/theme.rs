//! Couleurs du thème du bureau : palette `--palette-*` de
//! `~/.local/state/theme/current/wayle.scss`, la même que la barre ; relue quand on change de
//! thème. Sans ce fichier (autre machine) ou sans couleurs 24 bits : couleurs du terminal.

use ratatui::style::Color;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;
use std::time::SystemTime;

#[derive(Clone, Copy)]
pub struct Theme {
    pub accent: Color,
    pub fg: Color,
    pub muted: Color,
    pub ok: Color,
    pub warn: Color,
    pub err: Color,
    /// Texte sur l'accent (bouton actif).
    pub on_accent: Color,
    /// Fonds ; None : vidéo inverse ou soulignement à la place.
    pub surface: Option<Color>,
    pub field: Option<Color>,
    pub cursor: Option<Color>,
    pub cursor_idle: Option<Color>,
}

const ANSI: Theme = Theme {
    accent: Color::Blue,
    fg: Color::Reset,
    muted: Color::DarkGray,
    ok: Color::Green,
    warn: Color::Yellow,
    err: Color::Red,
    on_accent: Color::Black,
    surface: None,
    field: None,
    cursor: None,
    cursor_idle: None,
};

static CURRENT: RwLock<Theme> = RwLock::new(ANSI);

pub fn get() -> Theme {
    *CURRENT.read().unwrap()
}

fn dir() -> PathBuf {
    crate::config::home_dir().join(".local/state/theme/current")
}

fn truecolor() -> bool {
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    matches!(env("COLORTERM").as_str(), "truecolor" | "24bit")
        || !env("KITTY_WINDOW_ID").is_empty()
        || ["kitty", "ghostty", "wezterm", "alacritty", "foot"].iter().any(|t| env("TERM").contains(t))
}

fn hex(s: &str) -> Option<(u8, u8, u8)> {
    let h = s.strip_prefix('#')?.get(..6)?;
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// `a` posé sur `b` avec l'opacité `t`.
fn mix(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> Color {
    let m = |x: u8, y: u8| (x as f32 * t + y as f32 * (1.0 - t)).round() as u8;
    Color::Rgb(m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

fn from_palette() -> Option<Theme> {
    let text = std::fs::read_to_string(dir().join("wayle.scss")).ok()?;
    let mut pal: HashMap<String, (u8, u8, u8)> = HashMap::new();
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("--palette-") else { continue };
        let Some((k, v)) = rest.split_once(':') else { continue };
        if let Some(c) = hex(v.trim()) {
            pal.insert(k.trim().to_string(), c);
        }
    }
    let get = |k: &str| pal.get(k).copied();
    let rgb = |c: (u8, u8, u8)| Color::Rgb(c.0, c.1, c.2);
    let (bg, primary) = (get("bg")?, get("primary")?);
    let or = |k: &str, d: Color| get(k).map(rgb).unwrap_or(d);
    Some(Theme {
        accent: rgb(primary),
        fg: or("fg", Color::Reset),
        muted: or("fg-muted", Color::DarkGray),
        ok: or("green", Color::Green),
        warn: or("yellow", Color::Yellow),
        err: or("red", Color::Red),
        on_accent: rgb(bg),
        surface: get("surface").map(rgb),
        field: get("elevated").map(rgb),
        cursor: Some(mix(primary, bg, 0.28)),
        cursor_idle: get("elevated").map(rgb),
    })
}

pub fn load() {
    let t = if truecolor() { from_palette() } else { None };
    *CURRENT.write().unwrap() = t.unwrap_or(ANSI);
}

/// Change quand on bascule de thème (`theme-set`) ou qu'on retouche sa palette.
pub fn stamp() -> Option<(PathBuf, Option<SystemTime>)> {
    let cur = dir();
    let target = std::fs::read_link(&cur).ok()?;
    Some((target, std::fs::metadata(cur.join("wayle.scss")).and_then(|m| m.modified()).ok()))
}
