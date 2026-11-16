/// Returns the text a key produces when pressed, or `None` if it produces none.
///
/// Control and navigation keys — `Backspace`, `Tab`, `Escape`, the arrows, the
/// function keys, and so on — produce no text.
pub fn key_text(code: u8, shift: bool) -> Option<&'static str> {
    match code {
        13 => Some("\r"),
        32 => Some(" "),
        48..=57 | 65..=90 | 186..=192 | 219..=222 => key_name(code, shift),
        _ => None,
    }
}

/// Returns the DOM `key` value for a keyboard event, e.g. `"c"`, `"Enter"`, or
/// `"Backspace"`, or `None` for an unrecognized key code. With `shift`,
/// character keys give their US keyboard layout shifted value, e.g. `"M"` or
/// `"!"`.
pub fn key_name(code: u8, shift: bool) -> Option<&'static str> {
    if shift && let Some(shifted) = shifted_character(code) {
        return Some(shifted);
    }
    unshifted_key_name(code)
}

/// Returns the DOM `code` value naming the physical key, e.g. `"KeyM"`,
/// `"Digit1"`, or `"Enter"`, or `None` for an unrecognized key code.
pub fn key_code(code: u8) -> Option<&'static str> {
    const LETTERS: [&str; 26] = [
        "KeyA", "KeyB", "KeyC", "KeyD", "KeyE", "KeyF", "KeyG", "KeyH", "KeyI",
        "KeyJ", "KeyK", "KeyL", "KeyM", "KeyN", "KeyO", "KeyP", "KeyQ", "KeyR",
        "KeyS", "KeyT", "KeyU", "KeyV", "KeyW", "KeyX", "KeyY", "KeyZ",
    ];
    const DIGITS: [&str; 10] = [
        "Digit0", "Digit1", "Digit2", "Digit3", "Digit4", "Digit5", "Digit6",
        "Digit7", "Digit8", "Digit9",
    ];
    match code {
        16 => Some("ShiftLeft"),
        17 => Some("ControlLeft"),
        18 => Some("AltLeft"),
        32 => Some("Space"),
        48..=57 => Some(DIGITS[usize::from(code - 48)]),
        65..=90 => Some(LETTERS[usize::from(code - 65)]),
        91 => Some("MetaLeft"),
        186 => Some("Semicolon"),
        187 => Some("Equal"),
        188 => Some("Comma"),
        189 => Some("Minus"),
        190 => Some("Period"),
        191 => Some("Slash"),
        192 => Some("Backquote"),
        219 => Some("BracketLeft"),
        220 => Some("Backslash"),
        221 => Some("BracketRight"),
        222 => Some("Quote"),
        _ => unshifted_key_name(code),
    }
}

fn shifted_character(code: u8) -> Option<&'static str> {
    const LETTERS: [&str; 26] = [
        "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N",
        "O", "P", "Q", "R", "S", "T", "U", "V", "W", "X", "Y", "Z",
    ];
    const DIGITS: [&str; 10] =
        [")", "!", "@", "#", "$", "%", "^", "&", "*", "("];
    match code {
        48..=57 => Some(DIGITS[usize::from(code - 48)]),
        65..=90 => Some(LETTERS[usize::from(code - 65)]),
        186 => Some(":"),
        187 => Some("+"),
        188 => Some("<"),
        189 => Some("_"),
        190 => Some(">"),
        191 => Some("?"),
        192 => Some("~"),
        219 => Some("{"),
        220 => Some("|"),
        221 => Some("}"),
        222 => Some("\""),
        _ => None,
    }
}

fn unshifted_key_name(code: u8) -> Option<&'static str> {
    match code {
        8 => Some("Backspace"),
        9 => Some("Tab"),
        13 => Some("Enter"),
        16 => Some("Shift"),
        17 => Some("Control"),
        18 => Some("Alt"),
        19 => Some("Pause"),
        20 => Some("CapsLock"),
        27 => Some("Escape"),
        32 => Some(" "),
        33 => Some("PageUp"),
        34 => Some("PageDown"),
        35 => Some("End"),
        36 => Some("Home"),
        37 => Some("ArrowLeft"),
        38 => Some("ArrowUp"),
        39 => Some("ArrowRight"),
        40 => Some("ArrowDown"),
        45 => Some("Insert"),
        46 => Some("Delete"),
        48 => Some("0"),
        49 => Some("1"),
        50 => Some("2"),
        51 => Some("3"),
        52 => Some("4"),
        53 => Some("5"),
        54 => Some("6"),
        55 => Some("7"),
        56 => Some("8"),
        57 => Some("9"),
        65 => Some("a"),
        66 => Some("b"),
        67 => Some("c"),
        68 => Some("d"),
        69 => Some("e"),
        70 => Some("f"),
        71 => Some("g"),
        72 => Some("h"),
        73 => Some("i"),
        74 => Some("j"),
        75 => Some("k"),
        76 => Some("l"),
        77 => Some("m"),
        78 => Some("n"),
        79 => Some("o"),
        80 => Some("p"),
        81 => Some("q"),
        82 => Some("r"),
        83 => Some("s"),
        84 => Some("t"),
        85 => Some("u"),
        86 => Some("v"),
        87 => Some("w"),
        88 => Some("x"),
        89 => Some("y"),
        90 => Some("z"),
        91 => Some("Meta"),
        93 => Some("ContextMenu"),
        112 => Some("F1"),
        113 => Some("F2"),
        114 => Some("F3"),
        115 => Some("F4"),
        116 => Some("F5"),
        117 => Some("F6"),
        118 => Some("F7"),
        119 => Some("F8"),
        120 => Some("F9"),
        121 => Some("F10"),
        122 => Some("F11"),
        123 => Some("F12"),
        144 => Some("NumLock"),
        145 => Some("ScrollLock"),
        186 => Some(";"),
        187 => Some("="),
        188 => Some(","),
        189 => Some("-"),
        190 => Some("."),
        191 => Some("/"),
        192 => Some("`"),
        219 => Some("["),
        220 => Some("\\"),
        221 => Some("]"),
        222 => Some("'"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_produces_carriage_return() {
        assert_eq!(key_text(13, false), Some("\r"));
    }

    #[test]
    fn printable_keys_produce_their_character() {
        assert_eq!(key_text(67, false), Some("c"));
        assert_eq!(key_text(48, false), Some("0"));
        assert_eq!(key_text(32, false), Some(" "));
        assert_eq!(key_text(188, false), Some(","));
        assert_eq!(key_text(222, false), Some("'"));
    }

    #[test]
    fn shift_produces_the_us_layout_shifted_character() {
        for (code, shifted) in [
            (77, "M"),
            (65, "A"),
            (49, "!"),
            (48, ")"),
            (186, ":"),
            (191, "?"),
            (222, "\""),
        ] {
            assert_eq!(key_name(code, true), Some(shifted), "code {code}");
            assert_eq!(key_text(code, true), Some(shifted), "code {code}");
        }
    }

    #[test]
    fn shift_leaves_non_character_keys_unchanged() {
        for code in [8, 9, 13, 16, 32, 37, 112] {
            assert_eq!(
                key_name(code, true),
                key_name(code, false),
                "code {code}"
            );
            assert_eq!(
                key_text(code, true),
                key_text(code, false),
                "code {code}"
            );
        }
        assert_eq!(key_name(0, true), None);
        assert_eq!(key_name(8, true), Some("Backspace"));
    }

    #[test]
    fn physical_codes_name_the_key_not_its_character() {
        for (code, physical) in [
            (77, "KeyM"),
            (48, "Digit0"),
            (32, "Space"),
            (186, "Semicolon"),
            (222, "Quote"),
            (16, "ShiftLeft"),
            (91, "MetaLeft"),
            (13, "Enter"),
            (37, "ArrowLeft"),
        ] {
            assert_eq!(key_code(code), Some(physical), "code {code}");
        }
        assert_eq!(key_code(0), None);
    }

    #[test]
    fn control_and_navigation_keys_produce_no_text() {
        for code in [8, 9, 16, 27, 37, 38, 39, 40, 46, 112] {
            assert_eq!(
                key_text(code, false),
                None,
                "code {code} should produce no text"
            );
        }
    }
}
