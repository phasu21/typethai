//! Physical key -> US-QWERTY character tables (layout independent, so detection works whatever the OS mapping is).

/// Windows virtual-key code -> (unshifted, shifted) US char.
#[allow(dead_code)]
pub fn win_vk_to_us(vk: u32, shift: bool, caps: bool) -> Option<char> {
    let pair = match vk {
        0x30..=0x39 => { let d = (b'0' + (vk - 0x30) as u8) as char; (d, ")!@#$%^&*(".chars().nth((vk - 0x30) as usize).unwrap()) }
        0x41..=0x5A => { let l = (b'a' + (vk - 0x41) as u8) as char; let up = shift ^ caps; return Some(if up { l.to_ascii_uppercase() } else { l }) }
        0x20 => return None,
        0xBA => (';', ':'), 0xBB => ('=', '+'), 0xBC => (',', '<'), 0xBD => ('-', '_'), 0xBE => ('.', '>'), 0xBF => ('/', '?'),
        0xC0 => ('`', '~'), 0xDB => ('[', '{'), 0xDC => ('\\', '|'), 0xDD => (']', '}'), 0xDE => ('\'', '"'),
        _ => return None,
    };
    Some(if shift { pair.1 } else { pair.0 })
}

/// macOS ANSI virtual keycode (kVK_ANSI_*) -> (unshifted, shifted) US char.
#[allow(dead_code)]
pub fn mac_keycode_to_us(code: u16, shift: bool, caps: bool) -> Option<char> {
    const T: &[(u16, char, char)] = &[
        (0x00,'a','A'),(0x0B,'b','B'),(0x08,'c','C'),(0x02,'d','D'),(0x0E,'e','E'),(0x03,'f','F'),(0x05,'g','G'),(0x04,'h','H'),
        (0x22,'i','I'),(0x26,'j','J'),(0x28,'k','K'),(0x25,'l','L'),(0x2E,'m','M'),(0x2D,'n','N'),(0x1F,'o','O'),(0x23,'p','P'),
        (0x0C,'q','Q'),(0x0F,'r','R'),(0x01,'s','S'),(0x11,'t','T'),(0x20,'u','U'),(0x09,'v','V'),(0x0D,'w','W'),(0x07,'x','X'),
        (0x10,'y','Y'),(0x06,'z','Z'),
        (0x12,'1','!'),(0x13,'2','@'),(0x14,'3','#'),(0x15,'4','$'),(0x17,'5','%'),(0x16,'6','^'),(0x1A,'7','&'),(0x1C,'8','*'),
        (0x19,'9','('),(0x1D,'0',')'),(0x1B,'-','_'),(0x18,'=','+'),(0x21,'[','{'),(0x1E,']','}'),(0x2A,'\\','|'),
        (0x29,';',':'),(0x27,'\'','"'),(0x2B,',','<'),(0x2F,'.','>'),(0x2C,'/','?'),(0x32,'`','~'),
    ];
    let &(_, lo, hi) = T.iter().find(|t| t.0 == code)?;
    let up = if lo.is_ascii_alphabetic() { shift ^ caps } else { shift };
    Some(if up { hi } else { lo })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn tables() {
        assert_eq!(win_vk_to_us(0x4C, false, false), Some('l')); assert_eq!(win_vk_to_us(0xBA, false, false), Some(';'));
        assert_eq!(win_vk_to_us(0x41, true, true), Some('a')); assert_eq!(win_vk_to_us(0x31, true, false), Some('!'));
        assert_eq!(mac_keycode_to_us(0x25, false, false), Some('l')); assert_eq!(mac_keycode_to_us(0x29, false, false), Some(';'));
        assert_eq!(mac_keycode_to_us(0x27, true, false), Some('"'));
        // every printable US char reachable on both platforms
        let mut w = std::collections::HashSet::new(); let mut m = std::collections::HashSet::new();
        for vk in 0..256 { for s in [false, true] { if let Some(c) = win_vk_to_us(vk, s, false) { w.insert(c); } } }
        for k in 0..128 { for s in [false, true] { if let Some(c) = mac_keycode_to_us(k, s, false) { m.insert(c); } } }
        assert_eq!(w.len(), 94); assert_eq!(w, m);
    }
}
