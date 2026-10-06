//! Parses hotkey strings such as "Ctrl+Alt+P" into RegisterHotKey arguments.

pub const MOD_ALT: u32 = 0x1;
pub const MOD_CONTROL: u32 = 0x2;
pub const MOD_SHIFT: u32 = 0x4;
pub const MOD_WIN: u32 = 0x8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    pub modifiers: u32,
    pub vk: u32,
}

/// Returns `Ok(None)` for an empty string (hotkey disabled).
pub fn parse(text: &str) -> Result<Option<Hotkey>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let mut modifiers = 0;
    let mut vk = None;
    for part in text.split('+').map(str::trim) {
        let upper = part.to_ascii_uppercase();
        let m = match upper.as_str() {
            "CTRL" | "CONTROL" => MOD_CONTROL,
            "ALT" => MOD_ALT,
            "SHIFT" => MOD_SHIFT,
            "WIN" | "WINDOWS" => MOD_WIN,
            _ => 0,
        };
        if m != 0 {
            modifiers |= m;
            continue;
        }
        if vk.is_some() {
            return Err(format!("\"{text}\" has more than one key"));
        }
        vk = Some(key_code(&upper).ok_or_else(|| format!("unknown key \"{part}\" in \"{text}\""))?);
    }
    let vk = vk.ok_or_else(|| format!("\"{text}\" has no key, only modifiers"))?;
    Ok(Some(Hotkey { modifiers, vk }))
}

fn key_code(name: &str) -> Option<u32> {
    let bytes = name.as_bytes();
    if bytes.len() == 1 && (bytes[0].is_ascii_uppercase() || bytes[0].is_ascii_digit()) {
        return Some(u32::from(bytes[0]));
    }
    if let Some(n) = name.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
        return (1..=24).contains(&n).then_some(0x70 + n - 1);
    }
    Some(match name {
        "PAUSE" => 0x13,
        "SCROLLLOCK" => 0x91,
        "INSERT" | "INS" => 0x2D,
        "DELETE" | "DEL" => 0x2E,
        "HOME" => 0x24,
        "END" => 0x23,
        "PAGEUP" | "PGUP" => 0x21,
        "PAGEDOWN" | "PGDN" => 0x22,
        "SPACE" => 0x20,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_hotkeys() {
        assert_eq!(
            parse("Ctrl+Alt+P"),
            Ok(Some(Hotkey { modifiers: MOD_CONTROL | MOD_ALT, vk: 'P' as u32 }))
        );
        assert_eq!(
            parse(" shift + win + 7 "),
            Ok(Some(Hotkey { modifiers: MOD_SHIFT | MOD_WIN, vk: '7' as u32 }))
        );
        assert_eq!(parse("F13"), Ok(Some(Hotkey { modifiers: 0, vk: 0x7C })));
        assert_eq!(parse("Ctrl+ScrollLock"), Ok(Some(Hotkey { modifiers: MOD_CONTROL, vk: 0x91 })));
        assert_eq!(parse(""), Ok(None));
    }

    #[test]
    fn rejects_bad_hotkeys() {
        assert!(parse("Ctrl+Alt").is_err());
        assert!(parse("Ctrl+P+Q").is_err());
        assert!(parse("Ctrl+F25").is_err());
        assert!(parse("Ctrl+Banana").is_err());
    }
}
