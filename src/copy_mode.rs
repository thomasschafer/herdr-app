//! Copy mode (`terminal.copy_mode`): the keyboard moves a cursor over the
//! terminal's text, scrollback included, selects and yanks it, as Herdr's
//! copy mode and vim do. While it's on, keys are copy mode's (the
//! `CopyMode` key context), not the program's.
//!
//! Positions are libghostty pins, so output that scrolls or prunes the
//! scrollback while copy mode is on moves them with the text.

use crate::vt::{Pin, ScreenPoint, Scroll, Search, Vt};
use gpui::Keystroke;

/// What a key did.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Stay,
    /// Leave copy mode.
    Exit,
    /// Copy the selection (else the search match under the cursor), then leave.
    Yank,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visual {
    /// `v`: from the anchor to the cursor.
    Chars,
    /// `V`: whole rows from the anchor's to the cursor's.
    Lines,
}

/// A search being typed after `/` or `?`.
struct Prompt {
    forward: bool,
    query: String,
}

pub struct CopyMode {
    cursor: Pin,
    visual: Option<(Visual, Pin)>,
    /// The last search (`n` repeats it, `N` the other way).
    query: String,
    forward: bool,
    prompt: Option<Prompt>,
    /// The search found nothing.
    missed: bool,
    search: Search,
    /// The viewport's first row on entry; None: it followed the output.
    entry_top: Option<u32>,
}

/// What copy mode shows over the viewport, in viewport cells.
#[derive(Debug, Default, PartialEq)]
pub struct View {
    /// The copy cursor (column, row), when it's in view.
    pub cursor: Option<(u16, u16)>,
    /// Search matches in view: (row, first column, last column).
    pub matches: Vec<(u16, u16, u16)>,
    /// The match at the cursor, as `matches` gives it.
    pub current: Option<(u16, u16, u16)>,
    /// Where copy mode is: its mode, the search being typed, or how the last one went.
    pub status: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Blank,
    Punct,
    Word,
}

/// Herdr's word separators for `w`, `b` and `e`.
const SEPARATORS: &str = "!\"#$%&'()*+,-./:;<=>?@[\\]^`{|}~";

fn class(c: char, big: bool) -> Class {
    if c.is_whitespace() {
        Class::Blank
    } else if !big && SEPARATORS.contains(c) {
        Class::Punct
    } else {
        Class::Word
    }
}

/// The screen's cells from `from` (included) one way, with a blank between
/// rows the terminal didn't wrap: a line's end ends its word. The second half
/// of a wide character isn't a cell of its own.
fn cells(vt: &Vt, from: ScreenPoint, forward: bool, big: bool) -> impl Iterator<Item = (Option<ScreenPoint>, Class)> + '_ {
    let cols = vt.cols;
    let mut line = vt.line(from.row);
    let (mut row, mut col) = (from.row, from.col as i32);
    let mut pending_break = false;
    std::iter::from_fn(move || {
        loop {
            if pending_break {
                pending_break = false;
                return Some((None, Class::Blank));
            }
            let l = line.as_ref()?;
            if col < 0 {
                row = row.checked_sub(1)?;
                line = vt.line(row);
                col = cols as i32 - 1;
                pending_break = !line.as_ref()?.wraps;
                continue;
            }
            if col >= cols as i32 {
                pending_break = !l.wraps;
                row += 1;
                line = vt.line(row);
                col = 0;
                // The last row: nothing after it, not even a break.
                if line.is_none() {
                    return None;
                }
                continue;
            }
            let here = l.cells.get(col as usize).copied().flatten();
            let at = ScreenPoint { row, col: col as u16 };
            col += if forward { 1 } else { -1 };
            if let Some(c) = here {
                return Some((Some(at), class(c, big)));
            }
        }
    })
}

/// `w` / `W`: the start of the next word.
fn next_word_start(vt: &Vt, from: ScreenPoint, big: bool) -> ScreenPoint {
    let mut it = cells(vt, from, true, big);
    let Some((_, first)) = it.next() else { return from };
    let mut last = from;
    let mut in_first = first != Class::Blank;
    for (at, c) in it {
        if in_first && c == first {
            last = at.unwrap_or(last);
            continue;
        }
        in_first = false;
        match at {
            Some(at) if c != Class::Blank => return at,
            Some(at) => last = at,
            None => {}
        }
    }
    last
}

/// `e` / `E`: the end of this word, or of the next one when already there.
fn next_word_end(vt: &Vt, from: ScreenPoint, big: bool) -> ScreenPoint {
    run_end(cells(vt, from, true, big).skip(1), from)
}

/// `b` / `B`: the start of this word, or of the one before when already there.
fn previous_word_start(vt: &Vt, from: ScreenPoint, big: bool) -> ScreenPoint {
    run_end(cells(vt, from, false, big).skip(1), from)
}

/// Past any blanks, the last cell of the run of one class that follows.
fn run_end(cells: impl Iterator<Item = (Option<ScreenPoint>, Class)>, from: ScreenPoint) -> ScreenPoint {
    let mut run: Option<(Class, ScreenPoint)> = None;
    for (at, c) in cells {
        match (run, at) {
            (None, Some(at)) if c != Class::Blank => run = Some((c, at)),
            (None, _) => {}
            (Some((class, _)), Some(at)) if c == class => run = Some((class, at)),
            (Some(_), _) => break,
        }
    }
    run.map_or(from, |(_, at)| at)
}

/// `$`: the row's last character that isn't blank (0 on a blank row).
fn line_end(vt: &Vt, row: u32) -> u16 {
    vt.line(row).and_then(|l| l.cells.iter().rposition(|c| c.is_some_and(|c| !c.is_whitespace()))).unwrap_or(0) as u16
}

/// `^` / `_`: the row's first character that isn't blank (0 on a blank row).
fn first_non_blank(vt: &Vt, row: u32) -> u16 {
    vt.line(row).and_then(|l| l.cells.iter().position(|c| c.is_some_and(|c| !c.is_whitespace()))).unwrap_or(0) as u16
}

/// `{` / `}`: the next blank row up or down, None when there's none.
fn paragraph(vt: &Vt, row: u32, down: bool) -> Option<u32> {
    let rows = vt.scroll().rows;
    let blank = |r: u32| vt.line(r).is_some_and(|l| l.cells.iter().all(|c| c.is_none_or(char::is_whitespace)));
    if down { (row + 1..rows).find(|r| blank(*r)) } else { (0..row).rev().find(|r| blank(*r)) }
}

/// Lines a page key moves: Herdr's (a screen less two, half a screen).
fn page_lines(height: u32, half: bool) -> u32 {
    if height <= 2 {
        1
    } else if half {
        height / 2
    } else {
        height - 2
    }
}

/// The character a key types, for copy mode's letter keys: None with Ctrl,
/// Alt or Cmd held.
fn typed(k: &Keystroke) -> Option<char> {
    let m = k.modifiers;
    if m.control || m.alt || m.platform || m.function {
        return None;
    }
    let one = |s: &str| {
        let mut c = s.chars();
        c.next().filter(|_| c.next().is_none())
    };
    if k.key == "space" {
        return Some(' ');
    }
    k.key_char.as_deref().and_then(one).filter(|c| !c.is_control()).or_else(|| {
        let c = one(&k.key)?;
        Some(if m.shift { c.to_ascii_uppercase() } else { c })
    })
}

impl CopyMode {
    /// Starts at the program's cursor when it's in view, else the viewport's
    /// bottom-left (as Herdr does).
    pub fn enter(vt: &mut Vt) -> CopyMode {
        let s = vt.scroll();
        let in_view = |p: &ScreenPoint| p.row >= s.top && p.row < s.top + s.height;
        let at = Some(vt.cursor_on_screen()).filter(in_view).unwrap_or(ScreenPoint { row: s.top + s.height - 1, col: 0 });
        let cursor = vt.pin(at).expect("a cell in the viewport is on the screen");
        vt.clear_selection();
        let following = s.top + s.height >= s.rows;
        CopyMode {
            cursor,
            visual: None,
            query: String::new(),
            forward: true,
            prompt: None,
            missed: false,
            search: vt.new_search(),
            entry_top: (!following).then_some(s.top),
        }
    }

    /// The copy cursor; None when the text it was on is gone (copy mode ends).
    pub fn cursor(&self, vt: &Vt) -> Option<ScreenPoint> {
        self.cursor.get(vt)
    }

    pub fn visual(&self) -> Option<Visual> {
        self.visual.as_ref().map(|(v, _)| *v)
    }

    /// Text typed through the OS input method: the search being typed.
    pub fn type_text(&mut self, text: &str) {
        if let Some(p) = &mut self.prompt {
            p.query.push_str(text);
        }
    }

    pub fn handle(&mut self, vt: &mut Vt, k: &Keystroke) -> Outcome {
        let Some(at) = self.cursor(vt) else { return Outcome::Exit };
        if self.prompt.is_some() {
            self.prompt_key(vt, k);
            return Outcome::Stay;
        }
        let s = vt.scroll();
        let m = k.modifiers;
        let moved = |p: ScreenPoint| Some(p);
        let to = match (k.key.as_str(), m.control) {
            ("escape", false) => {
                if self.visual.is_some() || !self.query.is_empty() {
                    self.visual = None;
                    self.query.clear();
                    self.missed = false;
                    vt.clear_selection();
                    return Outcome::Stay;
                }
                return Outcome::Exit;
            }
            ("enter", false) => return Outcome::Yank,
            ("left", false) => moved(ScreenPoint { col: at.col.saturating_sub(1), ..at }),
            ("right", false) => moved(ScreenPoint { col: at.col + 1, ..at }),
            ("up", false) => moved(ScreenPoint { row: at.row.saturating_sub(1), ..at }),
            ("down", false) => moved(ScreenPoint { row: at.row + 1, ..at }),
            ("home", false) => moved(ScreenPoint { col: 0, ..at }),
            ("end", false) => moved(ScreenPoint { col: line_end(vt, at.row), ..at }),
            ("pageup", false) | ("b", true) => return self.page(vt, at, s, false, false),
            ("pagedown", false) | ("f", true) => return self.page(vt, at, s, true, false),
            ("u", true) => return self.page(vt, at, s, false, true),
            ("d", true) => return self.page(vt, at, s, true, true),
            _ => None,
        };
        let to = match to {
            Some(to) => to,
            None => match typed(k) {
                Some('q') => return Outcome::Exit,
                Some('y') => return Outcome::Yank,
                Some('v') | Some(' ') => return self.start_visual(vt, at, Visual::Chars),
                Some('V') => return self.start_visual(vt, at, Visual::Lines),
                Some('h') => ScreenPoint { col: at.col.saturating_sub(1), ..at },
                Some('l') => ScreenPoint { col: at.col + 1, ..at },
                Some('k') => ScreenPoint { row: at.row.saturating_sub(1), ..at },
                Some('j') => ScreenPoint { row: at.row + 1, ..at },
                Some('g') => ScreenPoint { row: 0, ..at },
                Some('G') => ScreenPoint { row: s.rows - 1, ..at },
                Some('0') => ScreenPoint { col: 0, ..at },
                Some('$') => ScreenPoint { col: line_end(vt, at.row), ..at },
                Some('^') | Some('_') => ScreenPoint { col: first_non_blank(vt, at.row), ..at },
                Some('H') => ScreenPoint { row: s.top, ..at },
                Some('M') => ScreenPoint { row: s.top + (s.height - 1) / 2, ..at },
                Some('L') => ScreenPoint { row: s.top + s.height - 1, ..at },
                Some('w') => next_word_start(vt, at, false),
                Some('W') => next_word_start(vt, at, true),
                Some('b') => previous_word_start(vt, at, false),
                Some('B') => previous_word_start(vt, at, true),
                Some('e') => next_word_end(vt, at, false),
                Some('E') => next_word_end(vt, at, true),
                Some('{') => ScreenPoint { row: paragraph(vt, at.row, false).unwrap_or(at.row), ..at },
                Some('}') => ScreenPoint { row: paragraph(vt, at.row, true).unwrap_or(at.row), ..at },
                Some('/') | Some('?') => {
                    self.prompt = Some(Prompt { forward: typed(k) == Some('/'), query: String::new() });
                    return Outcome::Stay;
                }
                Some('n') => return self.repeat(vt, at, false),
                Some('N') => return self.repeat(vt, at, true),
                // Anything else does nothing: it doesn't reach the program.
                _ => return Outcome::Stay,
            },
        };
        self.move_to(vt, to);
        Outcome::Stay
    }

    fn prompt_key(&mut self, vt: &mut Vt, k: &Keystroke) {
        let Some(at) = self.cursor(vt) else { return };
        let p = self.prompt.as_mut().expect("a prompt is open");
        match k.key.as_str() {
            "escape" => self.prompt = None,
            "enter" => {
                let p = self.prompt.take().expect("a prompt is open");
                if !p.query.is_empty() {
                    (self.query, self.forward) = (p.query, p.forward);
                    self.jump(vt, at, self.forward);
                }
            }
            "backspace" if !k.modifiers.platform && !k.modifiers.alt => {
                p.query.pop();
            }
            // Cmd/Alt-backspace, ctrl-u: the whole search.
            "backspace" => p.query.clear(),
            "u" if k.modifiers.control => p.query.clear(),
            _ => {
                if let Some(c) = typed(k) {
                    p.query.push(c);
                }
            }
        }
    }

    fn repeat(&mut self, vt: &mut Vt, at: ScreenPoint, reverse: bool) -> Outcome {
        if !self.query.is_empty() {
            self.jump(vt, at, self.forward != reverse);
        }
        Outcome::Stay
    }

    /// To the next match of the search after the cursor (or before it),
    /// around the end of the screen as vim does.
    fn jump(&mut self, vt: &mut Vt, at: ScreenPoint, forward: bool) {
        let matches = vt.find_all(&mut self.search, &self.query);
        let next = if forward {
            matches.iter().find(|(a, _)| *a > at).or(matches.first())
        } else {
            matches.iter().rev().find(|(a, _)| *a < at).or(matches.last())
        };
        self.missed = next.is_none();
        if let Some(&(start, _)) = next {
            self.move_to(vt, start);
        }
    }

    fn page(&mut self, vt: &mut Vt, at: ScreenPoint, s: Scroll, down: bool, half: bool) -> Outcome {
        let n = page_lines(s.height, half);
        let (row, top) = if down { (at.row + n, s.top + n) } else { (at.row.saturating_sub(n), s.top.saturating_sub(n)) };
        vt.scroll_to_row(top);
        self.move_to(vt, ScreenPoint { row, ..at });
        Outcome::Stay
    }

    fn start_visual(&mut self, vt: &mut Vt, at: ScreenPoint, kind: Visual) -> Outcome {
        let anchor = vt.pin(at).expect("the cursor is on the screen");
        self.visual = Some((kind, anchor));
        self.sync_selection(vt);
        Outcome::Stay
    }

    /// Moves the cursor (kept on the screen and off a wide character's
    /// second half), scrolls it into view and stretches the selection.
    fn move_to(&mut self, vt: &mut Vt, to: ScreenPoint) {
        let s = vt.scroll();
        let row = to.row.min(s.rows - 1);
        let mut col = to.col.min(vt.cols.saturating_sub(1));
        if let Some(line) = vt.line(row) {
            while col > 0 && line.cells.get(col as usize).is_some_and(Option::is_none) {
                col -= 1;
            }
        }
        let to = ScreenPoint { row, col };
        assert!(self.cursor.set(vt, to), "a clamped cell {to:?} is on the screen");
        let s = vt.scroll();
        if row < s.top {
            vt.scroll_to_row(row);
        } else if row >= s.top + s.height {
            vt.scroll_to_row(row + 1 - s.height);
        }
        self.sync_selection(vt);
    }

    /// A click: the cursor goes to that cell of the viewport.
    pub fn click(&mut self, vt: &mut Vt, col: u16, row: u16) {
        let top = vt.scroll().top;
        self.move_to(vt, ScreenPoint { row: top + row as u32, col });
    }

    fn sync_selection(&self, vt: &mut Vt) {
        let (Some((kind, anchor)), Some(at)) = (&self.visual, self.cursor(vt)) else { return };
        let Some(from) = anchor.get(vt) else { return vt.clear_selection() };
        match kind {
            Visual::Chars => vt.select_between(from, at),
            Visual::Lines => {
                let last = vt.cols.saturating_sub(1);
                let (a, b) = (from.row.min(at.row), from.row.max(at.row));
                vt.select_between(ScreenPoint { row: a, col: 0 }, ScreenPoint { row: b, col: last });
            }
        }
    }

    /// Ends copy mode: the viewport goes back where it was, the selection
    /// goes. Returns what `yank` copies: the selection, else the search
    /// match the cursor is on.
    pub fn finish(mut self, vt: &mut Vt, yank: bool) -> Option<String> {
        let text = yank.then(|| self.yanked(vt)).flatten();
        vt.clear_selection();
        match self.entry_top {
            Some(top) => vt.scroll_to_row(top),
            None => vt.scroll_to_bottom(),
        }
        text
    }

    fn yanked(&mut self, vt: &mut Vt) -> Option<String> {
        if self.visual.is_some() {
            return vt.selection_text();
        }
        let at = self.cursor(vt)?;
        let &(a, b) = vt.find_all(&mut self.search, &self.query).iter().find(|(a, _)| *a == at)?;
        vt.select_between(a, b);
        vt.selection_text()
    }

    /// What to draw over the viewport now.
    pub fn view(&mut self, vt: &mut Vt) -> View {
        let s = vt.scroll();
        let in_view = |p: ScreenPoint| (p.row >= s.top && p.row < s.top + s.height).then(|| (p.col, (p.row - s.top) as u16));
        let at = self.cursor(vt);
        let cursor = at.and_then(in_view);
        let mut matches = Vec::new();
        let mut current = None;
        if !self.query.is_empty() {
            for (a, b) in vt.find_in_view(&mut self.search, &self.query) {
                // One span per row the match covers.
                for row in a.row.max(s.top)..=b.row.min(s.top + s.height - 1) {
                    let x0 = if row == a.row { a.col } else { 0 };
                    let x1 = if row == b.row { b.col } else { vt.cols - 1 };
                    let span = ((row - s.top) as u16, x0, x1);
                    if Some(a) == at && current.is_none() {
                        current = Some(span);
                    }
                    matches.push(span);
                }
            }
        }
        let mode = match self.visual() {
            None => "COPY",
            Some(Visual::Chars) => "VISUAL",
            Some(Visual::Lines) => "VISUAL LINE",
        };
        let status = match &self.prompt {
            Some(p) => format!("{}{}", if p.forward { '/' } else { '?' }, p.query),
            None if self.missed => format!("{mode}  no match: {}", self.query),
            None if !self.query.is_empty() => format!("{mode}  {}{}", if self.forward { '/' } else { '?' }, self.query),
            None => mode.to_owned(),
        };
        View { cursor, matches, current, status }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(spec: &str) -> Keystroke {
        // "shift-g" style; a lone "-" is the minus key.
        let mut parts: Vec<&str> = spec.split('-').collect();
        let last = parts.pop().unwrap();
        let key = if last.is_empty() { "-" } else { last };
        let has = |m: &str| parts.contains(&m);
        let shift = has("shift");
        let typed = (key.chars().count() == 1 && !has("ctrl")).then(|| if shift { key.to_uppercase() } else { key.to_owned() });
        Keystroke {
            modifiers: gpui::Modifiers { control: has("ctrl"), alt: false, shift, platform: false, function: false },
            key: key.into(),
            key_char: typed,
        }
    }

    /// A terminal of `rows` rows showing `text`, in copy mode.
    fn copying(cols: u16, rows: u16, text: &str) -> (Vt, CopyMode) {
        let mut vt = Vt::new(cols, rows);
        vt.write(text.replace('\n', "\r\n").as_bytes());
        let c = CopyMode::enter(&mut vt);
        (vt, c)
    }

    fn press(vt: &mut Vt, c: &mut CopyMode, keys: &str) -> Outcome {
        let mut last = Outcome::Stay;
        for k in keys.split(' ') {
            last = c.handle(vt, &key(k));
        }
        last
    }

    fn at(vt: &Vt, c: &CopyMode) -> (u32, u16) {
        let p = c.cursor(vt).unwrap();
        (p.row, p.col)
    }

    #[test]
    fn it_starts_at_the_programs_cursor() {
        let (vt, c) = copying(20, 4, "one\ntwo\n$ ls");
        assert_eq!(at(&vt, &c), (2, 4));
    }

    #[test]
    fn hjkl_and_arrows_move_and_stay_on_the_screen() {
        let (mut vt, mut c) = copying(10, 4, "abc\ndef\nghi\n");
        assert_eq!(at(&vt, &c), (3, 0));
        press(&mut vt, &mut c, "k k l l");
        assert_eq!(at(&vt, &c), (1, 2));
        press(&mut vt, &mut c, "up up up h left left left");
        assert_eq!(at(&vt, &c), (0, 0), "the top-left corner holds");
        press(&mut vt, &mut c, "j j j j j down right");
        assert_eq!(at(&vt, &c), (3, 1), "the last row holds");
        for _ in 0..20 {
            press(&mut vt, &mut c, "l");
        }
        assert_eq!(at(&vt, &c), (3, 9), "the last column holds");
    }

    #[test]
    fn line_motions_find_the_text_on_the_row() {
        let (mut vt, mut c) = copying(20, 3, "   indented text  \n");
        press(&mut vt, &mut c, "k");
        press(&mut vt, &mut c, "shift-$");
        assert_eq!(at(&vt, &c), (0, 15));
        press(&mut vt, &mut c, "0");
        assert_eq!(at(&vt, &c), (0, 0));
        press(&mut vt, &mut c, "shift-^");
        assert_eq!(at(&vt, &c), (0, 3));
        press(&mut vt, &mut c, "end home shift-_");
        assert_eq!(at(&vt, &c), (0, 3), "_ is ^ as in Herdr");
    }

    #[test]
    fn word_motions_are_vims() {
        let (mut vt, mut c) = copying(40, 3, "foo.bar baz-qux\n  next line\n");
        press(&mut vt, &mut c, "k k 0");
        let mut stops = Vec::new();
        for _ in 0..7 {
            press(&mut vt, &mut c, "w");
            stops.push(at(&vt, &c));
        }
        // foo . bar baz - qux | next line
        assert_eq!(stops, [(0, 3), (0, 4), (0, 8), (0, 11), (0, 12), (1, 2), (1, 7)]);
        press(&mut vt, &mut c, "k 0 shift-w");
        assert_eq!(at(&vt, &c), (0, 8), "W skips punctuation");
        press(&mut vt, &mut c, "shift-w");
        assert_eq!(at(&vt, &c), (1, 2), "and goes on to the next line");
        press(&mut vt, &mut c, "b");
        assert_eq!(at(&vt, &c), (0, 12), "a line's end ends its word");
        press(&mut vt, &mut c, "b b");
        assert_eq!(at(&vt, &c), (0, 8));
        press(&mut vt, &mut c, "shift-b");
        assert_eq!(at(&vt, &c), (0, 0));
        press(&mut vt, &mut c, "e");
        assert_eq!(at(&vt, &c), (0, 2));
        press(&mut vt, &mut c, "e");
        assert_eq!(at(&vt, &c), (0, 3), "the dot is a word of its own");
        press(&mut vt, &mut c, "shift-e");
        assert_eq!(at(&vt, &c), (0, 6));
        press(&mut vt, &mut c, "shift-e shift-e");
        assert_eq!(at(&vt, &c), (1, 5), "across the line's end");
    }

    #[test]
    fn words_continue_across_rows_the_terminal_wrapped() {
        // "abcdefghij" wraps after 8 columns: still one word.
        let (mut vt, mut c) = copying(8, 4, "x abcdefghij y");
        press(&mut vt, &mut c, "k 0 w");
        assert_eq!(at(&vt, &c), (0, 2));
        press(&mut vt, &mut c, "e");
        assert_eq!(at(&vt, &c), (1, 3));
        press(&mut vt, &mut c, "w");
        assert_eq!(at(&vt, &c), (1, 5));
    }

    #[test]
    fn wide_characters_are_one_stop() {
        let (mut vt, mut c) = copying(20, 2, "a\u{4e2d} b");
        press(&mut vt, &mut c, "0 l l");
        assert_eq!(at(&vt, &c), (0, 1), "the wide character's second half is no stop");
        press(&mut vt, &mut c, "w");
        assert_eq!(at(&vt, &c), (0, 4));
        press(&mut vt, &mut c, "h");
        assert_eq!(at(&vt, &c), (0, 3));
        press(&mut vt, &mut c, "h");
        assert_eq!(at(&vt, &c), (0, 1), "moving left lands on its first half");
    }

    #[test]
    fn paragraphs_go_to_blank_lines() {
        let (mut vt, mut c) = copying(20, 8, "a\nb\n\nc\nd\n\ne");
        press(&mut vt, &mut c, "g");
        assert_eq!(at(&vt, &c), (0, 1), "g keeps the column, as in Herdr");
        press(&mut vt, &mut c, "0 shift-}");
        assert_eq!(at(&vt, &c), (2, 0));
        press(&mut vt, &mut c, "shift-}");
        assert_eq!(at(&vt, &c), (5, 0));
        press(&mut vt, &mut c, "shift-{ shift-{");
        assert_eq!(at(&vt, &c), (2, 0));
        press(&mut vt, &mut c, "shift-{");
        assert_eq!(at(&vt, &c), (2, 0), "none above: it stays");
    }

    /// 100 numbered lines on a 10-row screen: 91 rows of scrollback.
    fn numbered() -> (Vt, CopyMode) {
        let text: String = (0..100).map(|i| format!("line {i:03}\n")).collect();
        copying(20, 10, &text)
    }

    #[test]
    fn g_and_shift_g_and_pages_move_through_the_scrollback() {
        let (mut vt, mut c) = numbered();
        let s = vt.scroll();
        assert_eq!((s.rows, s.top, s.height), (101, 91, 10));
        press(&mut vt, &mut c, "g");
        assert_eq!((at(&vt, &c), vt.scroll().top), ((0, 0), 0), "the oldest line, in view");
        press(&mut vt, &mut c, "ctrl-f");
        assert_eq!((at(&vt, &c).0, vt.scroll().top), (8, 8), "a page is the screen less two");
        press(&mut vt, &mut c, "ctrl-d");
        assert_eq!((at(&vt, &c).0, vt.scroll().top), (13, 13), "half a screen");
        press(&mut vt, &mut c, "ctrl-u pageup");
        assert_eq!((at(&vt, &c).0, vt.scroll().top), (0, 0));
        press(&mut vt, &mut c, "pagedown shift-g");
        assert_eq!((at(&vt, &c).0, vt.scroll().top), (100, 91));
        press(&mut vt, &mut c, "ctrl-b");
        assert_eq!((at(&vt, &c).0, vt.scroll().top), (92, 83));
    }

    #[test]
    fn h_m_l_go_to_the_viewports_top_middle_and_bottom() {
        let (mut vt, mut c) = numbered();
        press(&mut vt, &mut c, "g ctrl-d");
        let top = vt.scroll().top;
        press(&mut vt, &mut c, "shift-h");
        assert_eq!(at(&vt, &c).0, top);
        press(&mut vt, &mut c, "shift-m");
        assert_eq!(at(&vt, &c).0, top + 4);
        press(&mut vt, &mut c, "shift-l");
        assert_eq!(at(&vt, &c).0, top + 9);
        assert_eq!(vt.scroll().top, top, "none of them scroll");
    }

    #[test]
    fn moving_off_screen_scrolls_just_enough() {
        let (mut vt, mut c) = numbered();
        press(&mut vt, &mut c, "shift-h k");
        assert_eq!(vt.scroll().top, 90);
        press(&mut vt, &mut c, "shift-l j");
        assert_eq!(vt.scroll().top, 91);
    }

    #[test]
    fn visual_selections_yank_their_text() {
        let (mut vt, mut c) = copying(30, 4, "hello world\nsecond line\n");
        press(&mut vt, &mut c, "k k 0 w v e");
        assert_eq!(c.visual(), Some(Visual::Chars));
        assert_eq!(vt.selection_text().as_deref(), Some("world"), "the selection shows as it grows");
        assert_eq!(press(&mut vt, &mut c, "y"), Outcome::Yank);
        assert_eq!(c.finish(&mut vt, true).as_deref(), Some("world"));
        assert_eq!(vt.selection_text(), None, "leaving clears it");

        let mut c = CopyMode::enter(&mut vt);
        press(&mut vt, &mut c, "k k l shift-v j");
        assert_eq!(c.visual(), Some(Visual::Lines));
        assert_eq!(press(&mut vt, &mut c, "enter"), Outcome::Yank);
        assert_eq!(c.finish(&mut vt, true).as_deref(), Some("hello world\nsecond line"));

        let mut c = CopyMode::enter(&mut vt);
        press(&mut vt, &mut c, "k k space l");
        assert_eq!(vt.selection_text().as_deref(), Some("he"), "space starts one too");
        press(&mut vt, &mut c, "h h");
        assert_eq!(vt.selection_text().as_deref(), Some("h"), "back past the anchor");
        assert_eq!(c.finish(&mut vt, false), None, "leaving without yanking copies nothing");
    }

    #[test]
    fn escape_clears_the_selection_then_leaves_and_q_leaves() {
        let (mut vt, mut c) = copying(20, 3, "abc");
        press(&mut vt, &mut c, "v l");
        assert_eq!(press(&mut vt, &mut c, "escape"), Outcome::Stay);
        assert_eq!((c.visual(), vt.selection_text()), (None, None));
        assert_eq!(press(&mut vt, &mut c, "escape"), Outcome::Exit);
        assert_eq!(press(&mut vt, &mut c, "q"), Outcome::Exit);
        assert_eq!(press(&mut vt, &mut c, "x"), Outcome::Stay, "other keys do nothing");
    }

    #[test]
    fn search_finds_forward_and_back_and_wraps_around() {
        let (mut vt, mut c) = copying(30, 6, "an apple\nApple pie\nno fruit\nan apple tart\n");
        press(&mut vt, &mut c, "g");
        for k in "/ a p p l e".split(' ') {
            c.handle(&mut vt, &key(k));
        }
        assert_eq!(c.view(&mut vt).status, "/apple");
        press(&mut vt, &mut c, "enter");
        assert_eq!(at(&vt, &c), (0, 3), "after the cursor");
        press(&mut vt, &mut c, "n");
        assert_eq!(at(&vt, &c), (1, 0), "any case without a capital");
        press(&mut vt, &mut c, "n n");
        assert_eq!(at(&vt, &c), (0, 3), "around the end");
        press(&mut vt, &mut c, "shift-n");
        assert_eq!(at(&vt, &c), (3, 3), "N goes back, around the start");
        let view = c.view(&mut vt);
        assert_eq!(view.matches, [(0, 3, 7), (1, 0, 4), (3, 3, 7)]);
        assert_eq!(view.current, Some((3, 3, 7)));
        assert_eq!(view.cursor, Some((3, 3)));
        // y with no selection copies the match under the cursor.
        assert_eq!(press(&mut vt, &mut c, "y"), Outcome::Yank);
        assert_eq!(c.finish(&mut vt, true).as_deref(), Some("apple"));
    }

    #[test]
    fn a_capital_makes_the_search_exact_and_question_mark_searches_back() {
        let (mut vt, mut c) = copying(30, 6, "an apple\nApple pie\nno fruit\nan apple tart\n");
        for k in "shift-? shift-a p p x backspace l e enter".split(' ') {
            c.handle(&mut vt, &key(k));
        }
        assert_eq!(at(&vt, &c), (1, 0), "only the capital one, before the cursor");
        press(&mut vt, &mut c, "n");
        assert_eq!(at(&vt, &c), (1, 0), "it's the only one");
        for k in "/ z z z enter".split(' ') {
            c.handle(&mut vt, &key(k));
        }
        assert_eq!(c.view(&mut vt).status, "COPY  no match: zzz");
        assert_eq!(at(&vt, &c), (1, 0));
        press(&mut vt, &mut c, "escape");
        assert_eq!(c.view(&mut vt).status, "COPY", "escape clears the search first");
        // Escape in the prompt drops what was typed.
        for k in "/ a escape".split(' ') {
            c.handle(&mut vt, &key(k));
        }
        assert_eq!(c.view(&mut vt).status, "COPY");
    }

    #[test]
    fn the_cursor_follows_its_text_as_output_prunes_the_scrollback() {
        let mut vt = Vt::new(20, 5);
        vt.write(b"\x1b[3J");
        for i in 0..crate::vt::SCROLLBACK_MAX_LINES + 50 {
            vt.write(format!("row {i}\r\n").as_bytes());
        }
        let mut c = CopyMode::enter(&mut vt);
        press(&mut vt, &mut c, "k k k");
        let before = c.cursor(&vt).unwrap();
        let line = |vt: &Vt, p: ScreenPoint| vt.line(p.row).unwrap().cells.iter().flatten().collect::<String>();
        let text = line(&vt, before);
        for i in 0..200 {
            vt.write(format!("more {i}\r\n").as_bytes());
        }
        let after = c.cursor(&vt).expect("its line is still kept");
        assert_eq!(line(&vt, after), text, "on the same text");
    }

    #[test]
    fn leaving_puts_the_viewport_back() {
        let (mut vt, mut c) = numbered();
        press(&mut vt, &mut c, "g");
        assert_eq!(vt.scroll().top, 0);
        c.finish(&mut vt, false);
        assert_eq!(vt.scroll().top, 91, "it followed the output: back to the bottom");
        vt.scroll_to_row(40);
        let mut c = CopyMode::enter(&mut vt);
        assert_eq!(at(&vt, &c), (49, 0), "the program's cursor is out of view: the bottom-left");
        press(&mut vt, &mut c, "shift-g");
        c.finish(&mut vt, false);
        assert_eq!(vt.scroll().top, 40);
    }
}
