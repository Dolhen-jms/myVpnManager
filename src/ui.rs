//! Dessin, à la nmtui : connexions et boutons, état, journal ; fenêtres par-dessus.

use crate::app::{App, BUTTONS, Button, BusyKind, FORM_CANCEL, FORM_FIELDS, FORM_PASSWORD, FORM_SAVE, Focus, Modal, Severity};
use crate::theme::{self, Theme};
use crate::vpn;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};

const SPIN: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

fn spin() -> &'static str {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    SPIN[(ms / 100) as usize % SPIN.len()]
}

fn fg(c: Color) -> Style {
    Style::default().fg(c)
}

fn bold(c: Color) -> Style {
    fg(c).add_modifier(Modifier::BOLD)
}

fn frame<'a>(title: &'a str, t: &Theme, focused: bool) -> Block<'a> {
    let c = if focused { t.accent } else { t.muted };
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(fg(c))
        .title(Span::styled(format!(" {title} "), bold(if focused { t.accent } else { t.fg })))
}

/// Ligne sélectionnée : teinte d'accent si la zone a le focus.
fn cursor_style(t: &Theme, active: bool) -> Style {
    match (active, t.cursor, t.cursor_idle) {
        (true, Some(c), _) => Style::default().bg(c),
        (false, _, Some(c)) => Style::default().bg(c),
        (true, None, _) => Style::default().add_modifier(Modifier::REVERSED),
        (false, _, None) => Style::default().add_modifier(Modifier::BOLD),
    }
}

fn button_span(label: &str, focused: bool, t: &Theme) -> Span<'static> {
    let style = if focused { Style::default().fg(t.on_accent).bg(t.accent).add_modifier(Modifier::BOLD) } else { fg(t.fg) };
    Span::styled(format!("<{label}>"), style)
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
}

/// Symbole, couleur et état d'une connexion.
fn vpn_state(app: &App, name: &str, t: &Theme) -> (&'static str, Color, String) {
    let s = app.session(name);
    match &app.busy {
        Some(b) if b.name == name && b.kind == BusyKind::Connecting => {
            return (spin(), t.warn, format!("connexion… {} s", b.since.elapsed().as_secs()));
        }
        Some(b) if b.kind == BusyKind::Disconnecting && (b.name == name || b.name.is_empty()) && s.is_some() => {
            return (spin(), t.warn, "déconnexion…".into());
        }
        _ => {}
    }
    match s {
        Some(s) if s.up() => ("●", t.ok, "connecté".into()),
        Some(_) => (spin(), t.warn, "connexion…".into()),
        None => ("○", t.muted, String::new()),
    }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let t = theme::get();
    app.areas = Default::default();
    let area = f.area();
    let w = area.width.min(112);
    let outer = Rect { x: area.x + (area.width - w) / 2, width: w, ..area };
    let ob = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(fg(t.muted))
        .title(Line::from(Span::styled(" myVpnManager ", bold(t.accent))).centered());
    let inner = ob.inner(outer).inner(Margin { horizontal: 1, vertical: 0 });
    f.render_widget(ob, outer);

    let rows = app.cfg.vpns.len().max(1) as u16;
    let top_h = (rows + 2).max(BUTTONS.len() as u16 + 2).min(inner.height.saturating_sub(8).max(5));
    let [top, state, log, foot] =
        Layout::vertical([Constraint::Length(top_h), Constraint::Length(4), Constraint::Min(3), Constraint::Length(1)]).areas(inner);
    let [list_a, btn_a] = Layout::horizontal([Constraint::Min(20), Constraint::Length(17)]).spacing(1).areas(top);

    draw_list(f, app, list_a, &t);
    draw_buttons(f, app, btn_a, &t);
    draw_state(f, app, state, &t);
    draw_log(f, app, log, &t);
    draw_footer(f, app, foot, &t);

    if !app.modals.is_empty() {
        draw_modal(f, app, &t);
    }
}

fn draw_list(f: &mut Frame, app: &mut App, area: Rect, t: &Theme) {
    let block = frame("Connexions", t, app.focus == Focus::List && app.modals.is_empty());
    let inner = block.inner(area);
    f.render_widget(block, area);
    app.areas.list = inner;
    if let Some(e) = &app.cfg_error {
        let p = Paragraph::new(vec![
            Line::styled("config.toml illisible :", bold(t.err)),
            Line::styled(e.clone(), fg(t.err)),
            Line::styled("corrige le fichier puis r pour le relire", fg(t.muted)),
        ])
        .wrap(Wrap { trim: false });
        f.render_widget(p, inner);
        return;
    }
    if app.cfg.vpns.is_empty() {
        let p = Paragraph::new(vec![
            Line::styled("Aucune connexion.", fg(t.fg)),
            Line::from(vec![Span::styled("a", bold(t.accent)), Span::styled(" pour en ajouter une, ou édite", fg(t.muted))]),
            Line::styled(crate::config::path().display().to_string(), fg(t.muted)),
        ]);
        f.render_widget(p, inner);
        return;
    }
    let h = inner.height as usize;
    let offset = app.selected.saturating_sub(h.saturating_sub(1));
    app.areas.list_offset = offset;
    let name_w = app.cfg.vpns.iter().map(|v| v.name.chars().count()).max().unwrap_or(0);
    for (row, i) in (offset..app.cfg.vpns.len()).take(h).enumerate() {
        let v = &app.cfg.vpns[i];
        let (sym, color, state) = vpn_state(app, &v.name, t);
        let who = match (v.user.is_empty(), v.host.is_empty()) {
            (false, _) => format!("{} · {}", v.user, v.host),
            (true, _) => v.host.clone(),
        };
        let r = Rect { y: inner.y + row as u16, height: 1, ..inner };
        if i == app.selected {
            f.buffer_mut().set_style(r, cursor_style(t, app.focus == Focus::List));
        }
        let mut spans = vec![
            Span::styled(format!(" {sym} "), bold(color)),
            Span::styled(format!("{:<name_w$}  ", v.name), bold(t.fg)),
            Span::styled(who, fg(t.muted)),
        ];
        if !state.is_empty() {
            spans.push(Span::styled(format!("  {state}"), fg(color)));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), r);
    }
}

fn draw_buttons(f: &mut Frame, app: &mut App, area: Rect, t: &Theme) {
    let focused = app.focus == Focus::Buttons && app.modals.is_empty();
    for (i, b) in BUTTONS.iter().enumerate() {
        let y = area.y + 1 + i as u16;
        if y >= area.bottom() {
            break;
        }
        let label = match b {
            Button::Toggle => app.toggle_label(),
            Button::Edit => "Modifier…",
            Button::Add => "Ajouter…",
            Button::Delete => "Supprimer",
            Button::Quit => "Quitter",
        };
        let span = button_span(label, focused && app.button == i, t);
        let w = span.width() as u16;
        let r = Rect { x: area.x + 2, y, width: w.min(area.width.saturating_sub(2)), height: 1 };
        f.render_widget(Paragraph::new(Line::from(span)), r);
        app.areas.buttons.push((r, i));
    }
}

fn draw_state(f: &mut Frame, app: &App, area: Rect, t: &Theme) {
    let block = frame("État", t, false);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let Some(v) = app.current() else {
        let other = app.sessions.first().map(|s| format!("actif hors config : {}", s.host)).unwrap_or_default();
        f.render_widget(Paragraph::new(Line::styled(other, fg(t.warn))), inner);
        return;
    };
    let (sym, color, _) = vpn_state(app, &v.name, t);
    let mut l1 = vec![Span::styled(format!("{sym} "), bold(color))];
    match (app.session(&v.name), &app.busy) {
        (_, Some(b)) if b.name == v.name && b.kind == BusyKind::Connecting => {
            l1.push(Span::styled(format!("Connexion en cours… {} s", b.since.elapsed().as_secs()), bold(t.warn)));
        }
        (Some(s), Some(b)) if b.kind == BusyKind::Disconnecting => {
            let _ = s;
            l1.push(Span::styled("Déconnexion…", bold(t.warn)));
        }
        (Some(s), _) if s.up() => {
            l1.push(Span::styled("Connecté", bold(t.ok)));
            let mut d = Vec::new();
            if let Some(ip) = &s.ip {
                d.push(format!("IP {ip}"));
            }
            if let Some(i) = &s.iface {
                d.push(format!("interface {i}"));
            }
            if let Some(x) = vpn::since(s.started) {
                d.push(format!("depuis {x}"));
            }
            l1.push(Span::styled(format!(" · {}", d.join(" · ")), fg(t.fg)));
        }
        (Some(_), _) => l1.push(Span::styled("Connexion en cours…", bold(t.warn))),
        (None, _) => {
            l1.push(Span::styled("Déconnecté", fg(t.muted)));
            if let Some(o) = app.sessions.iter().find(|s| s.vpn.as_deref() != Some(&v.name)) {
                l1.push(Span::styled(format!(" · actif : {}", o.label()), fg(t.warn)));
            }
        }
    }
    let pw = if !v.password_enc.is_empty() {
        "chiffré"
    } else if !v.password.is_empty() {
        "en clair"
    } else if !v.password_command.is_empty() {
        "par commande"
    } else {
        "demandé"
    };
    let mut d = vec![v.protocol.clone()];
    if !v.authgroup.is_empty() {
        d.push(format!("groupe {}", v.authgroup));
    }
    d.push(format!("mot de passe {pw}"));
    if !v.extra_args.is_empty() {
        d.push(v.extra_args.join(" "));
    }
    let l2 = Line::styled(format!("  {}", d.join(" · ")), fg(t.muted));
    f.render_widget(Paragraph::new(vec![Line::from(l1), l2]), inner);
}

fn draw_log(f: &mut Frame, app: &App, area: Rect, t: &Theme) {
    let title = match app.current() {
        Some(v) => format!("Journal — {}", v.name),
        None => "Journal".into(),
    };
    let block = frame(&title, t, false);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if app.log.is_empty() {
        f.render_widget(Paragraph::new(Line::styled("(rien depuis le démarrage)", fg(t.muted))), inner);
        return;
    }
    let h = inner.height as usize;
    let lines: Vec<Line> = app.log[app.log.len().saturating_sub(h)..]
        .iter()
        .map(|l| {
            let low = l.to_lowercase();
            let c = if ["fail", "error", "denied", "refused", "unable", "invalid"].iter().any(|w| low.contains(w)) {
                t.err
            } else if ["connected", "established", "configured as"].iter().any(|w| low.contains(w)) {
                t.ok
            } else {
                t.muted
            };
            Line::styled(l.clone(), fg(c))
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect, t: &Theme) {
    if let Some((m, sev, _)) = &app.message {
        let c = match sev {
            Severity::Info => t.fg,
            Severity::Ok => t.ok,
            Severity::Error => t.err,
        };
        f.render_widget(Paragraph::new(Line::styled(m.clone(), bold(c))), area);
        return;
    }
    let keys = [("⏎", "connecter/déconnecter"), ("⇥", "boutons"), ("a", "ajouter"), ("e", "modifier"), ("x", "supprimer"), ("?", "aide"), ("q", "quitter")];
    let mut spans = Vec::new();
    for (k, d) in keys {
        spans.push(Span::styled(k, bold(t.accent)));
        spans.push(Span::styled(format!(" {d}  "), fg(t.muted)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ---------------------------------------------------------------- fenêtres

fn modal_frame<'a>(f: &mut Frame, r: Rect, title: &'a str, t: &Theme) -> Rect {
    f.render_widget(Clear, r);
    let mut b = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(fg(t.accent))
        .title(Line::from(Span::styled(format!(" {title} "), bold(t.accent))).centered());
    if let Some(bg) = t.surface {
        b = b.style(Style::default().bg(bg));
    }
    let inner = b.inner(r).inner(Margin { horizontal: 1, vertical: 0 });
    f.render_widget(b, r);
    inner
}

/// Champ de saisie ; place le curseur du terminal s'il a le focus.
fn draw_field(f: &mut Frame, r: Rect, input: &crate::input::Input, mask: bool, focused: bool, t: &Theme) {
    let style = match t.field {
        Some(bg) => Style::default().bg(bg).fg(t.fg),
        None => Style::default().add_modifier(Modifier::UNDERLINED),
    };
    f.buffer_mut().set_style(r, style);
    let (text, cx) = input.view(r.width.saturating_sub(1) as usize, mask);
    f.render_widget(Paragraph::new(Line::styled(text, style)), r);
    if focused {
        f.set_cursor_position(Position::new(r.x + cx as u16, r.y));
    }
}

fn hint(text: &str, t: &Theme) -> Line<'static> {
    Line::styled(text.to_string(), fg(t.muted))
}

fn draw_modal(f: &mut Frame, app: &mut App, t: &Theme) {
    let area = f.area();
    let mut buttons = Vec::new();
    match app.modals.last().unwrap() {
        Modal::Help => {
            let keys = [
                ("⏎ / espace / c", "connecter ou déconnecter (annuler pendant la connexion)"),
                ("double clic", "pareil, à la souris"),
                ("d", "déconnecter le VPN actif"),
                ("⇥ ← →", "passer de la liste aux boutons"),
                ("↑ ↓  j k", "se déplacer"),
                ("a", "ajouter une connexion"),
                ("e", "modifier la connexion"),
                ("x / Suppr", "supprimer la connexion"),
                ("r", "relire config.toml"),
                ("m", "changer le mot de passe maître"),
                ("q / Échap", "quitter : le VPN reste connecté"),
            ];
            let mut lines: Vec<Line> = keys
                .iter()
                .map(|(k, d)| Line::from(vec![Span::styled(format!("{k:>16}  "), bold(t.accent)), Span::styled(*d, fg(t.fg))]))
                .collect();
            lines.push(Line::raw(""));
            lines.push(hint(&format!("config : {}", crate::config::path().display()), t));
            lines.push(hint("formulaire : ⇥ champ suivant · ← → choisit le protocole · Ctrl+S enregistre", t));
            let r = centered(area, 84, lines.len() as u16 + 2);
            let inner = modal_frame(f, r, "Aide", t);
            f.render_widget(Paragraph::new(lines), inner);
        }
        Modal::Prompt(p) => {
            let w = 64u16.min(area.width.saturating_sub(4));
            let text_lines: Vec<Line> = p.text.lines().map(|l| Line::styled(l.to_string(), fg(t.fg))).collect();
            let n = text_lines.len() as u16;
            let h = n + 5 + p.error.is_some() as u16;
            let r = centered(area, w, h);
            let inner = modal_frame(f, r, &p.title, t);
            f.render_widget(Paragraph::new(text_lines), Rect { height: n, ..inner });
            let field = Rect { y: inner.y + n + 1, height: 1, ..inner };
            draw_field(f, field, &p.input, p.mask, true, t);
            let mut y = field.y + 1;
            if let Some(e) = &p.error {
                f.render_widget(Paragraph::new(Line::styled(e.clone(), bold(t.err))), Rect { y, height: 1, ..inner });
                y += 1;
            }
            f.render_widget(Paragraph::new(hint("⏎ valider · Échap annuler", t)), Rect { y, height: 1, ..inner });
        }
        Modal::Confirm(c) => {
            let w = 60u16.min(area.width.saturating_sub(4));
            let r = centered(area, w, 6);
            let inner = modal_frame(f, r, &c.title, t);
            f.render_widget(Paragraph::new(Line::styled(c.text.clone(), fg(t.fg))).wrap(Wrap { trim: true }), Rect { height: 2, ..inner });
            let yes = button_span("Oui", c.yes_focused, t);
            let no = button_span("Non", !c.yes_focused, t);
            let (wy, wn) = (yes.width() as u16, no.width() as u16);
            let x = inner.x + inner.width.saturating_sub(wy + wn + 3) / 2;
            let y = inner.y + 3;
            let ry = Rect { x, y, width: wy, height: 1 };
            let rn = Rect { x: x + wy + 3, y, width: wn, height: 1 };
            f.render_widget(Paragraph::new(Line::from(yes)), ry);
            f.render_widget(Paragraph::new(Line::from(no)), rn);
            buttons.push((ry, 0));
            buttons.push((rn, 1));
        }
        Modal::Form(form) => {
            let w = 84u16.min(area.width.saturating_sub(2));
            let h = FORM_FIELDS.len() as u16 * 2 + 6;
            let r = centered(area, w, h);
            let title = if form.editing.is_some() { "Modifier la connexion" } else { "Nouvelle connexion" };
            let inner = modal_frame(f, r, title, t);
            let label_w = 14u16;
            for (i, (label, _)) in FORM_FIELDS.iter().enumerate() {
                let y = inner.y + 1 + i as u16 * 2;
                let focused = form.focus == i;
                let ls = if focused { bold(t.accent) } else { fg(t.fg) };
                f.render_widget(
                    Paragraph::new(Line::styled(format!("{label:>w$}", w = label_w as usize - 2), ls)),
                    Rect { x: inner.x, y, width: label_w, height: 1 },
                );
                let fr = Rect { x: inner.x + label_w, y, width: inner.width.saturating_sub(label_w), height: 1 };
                draw_field(f, fr, &form.inputs[i], i == FORM_PASSWORD, focused, t);
                buttons.push((Rect { x: inner.x, width: inner.width, ..fr }, i));
            }
            let y = inner.y + 1 + FORM_FIELDS.len() as u16 * 2;
            let line = match (&form.error, FORM_FIELDS.get(form.focus)) {
                (Some(e), _) => Line::styled(e.clone(), bold(t.err)),
                (None, Some((_, h))) => hint(h, t),
                (None, None) => hint("Ctrl+S enregistre · Échap annule", t),
            };
            f.render_widget(Paragraph::new(line).wrap(Wrap { trim: true }), Rect { y, height: 2, ..inner });
            let save = button_span("Enregistrer", form.focus == FORM_SAVE, t);
            let cancel = button_span("Annuler", form.focus == FORM_CANCEL, t);
            let (ws, wc) = (save.width() as u16, cancel.width() as u16);
            let by = y + 2;
            let rc = Rect { x: inner.right().saturating_sub(wc), y: by, width: wc, height: 1 };
            let rs = Rect { x: rc.x.saturating_sub(ws + 2), y: by, width: ws, height: 1 };
            f.render_widget(Paragraph::new(Line::from(save)), rs);
            f.render_widget(Paragraph::new(Line::from(cancel)), rc);
            buttons.push((rs, FORM_SAVE));
            buttons.push((rc, FORM_CANCEL));
        }
    }
    app.areas.modal_buttons = buttons;
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};

    fn dump(app: &mut crate::app::App, w: u16, h: u16) {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| super::draw(f, app)).unwrap();
        let buf = t.backend().buffer().clone();
        for y in 0..h {
            let line: String = (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect();
            println!("{line}");
        }
    }

    #[test]
    fn render() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut app = crate::app::App::new(tx);
        dump(&mut app, 100, 26);
        app.edit();
        dump(&mut app, 100, 26);
    }
}
