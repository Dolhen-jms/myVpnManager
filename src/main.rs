//! myVpnManager — client openconnect en terminal, à la nmtui.

mod app;
mod bar;
mod config;
mod crypto;
mod input;
mod theme;
mod ui;
mod vpn;

use app::{App, Msg};
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind};
use ratatui::crossterm::execute;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const HELP: &str = "\
usage : myvpnmanager [commande]

  (sans commande)        interface (le VPN reste connecté quand on la quitte)
  status                 état des connexions
  status --json          état pour la barre wayle
  status --watch         pareil, une ligne JSON à chaque changement
  open                   ouvre l'interface dans un terminal (ou la met au premier plan)
  disconnect             coupe le VPN actif

configuration : ~/.config/myvpnmanager/config.toml";

fn main() {
    config::migrate_old_dir();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => tui(),
        Some("status") => bar::status(&args[1..]),
        Some("open") => bar::open(),
        Some("disconnect" | "down") => disconnect(),
        Some("-h" | "--help" | "help") => println!("{HELP}"),
        Some(x) => {
            eprintln!("myvpnmanager: commande inconnue : {x}\n\n{HELP}");
            std::process::exit(2);
        }
    }
}

fn disconnect() {
    let cfg = config::load().unwrap_or_default();
    let pids: Vec<u32> = vpn::sessions(&cfg).iter().map(|s| s.pid).collect();
    if pids.is_empty() {
        println!("Aucun VPN actif.");
        return;
    }
    match vpn::disconnect_interactive(&pids) {
        Ok(()) => println!("VPN déconnecté."),
        Err(e) => {
            eprintln!("myvpnmanager: {e}");
            std::process::exit(1);
        }
    }
}

fn tui() {
    theme::load();
    let mut theme_stamp = theme::stamp();
    let mut terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let (tx, rx) = mpsc::channel::<Msg>();
    let mut app = App::new(tx);
    let mut last_tick = Instant::now();
    loop {
        if last_tick.elapsed() >= Duration::from_secs(1) {
            last_tick = Instant::now();
            app.tick();
            let s = theme::stamp();
            if s != theme_stamp {
                theme_stamp = s;
                theme::load();
                let _ = terminal.clear();
            }
        }
        let _ = terminal.draw(|f| ui::draw(f, &mut app));
        if app.quit {
            break;
        }
        if event::poll(Duration::from_millis(100)).unwrap_or(false) {
            match event::read() {
                Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => app.on_key(k),
                Ok(Event::Mouse(m)) => app.on_mouse(m),
                _ => {}
            }
        }
        while let Ok(m) = rx.try_recv() {
            m(&mut app);
        }
    }
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
}
