//! Miniquad event conversion utilities.

use blitz_traits::events::MouseEventButton;
use blitz_traits::events::{BlitzKeyEvent, KeyState};
use keyboard_types::{Code, Key, Location, Modifiers};
use miniquad::{KeyCode, KeyMods, MouseButton};

/// Convert miniquad KeyMods to keyboard-types Modifiers.
pub fn mq_mods_to_kbt(mods: KeyMods) -> Modifiers {
    let mut result = Modifiers::empty();
    if mods.shift {
        result |= Modifiers::SHIFT;
    }
    if mods.ctrl {
        result |= Modifiers::CONTROL;
    }
    if mods.alt {
        result |= Modifiers::ALT;
    }
    if mods.logo {
        result |= Modifiers::META;
    }
    result
}

/// Convert miniquad MouseButton to blitz MouseEventButton.
pub fn mq_mouse_button_to_blitz(button: MouseButton) -> MouseEventButton {
    match button {
        MouseButton::Left => MouseEventButton::Main,
        MouseButton::Right => MouseEventButton::Secondary,
        MouseButton::Middle => MouseEventButton::Auxiliary,
        MouseButton::Back => MouseEventButton::Fourth,
        MouseButton::Forward => MouseEventButton::Fifth,
        MouseButton::Unknown => MouseEventButton::Main,
    }
}

/// Convert miniquad KeyCode to keyboard-types Code.
pub fn mq_keycode_to_code(key: KeyCode) -> Code {
    match key {
        KeyCode::A => Code::KeyA,
        KeyCode::B => Code::KeyB,
        KeyCode::C => Code::KeyC,
        KeyCode::D => Code::KeyD,
        KeyCode::E => Code::KeyE,
        KeyCode::F => Code::KeyF,
        KeyCode::G => Code::KeyG,
        KeyCode::H => Code::KeyH,
        KeyCode::I => Code::KeyI,
        KeyCode::J => Code::KeyJ,
        KeyCode::K => Code::KeyK,
        KeyCode::L => Code::KeyL,
        KeyCode::M => Code::KeyM,
        KeyCode::N => Code::KeyN,
        KeyCode::O => Code::KeyO,
        KeyCode::P => Code::KeyP,
        KeyCode::Q => Code::KeyQ,
        KeyCode::R => Code::KeyR,
        KeyCode::S => Code::KeyS,
        KeyCode::T => Code::KeyT,
        KeyCode::U => Code::KeyU,
        KeyCode::V => Code::KeyV,
        KeyCode::W => Code::KeyW,
        KeyCode::X => Code::KeyX,
        KeyCode::Y => Code::KeyY,
        KeyCode::Z => Code::KeyZ,
        KeyCode::Key0 => Code::Digit0,
        KeyCode::Key1 => Code::Digit1,
        KeyCode::Key2 => Code::Digit2,
        KeyCode::Key3 => Code::Digit3,
        KeyCode::Key4 => Code::Digit4,
        KeyCode::Key5 => Code::Digit5,
        KeyCode::Key6 => Code::Digit6,
        KeyCode::Key7 => Code::Digit7,
        KeyCode::Key8 => Code::Digit8,
        KeyCode::Key9 => Code::Digit9,
        KeyCode::Enter => Code::Enter,
        KeyCode::Escape => Code::Escape,
        KeyCode::Backspace => Code::Backspace,
        KeyCode::Tab => Code::Tab,
        KeyCode::Space => Code::Space,
        KeyCode::Minus => Code::Minus,
        KeyCode::Equal => Code::Equal,
        KeyCode::LeftBracket => Code::BracketLeft,
        KeyCode::RightBracket => Code::BracketRight,
        KeyCode::Backslash => Code::Backslash,
        KeyCode::Semicolon => Code::Semicolon,
        KeyCode::Apostrophe => Code::Quote,
        KeyCode::GraveAccent => Code::Backquote,
        KeyCode::Comma => Code::Comma,
        KeyCode::Period => Code::Period,
        KeyCode::Slash => Code::Slash,
        KeyCode::Delete => Code::Delete,
        KeyCode::Insert => Code::Insert,
        KeyCode::Home => Code::Home,
        KeyCode::End => Code::End,
        KeyCode::PageUp => Code::PageUp,
        KeyCode::PageDown => Code::PageDown,
        KeyCode::Right => Code::ArrowRight,
        KeyCode::Left => Code::ArrowLeft,
        KeyCode::Down => Code::ArrowDown,
        KeyCode::Up => Code::ArrowUp,
        KeyCode::F1 => Code::F1,
        KeyCode::F2 => Code::F2,
        KeyCode::F3 => Code::F3,
        KeyCode::F4 => Code::F4,
        KeyCode::F5 => Code::F5,
        KeyCode::F6 => Code::F6,
        KeyCode::F7 => Code::F7,
        KeyCode::F8 => Code::F8,
        KeyCode::F9 => Code::F9,
        KeyCode::F10 => Code::F10,
        KeyCode::F11 => Code::F11,
        KeyCode::F12 => Code::F12,
        KeyCode::LeftShift | KeyCode::RightShift => Code::ShiftLeft,
        KeyCode::LeftControl | KeyCode::RightControl => Code::ControlLeft,
        KeyCode::LeftAlt | KeyCode::RightAlt => Code::AltLeft,
        KeyCode::LeftSuper | KeyCode::RightSuper => Code::MetaLeft,
        KeyCode::CapsLock => Code::CapsLock,
        _ => Code::Unidentified,
    }
}

/// Convert miniquad KeyCode to keyboard-types Key.
pub fn mq_keycode_to_key(key: KeyCode) -> Key {
    match key {
        KeyCode::Enter => Key::Enter,
        KeyCode::Escape => Key::Escape,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Tab => Key::Tab,
        KeyCode::Space => Key::Character(" ".into()),
        KeyCode::Delete => Key::Delete,
        KeyCode::Insert => Key::Insert,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Right => Key::ArrowRight,
        KeyCode::Left => Key::ArrowLeft,
        KeyCode::Down => Key::ArrowDown,
        KeyCode::Up => Key::ArrowUp,
        KeyCode::F1 => Key::F1,
        KeyCode::F2 => Key::F2,
        KeyCode::F3 => Key::F3,
        KeyCode::F4 => Key::F4,
        KeyCode::F5 => Key::F5,
        KeyCode::F6 => Key::F6,
        KeyCode::F7 => Key::F7,
        KeyCode::F8 => Key::F8,
        KeyCode::F9 => Key::F9,
        KeyCode::F10 => Key::F10,
        KeyCode::F11 => Key::F11,
        KeyCode::F12 => Key::F12,
        KeyCode::LeftShift | KeyCode::RightShift => Key::Shift,
        KeyCode::LeftControl | KeyCode::RightControl => Key::Control,
        KeyCode::LeftAlt | KeyCode::RightAlt => Key::Alt,
        KeyCode::LeftSuper | KeyCode::RightSuper => Key::Meta,
        KeyCode::CapsLock => Key::CapsLock,
        // Letter keys (lowercase — shift handling happens at a higher level or via char_event)
        KeyCode::A => Key::Character("a".into()),
        KeyCode::B => Key::Character("b".into()),
        KeyCode::C => Key::Character("c".into()),
        KeyCode::D => Key::Character("d".into()),
        KeyCode::E => Key::Character("e".into()),
        KeyCode::F => Key::Character("f".into()),
        KeyCode::G => Key::Character("g".into()),
        KeyCode::H => Key::Character("h".into()),
        KeyCode::I => Key::Character("i".into()),
        KeyCode::J => Key::Character("j".into()),
        KeyCode::K => Key::Character("k".into()),
        KeyCode::L => Key::Character("l".into()),
        KeyCode::M => Key::Character("m".into()),
        KeyCode::N => Key::Character("n".into()),
        KeyCode::O => Key::Character("o".into()),
        KeyCode::P => Key::Character("p".into()),
        KeyCode::Q => Key::Character("q".into()),
        KeyCode::R => Key::Character("r".into()),
        KeyCode::S => Key::Character("s".into()),
        KeyCode::T => Key::Character("t".into()),
        KeyCode::U => Key::Character("u".into()),
        KeyCode::V => Key::Character("v".into()),
        KeyCode::W => Key::Character("w".into()),
        KeyCode::X => Key::Character("x".into()),
        KeyCode::Y => Key::Character("y".into()),
        KeyCode::Z => Key::Character("z".into()),
        // Number keys
        KeyCode::Key0 => Key::Character("0".into()),
        KeyCode::Key1 => Key::Character("1".into()),
        KeyCode::Key2 => Key::Character("2".into()),
        KeyCode::Key3 => Key::Character("3".into()),
        KeyCode::Key4 => Key::Character("4".into()),
        KeyCode::Key5 => Key::Character("5".into()),
        KeyCode::Key6 => Key::Character("6".into()),
        KeyCode::Key7 => Key::Character("7".into()),
        KeyCode::Key8 => Key::Character("8".into()),
        KeyCode::Key9 => Key::Character("9".into()),
        // Symbol keys
        KeyCode::Minus => Key::Character("-".into()),
        KeyCode::Equal => Key::Character("=".into()),
        KeyCode::LeftBracket => Key::Character("[".into()),
        KeyCode::RightBracket => Key::Character("]".into()),
        KeyCode::Backslash => Key::Character("\\".into()),
        KeyCode::Semicolon => Key::Character(";".into()),
        KeyCode::Apostrophe => Key::Character("'".into()),
        KeyCode::GraveAccent => Key::Character("`".into()),
        KeyCode::Comma => Key::Character(",".into()),
        KeyCode::Period => Key::Character(".".into()),
        KeyCode::Slash => Key::Character("/".into()),
        _ => Key::Unidentified,
    }
}

/// Create a BlitzKeyEvent from miniquad key input.
pub fn create_key_event(
    keycode: KeyCode,
    keymods: KeyMods,
    state: KeyState,
    repeat: bool,
    text: Option<char>,
) -> BlitzKeyEvent {
    let text = text.filter(|_| state == KeyState::Pressed).map(|c| c.to_string());
    let key = text
        .as_ref()
        .map(|text| Key::Character(text.as_str().into()))
        .unwrap_or_else(|| mq_keycode_to_key(keycode));

    BlitzKeyEvent {
        key,
        code: mq_keycode_to_code(keycode),
        modifiers: mq_mods_to_kbt(keymods),
        state,
        is_auto_repeating: repeat,
        location: Location::Standard,
        is_composing: false,
        text: text.map(|text| text.into()),
    }
}
