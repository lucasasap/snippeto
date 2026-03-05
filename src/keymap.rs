use evdev::KeyCode;

pub fn keycode_to_char(code: KeyCode) -> Option<(char, char)> {
    match code {
        KeyCode::KEY_A => Some(('a', 'A')),
        KeyCode::KEY_B => Some(('b', 'B')),
        KeyCode::KEY_C => Some(('c', 'C')),
        KeyCode::KEY_D => Some(('d', 'D')),
        KeyCode::KEY_E => Some(('e', 'E')),
        KeyCode::KEY_F => Some(('f', 'F')),
        KeyCode::KEY_G => Some(('g', 'G')),
        KeyCode::KEY_H => Some(('h', 'H')),
        KeyCode::KEY_I => Some(('i', 'I')),
        KeyCode::KEY_J => Some(('j', 'J')),
        KeyCode::KEY_K => Some(('k', 'K')),
        KeyCode::KEY_L => Some(('l', 'L')),
        KeyCode::KEY_M => Some(('m', 'M')),
        KeyCode::KEY_N => Some(('n', 'N')),
        KeyCode::KEY_O => Some(('o', 'O')),
        KeyCode::KEY_P => Some(('p', 'P')),
        KeyCode::KEY_Q => Some(('q', 'Q')),
        KeyCode::KEY_R => Some(('r', 'R')),
        KeyCode::KEY_S => Some(('s', 'S')),
        KeyCode::KEY_T => Some(('t', 'T')),
        KeyCode::KEY_U => Some(('u', 'U')),
        KeyCode::KEY_V => Some(('v', 'V')),
        KeyCode::KEY_W => Some(('w', 'W')),
        KeyCode::KEY_X => Some(('x', 'X')),
        KeyCode::KEY_Y => Some(('y', 'Y')),
        KeyCode::KEY_Z => Some(('z', 'Z')),

        KeyCode::KEY_1 => Some(('1', '!')),
        KeyCode::KEY_2 => Some(('2', '@')),
        KeyCode::KEY_3 => Some(('3', '#')),
        KeyCode::KEY_4 => Some(('4', '$')),
        KeyCode::KEY_5 => Some(('5', '%')),
        KeyCode::KEY_6 => Some(('6', '^')),
        KeyCode::KEY_7 => Some(('7', '&')),
        KeyCode::KEY_8 => Some(('8', '*')),
        KeyCode::KEY_9 => Some(('9', '(')),
        KeyCode::KEY_0 => Some(('0', ')')),

        KeyCode::KEY_MINUS => Some(('-', '_')),
        KeyCode::KEY_EQUAL => Some(('=', '+')),
        KeyCode::KEY_LEFTBRACE => Some(('[', '{')),
        KeyCode::KEY_RIGHTBRACE => Some((']', '}')),
        KeyCode::KEY_BACKSLASH => Some(('\\', '|')),
        KeyCode::KEY_SEMICOLON => Some((';', ':')),
        KeyCode::KEY_APOSTROPHE => Some(('\'', '"')),
        KeyCode::KEY_GRAVE => Some(('`', '~')),
        KeyCode::KEY_COMMA => Some((',', '<')),
        KeyCode::KEY_DOT => Some(('.', '>')),
        KeyCode::KEY_SLASH => Some(('/', '?')),
        KeyCode::KEY_SPACE => Some((' ', ' ')),

        _ => None,
    }
}

pub struct ShiftState {
    left: bool,
    right: bool,
}

impl ShiftState {
    pub fn new() -> Self {
        Self {
            left: false,
            right: false,
        }
    }

    /// Update shift state from a key event. Returns true if this was a shift key.
    pub fn update(&mut self, code: KeyCode, value: i32) -> bool {
        match code {
            KeyCode::KEY_LEFTSHIFT => {
                self.left = value != 0;
                true
            }
            KeyCode::KEY_RIGHTSHIFT => {
                self.right = value != 0;
                true
            }
            _ => false,
        }
    }

    pub fn active(&self) -> bool {
        self.left || self.right
    }
}
