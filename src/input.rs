//! Ligne de saisie : curseur, Ctrl+U / Ctrl+W, défilement horizontal.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Debug, Default)]
pub struct Input {
    pub text: String,
    cursor: usize, // en caractères
}

impl Input {
    pub fn new(s: &str) -> Self {
        Input { text: s.to_string(), cursor: s.chars().count() }
    }

    fn len(&self) -> usize {
        self.text.chars().count()
    }

    fn byte(&self, i: usize) -> usize {
        self.text.char_indices().nth(i).map(|(b, _)| b).unwrap_or(self.text.len())
    }

    /// Édite le texte ; false si la touche ne le concerne pas.
    pub fn key(&mut self, k: KeyEvent) -> bool {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Char('u') if ctrl => {
                let b = self.byte(self.cursor);
                self.text.drain(..b);
                self.cursor = 0;
            }
            KeyCode::Char('w') if ctrl => {
                let chars: Vec<char> = self.text.chars().collect();
                let mut i = self.cursor;
                while i > 0 && chars[i - 1] == ' ' {
                    i -= 1;
                }
                while i > 0 && chars[i - 1] != ' ' {
                    i -= 1;
                }
                let (a, b) = (self.byte(i), self.byte(self.cursor));
                self.text.drain(a..b);
                self.cursor = i;
            }
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.len(),
            KeyCode::Char(c) if !ctrl => {
                let b = self.byte(self.cursor);
                self.text.insert(b, c);
                self.cursor += 1;
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                let b = self.byte(self.cursor);
                self.text.remove(b);
            }
            KeyCode::Delete if self.cursor < self.len() => {
                let b = self.byte(self.cursor);
                self.text.remove(b);
            }
            KeyCode::Backspace | KeyCode::Delete => {}
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.len(),
            _ => return false,
        }
        true
    }

    /// Texte visible dans `width` colonnes, et colonne du curseur.
    pub fn view(&self, width: usize, mask: bool) -> (String, usize) {
        let chars: Vec<char> = if mask { vec!['•'; self.len()] } else { self.text.chars().collect() };
        let width = width.max(1);
        let start = (self.cursor + 1).saturating_sub(width);
        (chars[start..].iter().take(width).collect(), self.cursor - start)
    }
}
