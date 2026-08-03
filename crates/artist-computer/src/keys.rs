//! Parsing key strokes once, for every backend.
//!
//! This exists because each backend previously parsed chords itself and two of
//! the three got it wrong in the same way: `stroke.rsplit('+').next()` takes the
//! base key and **silently discards the modifiers**. On a page that made
//! `ctrl+a` fall through to the single-character path and *insert the letter
//! "a"* into the focused field — a silent wrong action reported as success,
//! which is the exact failure class the whole design exists to eliminate.
//!
//! One parser, one vocabulary, three translations. A backend may still refuse a
//! chord it cannot deliver, but it can no longer quietly drop half of it.

use crate::program::StepError;

/// Modifier state for a chord.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

impl Modifiers {
    pub fn any(&self) -> bool {
        self.ctrl || self.alt || self.shift || self.meta
    }

    /// The CDP `Input.dispatchKeyEvent` modifier bitmask.
    pub fn cdp_bits(&self) -> i64 {
        (self.alt as i64)
            | ((self.ctrl as i64) << 1)
            | ((self.meta as i64) << 2)
            | ((self.shift as i64) << 3)
    }
}

/// A named key, independent of how any backend spells it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Enter,
    Tab,
    Escape,
    Backspace,
    Delete,
    Space,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    CapsLock,
    PrintScreen,
    /// The context-menu key. The keyboard-only way to open what a right click
    /// opens, and the only route to a context menu on a surface whose seat has
    /// no second button.
    Menu,
    /// A function key, `F1` through `F24`.
    ///
    /// One variant rather than twenty-four, because every backend translates
    /// them arithmetically and spelling each out would be twenty-four chances
    /// to mistype a keycode. Out-of-range numbers are refused at parse time.
    Function(u8),
    /// A media key. Absent from a keymap on many machines, but a stage owns its
    /// own keymap and can simply have them.
    Media(MediaKey),
    /// A single printable character.
    Char(char),
}

/// The media keys, as their own vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKey {
    PlayPause,
    Stop,
    Next,
    Previous,
    VolumeUp,
    VolumeDown,
    Mute,
}

/// A parsed key stroke.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chord {
    pub modifiers: Modifiers,
    pub key: Key,
}

/// Parse a stroke like `Enter`, `ctrl+c`, `ctrl+shift+t`.
///
/// Unknown names are a hard error rather than a silent no-op: a model that
/// asked for a key it cannot have must learn that, not believe it happened.
pub fn parse(stroke: &str) -> Result<Chord, StepError> {
    let stroke = stroke.trim();
    if stroke.is_empty() {
        return Err(StepError::Backend("empty key".into()));
    }

    let mut modifiers = Modifiers::default();
    // Split on '+', but the final segment may itself be a literal '+'.
    let parts: Vec<&str> = stroke.split('+').collect();
    let (base, prefix) = match parts.as_slice() {
        // A trailing empty segment means the stroke ended in '+', i.e. the key
        // *is* '+' — `ctrl++` is ctrl plus the plus key.
        [head @ .., "", ""] => ("+", head),
        [head @ .., ""] if parts.len() > 1 => ("+", head),
        [.., last] => (*last, &parts[..parts.len() - 1]),
        [] => return Err(StepError::Backend("empty key".into())),
    };

    for part in prefix {
        match part.trim().to_ascii_lowercase().as_str() {
            "ctrl" | "control" => modifiers.ctrl = true,
            "alt" | "option" => modifiers.alt = true,
            "shift" => modifiers.shift = true,
            "meta" | "super" | "cmd" | "command" | "win" => modifiers.meta = true,
            "" => {}
            other => {
                return Err(StepError::Backend(format!(
                    "unknown modifier {other:?}; use ctrl, alt, shift or meta"
                )));
            }
        }
    }

    let key = match base.to_ascii_lowercase().as_str() {
        "enter" | "return" => Key::Enter,
        "tab" => Key::Tab,
        "escape" | "esc" => Key::Escape,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "space" => Key::Space,
        "up" | "arrowup" => Key::Up,
        "down" | "arrowdown" => Key::Down,
        "left" | "arrowleft" => Key::Left,
        "right" | "arrowright" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" | "pgup" => Key::PageUp,
        "pagedown" | "pgdn" => Key::PageDown,
        "insert" | "ins" => Key::Insert,
        "capslock" => Key::CapsLock,
        "printscreen" | "prtsc" | "sysrq" => Key::PrintScreen,
        "menu" | "contextmenu" | "compose" => Key::Menu,
        "playpause" | "play" => Key::Media(MediaKey::PlayPause),
        "stop" => Key::Media(MediaKey::Stop),
        "next" | "nexttrack" => Key::Media(MediaKey::Next),
        "previous" | "prevtrack" | "prev" => Key::Media(MediaKey::Previous),
        "volumeup" => Key::Media(MediaKey::VolumeUp),
        "volumedown" => Key::Media(MediaKey::VolumeDown),
        "mute" => Key::Media(MediaKey::Mute),
        // `f` followed by digits, and nothing else. Checked before the
        // single-character fallback so `f` alone still types the letter.
        function
            if function.len() >= 2
                && function.starts_with('f')
                && function[1..].chars().all(|digit| digit.is_ascii_digit()) =>
        {
            let number: u8 = function[1..]
                .parse()
                .map_err(|_| StepError::Backend(format!("unknown key {base:?}")))?;
            if !(1..=24).contains(&number) {
                return Err(StepError::Backend(format!(
                    "no function key {base:?}; F1 through F24 exist"
                )));
            }
            Key::Function(number)
        }
        _ => {
            let mut chars = base.chars();
            match (chars.next(), chars.next()) {
                (Some(character), None) => Key::Char(character),
                _ => {
                    return Err(StepError::Backend(format!(
                        "unknown key {base:?}; use a name like Enter, Tab, Escape, Up, \
                         or a single character"
                    )));
                }
            }
        }
    };

    Ok(Chord { modifiers, key })
}

/// Parse a modifiers-only string like `"ctrl"` or `"ctrl+shift"`.
///
/// Separate from [`parse`], which always wants a base key. A modifier held
/// across a click or a drag has no base key by definition, and routing it
/// through the chord parser would require inventing one — which is how
/// `ctrl+click` would end up also typing a character.
///
/// `None` is no modifiers, which is the overwhelmingly common case and costs
/// nothing to state.
pub fn parse_modifiers(held: Option<&str>) -> Result<Modifiers, StepError> {
    let Some(held) = held else {
        return Ok(Modifiers::default());
    };
    let mut modifiers = Modifiers::default();
    for part in held.split('+') {
        match part.trim().to_ascii_lowercase().as_str() {
            "ctrl" | "control" => modifiers.ctrl = true,
            "alt" | "option" => modifiers.alt = true,
            "shift" => modifiers.shift = true,
            "meta" | "super" | "cmd" | "command" | "win" => modifiers.meta = true,
            "" => {}
            other => {
                return Err(StepError::Backend(format!(
                    "{other:?} is not a modifier; use ctrl, alt, shift or meta"
                )));
            }
        }
    }
    Ok(modifiers)
}

impl Key {
    /// The DOM `key` value and Windows virtual key code, for CDP.
    pub fn dom(&self) -> (String, i64) {
        match self {
            Self::Enter => ("Enter".into(), 13),
            Self::Tab => ("Tab".into(), 9),
            Self::Escape => ("Escape".into(), 27),
            Self::Backspace => ("Backspace".into(), 8),
            Self::Delete => ("Delete".into(), 46),
            Self::Space => (" ".into(), 32),
            Self::Up => ("ArrowUp".into(), 38),
            Self::Down => ("ArrowDown".into(), 40),
            Self::Left => ("ArrowLeft".into(), 37),
            Self::Right => ("ArrowRight".into(), 39),
            Self::Home => ("Home".into(), 36),
            Self::End => ("End".into(), 35),
            Self::PageUp => ("PageUp".into(), 33),
            Self::PageDown => ("PageDown".into(), 34),
            Self::Insert => ("Insert".into(), 45),
            Self::CapsLock => ("CapsLock".into(), 20),
            Self::PrintScreen => ("PrintScreen".into(), 44),
            Self::Menu => ("ContextMenu".into(), 93),
            // VK_F1 is 112 and the range is contiguous through F24.
            Self::Function(number) => (format!("F{number}"), 111 + i64::from(*number)),
            Self::Media(media) => match media {
                MediaKey::PlayPause => ("MediaPlayPause".into(), 179),
                MediaKey::Stop => ("MediaStop".into(), 178),
                MediaKey::Next => ("MediaTrackNext".into(), 176),
                MediaKey::Previous => ("MediaTrackPrevious".into(), 177),
                MediaKey::VolumeUp => ("AudioVolumeUp".into(), 175),
                MediaKey::VolumeDown => ("AudioVolumeDown".into(), 174),
                MediaKey::Mute => ("AudioVolumeMute".into(), 173),
            },
            Self::Char(character) => (character.to_string(), character.to_ascii_uppercase() as i64),
        }
    }

    /// The evdev keycode for a US layout, for the Wayland seat.
    ///
    /// `None` for characters that need a modifier or a layout we do not
    /// advertise — the caller must refuse rather than approximate.
    pub fn evdev(&self) -> Option<u32> {
        let code = match self {
            Self::Escape => 1,
            Self::Backspace => 14,
            Self::Tab => 15,
            Self::Enter => 28,
            Self::Space => 57,
            Self::Home => 102,
            Self::Up => 103,
            Self::PageUp => 104,
            Self::Left => 105,
            Self::Right => 106,
            Self::End => 107,
            Self::Down => 108,
            Self::PageDown => 109,
            Self::Delete => 111,
            Self::Insert => 110,
            Self::CapsLock => 58,
            Self::PrintScreen => 99, // KEY_SYSRQ
            Self::Menu => 127,       // KEY_COMPOSE, which is the context-menu key
            // F1..F10 are contiguous from 59; F11 and F12 sit apart at 87 and
            // 88, and F13 onward resume at 183. Three ranges, because that is
            // what evdev actually is — a single formula here would be wrong for
            // eight of the twenty-four.
            Self::Function(number) => match *number {
                1..=10 => 58 + u32::from(*number),
                11 => 87,
                12 => 88,
                13..=24 => 170 + u32::from(*number),
                _ => return None,
            },
            Self::Media(media) => match media {
                MediaKey::Mute => 113,
                MediaKey::VolumeDown => 114,
                MediaKey::VolumeUp => 115,
                MediaKey::Next => 163,
                MediaKey::PlayPause => 164,
                MediaKey::Previous => 165,
                MediaKey::Stop => 166, // KEY_STOPCD
            },
            Self::Char(character) => return evdev_for_char(*character),
        };
        Some(code)
    }
}

/// The name of a key, from the evdev code a compositor reported.
///
/// The inverse of [`Key::evdev`], and it exists for the viewer: the user's
/// compositor hands us raw evdev codes, and the stage speaks names. Translating
/// to a keysym in between is what makes a viewer type the wrong character on a
/// non-US layout — the stage owns the keymap, so the code is passed through as a
/// name it can look up itself.
///
/// `None` for anything not in the stage's vocabulary, which the caller drops
/// rather than approximating.
pub fn name_for_evdev(code: u32) -> Option<String> {
    let named = match code {
        1 => "Escape",
        14 => "BackSpace",
        15 => "Tab",
        28 => "Enter",
        57 => "Space",
        102 => "Home",
        103 => "Up",
        104 => "PageUp",
        105 => "Left",
        106 => "Right",
        107 => "End",
        108 => "Down",
        109 => "PageDown",
        111 => "Delete",
        _ => {
            // Printable keys are found by searching the forward table rather
            // than by keeping a second one. Two tables drift; one cannot.
            return ('\u{20}'..='\u{7e}')
                .find(|character| evdev_for_char(*character) == Some(code))
                .map(|character| character.to_string());
        }
    };
    Some(named.to_owned())
}

/// evdev keycode for a printable character on a US layout, unshifted.
///
/// Returns `None` for anything requiring shift; [`shifted_char`] reports which
/// characters need it so a caller can hold shift rather than silently sending
/// the wrong key — the bug that made `type "Hello"` deliver `hello`.
pub fn evdev_for_char(character: char) -> Option<u32> {
    const ROW1: [char; 10] = ['1', '2', '3', '4', '5', '6', '7', '8', '9', '0'];
    const ROWQ: [char; 10] = ['q', 'w', 'e', 'r', 't', 'y', 'u', 'i', 'o', 'p'];
    const ROWA: [char; 9] = ['a', 's', 'd', 'f', 'g', 'h', 'j', 'k', 'l'];
    const ROWZ: [char; 7] = ['z', 'x', 'c', 'v', 'b', 'n', 'm'];

    let lower = character.to_ascii_lowercase();
    if let Some(index) = ROW1.iter().position(|key| *key == lower) {
        return Some(2 + index as u32);
    }
    if let Some(index) = ROWQ.iter().position(|key| *key == lower) {
        return Some(16 + index as u32);
    }
    if let Some(index) = ROWA.iter().position(|key| *key == lower) {
        return Some(30 + index as u32);
    }
    if let Some(index) = ROWZ.iter().position(|key| *key == lower) {
        return Some(44 + index as u32);
    }
    let code = match lower {
        '-' => 12,
        '=' => 13,
        '[' => 26,
        ']' => 27,
        ';' => 39,
        '\'' => 40,
        '`' => 41,
        '\\' => 43,
        ',' => 51,
        '.' => 52,
        '/' => 53,
        _ => return None,
    };
    Some(code)
}

/// The unshifted character that produces `character` when shift is held, on a
/// US layout — or `None` when `character` needs no shift.
pub fn shifted_char(character: char) -> Option<char> {
    if character.is_ascii_uppercase() {
        return Some(character.to_ascii_lowercase());
    }
    let base = match character {
        '!' => '1',
        '@' => '2',
        '#' => '3',
        '$' => '4',
        '%' => '5',
        '^' => '6',
        '&' => '7',
        '*' => '8',
        '(' => '9',
        ')' => '0',
        '_' => '-',
        '+' => '=',
        '{' => '[',
        '}' => ']',
        ':' => ';',
        '"' => '\'',
        '~' => '`',
        '|' => '\\',
        '<' => ',',
        '>' => '.',
        '?' => '/',
        _ => return None,
    };
    Some(base)
}

/// Terminal byte sequence for a chord.
pub fn terminal_bytes(chord: &Chord) -> Result<Vec<u8>, StepError> {
    let mut bytes = match &chord.key {
        Key::Enter => vec![b'\r'],
        Key::Tab => vec![b'\t'],
        Key::Escape => vec![0x1b],
        Key::Backspace => vec![0x7f],
        Key::Delete => b"\x1b[3~".to_vec(),
        Key::Space => vec![b' '],
        Key::Up => b"\x1b[A".to_vec(),
        Key::Down => b"\x1b[B".to_vec(),
        Key::Right => b"\x1b[C".to_vec(),
        Key::Left => b"\x1b[D".to_vec(),
        Key::Home => b"\x1b[H".to_vec(),
        Key::End => b"\x1b[F".to_vec(),
        Key::PageUp => b"\x1b[5~".to_vec(),
        Key::PageDown => b"\x1b[6~".to_vec(),
        Key::Insert => b"\x1b[2~".to_vec(),
        // The xterm sequences, which every terminfo in practical use agrees on.
        // F1-F4 are SS3-prefixed and the rest are CSI with a number that skips
        // 16, 22 and 25 — an irregularity in the standard, not a typo here.
        Key::Function(number) => match *number {
            1 => b"\x1bOP".to_vec(),
            2 => b"\x1bOQ".to_vec(),
            3 => b"\x1bOR".to_vec(),
            4 => b"\x1bOS".to_vec(),
            5 => b"\x1b[15~".to_vec(),
            6..=10 => format!("\x1b[{}~", 11 + u16::from(*number)).into_bytes(),
            11 | 12 => format!("\x1b[{}~", 12 + u16::from(*number)).into_bytes(),
            _ => {
                return Err(StepError::Backend(format!(
                    "a terminal has no F{number}; F1 through F12 are the ones it can receive"
                )));
            }
        },
        // No byte sequence exists for any of these. A terminal is a byte
        // stream, and a key with no encoding cannot be approximated by another
        // one — sending something close would run a different command.
        key @ (Key::CapsLock | Key::PrintScreen | Key::Menu | Key::Media(_)) => {
            return Err(StepError::Backend(format!(
                "a terminal has no encoding for {key:?}"
            )));
        }
        Key::Char(character) => {
            if chord.modifiers.ctrl && character.is_ascii_alphabetic() {
                // Ctrl-A is 0x01: the letter's position in the alphabet.
                vec![(character.to_ascii_uppercase() as u8) - b'A' + 1]
            } else if chord.modifiers.shift {
                vec![character.to_ascii_uppercase() as u8]
            } else {
                let mut buffer = [0u8; 4];
                character.encode_utf8(&mut buffer).as_bytes().to_vec()
            }
        }
    };
    if chord.modifiers.alt {
        bytes.insert(0, 0x1b);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifiers_are_parsed_not_discarded() {
        // The regression that mattered: `ctrl+a` used to become plain `a` and
        // insert a literal letter into the focused field.
        let chord = parse("ctrl+a").unwrap();
        assert!(chord.modifiers.ctrl);
        assert_eq!(chord.key, Key::Char('a'));

        let chord = parse("ctrl+shift+t").unwrap();
        assert!(chord.modifiers.ctrl && chord.modifiers.shift);
        assert!(!chord.modifiers.alt);
        assert_eq!(chord.key, Key::Char('t'));
    }

    #[test]
    fn modifier_aliases_are_accepted() {
        for stroke in ["meta+a", "super+a", "cmd+a", "command+a", "win+a"] {
            assert!(parse(stroke).unwrap().modifiers.meta, "{stroke}");
        }
        assert!(parse("option+x").unwrap().modifiers.alt);
        assert!(parse("control+x").unwrap().modifiers.ctrl);
    }

    #[test]
    fn named_keys_and_their_aliases_parse() {
        assert_eq!(parse("Enter").unwrap().key, Key::Enter);
        assert_eq!(parse("return").unwrap().key, Key::Enter);
        assert_eq!(parse("esc").unwrap().key, Key::Escape);
        assert_eq!(parse("ArrowUp").unwrap().key, Key::Up);
        assert_eq!(parse("pgdn").unwrap().key, Key::PageDown);
    }

    #[test]
    fn the_plus_key_itself_is_addressable() {
        assert_eq!(parse("+").unwrap().key, Key::Char('+'));
        let chord = parse("ctrl++").unwrap();
        assert!(chord.modifiers.ctrl);
        assert_eq!(chord.key, Key::Char('+'));
    }

    #[test]
    fn unknown_names_are_errors_rather_than_silent_approximations() {
        assert!(parse("SuperTurbo").is_err());
        assert!(
            parse("hyper+a")
                .unwrap_err()
                .to_string()
                .contains("modifier")
        );
        assert!(parse("").is_err());
    }

    #[test]
    fn cdp_modifier_bits_match_the_protocol() {
        // alt 1, ctrl 2, meta 4, shift 8.
        assert_eq!(parse("a").unwrap().modifiers.cdp_bits(), 0);
        assert_eq!(parse("alt+a").unwrap().modifiers.cdp_bits(), 1);
        assert_eq!(parse("ctrl+a").unwrap().modifiers.cdp_bits(), 2);
        assert_eq!(parse("meta+a").unwrap().modifiers.cdp_bits(), 4);
        assert_eq!(parse("shift+a").unwrap().modifiers.cdp_bits(), 8);
        assert_eq!(parse("ctrl+shift+a").unwrap().modifiers.cdp_bits(), 10);
    }

    #[test]
    fn terminal_bytes_honour_modifiers() {
        assert_eq!(
            terminal_bytes(&parse("ctrl+c").unwrap()).unwrap(),
            vec![0x03]
        );
        assert_eq!(
            terminal_bytes(&parse("ctrl+d").unwrap()).unwrap(),
            vec![0x04]
        );
        assert_eq!(
            terminal_bytes(&parse("alt+b").unwrap()).unwrap(),
            vec![0x1b, b'b']
        );
        assert_eq!(terminal_bytes(&parse("Enter").unwrap()).unwrap(), b"\r");
        assert_eq!(terminal_bytes(&parse("Up").unwrap()).unwrap(), b"\x1b[A");
    }

    #[test]
    fn evdev_covers_the_us_punctuation_row() {
        // The gap that made `type "adam@example.com"` fail outright.
        for character in ['.', '-', '/', '_', ':', '@', ';', ',', '\'', '['] {
            let direct = evdev_for_char(character);
            let via_shift = shifted_char(character).and_then(evdev_for_char);
            assert!(
                direct.is_some() || via_shift.is_some(),
                "{character:?} is unreachable on the stage keyboard"
            );
        }
    }

    #[test]
    fn uppercase_is_reachable_by_shift_rather_than_silently_lowercased() {
        // `type "Hello"` used to deliver `hello`.
        assert_eq!(shifted_char('H'), Some('h'));
        assert!(evdev_for_char('h').is_some());
        assert_eq!(shifted_char('h'), None, "lowercase needs no shift");
    }

    #[test]
    fn dom_key_values_are_what_a_page_expects() {
        assert_eq!(Key::Enter.dom(), ("Enter".into(), 13));
        assert_eq!(Key::Up.dom(), ("ArrowUp".into(), 38));
        assert_eq!(Key::Char('a').dom().0, "a");
    }

    #[test]
    fn evdev_codes_round_trip_back_to_names_the_stage_understands() {
        // The viewer receives codes and must hand back something `parse`
        // accepts, or a human's keystroke is silently dropped.
        for name in [
            "Escape", "Tab", "Enter", "Space", "Home", "Up", "Left", "Right", "Down", "Delete",
            "a", "z", "0", "9",
        ] {
            let chord = parse(name).expect(name);
            let code = chord.key.evdev().expect(name);
            let back = name_for_evdev(code).expect(name);
            assert!(
                parse(&back).is_ok(),
                "{name} became code {code} became {back:?}, which does not parse"
            );
            assert_eq!(
                parse(&back).unwrap().key.evdev(),
                Some(code),
                "{name} did not survive the round trip"
            );
        }
    }

    #[test]
    fn an_unknown_evdev_code_is_dropped_rather_than_guessed() {
        // A key the stage has no name for must not become an approximation:
        // sending the wrong key is worse than sending none.
        assert_eq!(name_for_evdev(0), None);
        assert_eq!(name_for_evdev(9999), None);
    }
}
