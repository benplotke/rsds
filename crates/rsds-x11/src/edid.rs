//! Make, model and serial number from a monitor's EDID, named the way
//! libdisplay-info (and so wlroots) names them, so a monitor has the same
//! identity in profiles under X11 and Wayland.

const HEADER: [u8; 8] = [0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0];
/// Database of PNP manufacturer IDs, from the hwdata package.
const PNP_IDS: &str = "/usr/share/hwdata/pnp.ids";

pub(crate) struct Edid {
    pub make: String,
    pub model: String,
    pub serial: Option<String>,
}

pub(crate) fn parse(e: &[u8]) -> Option<Edid> {
    if e.len() < 128 || e[..8] != HEADER {
        return None;
    }
    let id = u16::from_be_bytes([e[8], e[9]]);
    let letter = |shift: u16| (b'A' - 1 + ((id >> shift) & 0x1f) as u8) as char;
    let pnp: String = [letter(10), letter(5), letter(0)].into_iter().collect();
    let product = u16::from_le_bytes([e[10], e[11]]);
    let serial_number = u32::from_le_bytes([e[12], e[13], e[14], e[15]]);

    // Display descriptors: 0xfc is the product name, 0xff the serial.
    let mut name = None;
    let mut serial = None;
    for d in e[54..126].as_chunks::<18>().0 {
        if d[..3] != [0, 0, 0] {
            continue;
        }
        let text = || {
            let t = &d[5..];
            let end = t.iter().position(|&b| b == b'\n').unwrap_or(t.len());
            Some(String::from_utf8_lossy(&t[..end]).trim().to_string()).filter(|s| !s.is_empty())
        };
        match d[3] {
            0xfc => name = name.or_else(text),
            0xff => serial = serial.or_else(text),
            _ => {}
        }
    }
    Some(Edid {
        make: manufacturer(&pnp).unwrap_or(pnp),
        model: name.unwrap_or_else(|| format!("0x{product:04X}")),
        serial: serial.or_else(|| (serial_number != 0).then(|| format!("0x{serial_number:08X}"))),
    })
}

/// Manufacturer name for a PNP ID, if hwdata knows it.
fn manufacturer(pnp: &str) -> Option<String> {
    let ids = std::fs::read_to_string(PNP_IDS).ok()?;
    ids.lines()
        .find_map(|l| l.strip_prefix(pnp)?.strip_prefix('\t'))
        .map(|name| name.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edid(id: &[u8; 3], product: u16, serial: u32, descriptors: &[(u8, &str)]) -> Vec<u8> {
        let mut e = vec![0u8; 128];
        e[..8].copy_from_slice(&HEADER);
        let c = |i: usize| (id[i] - b'A' + 1) as u16;
        e[8..10].copy_from_slice(&((c(0) << 10) | (c(1) << 5) | c(2)).to_be_bytes());
        e[10..12].copy_from_slice(&product.to_le_bytes());
        e[12..16].copy_from_slice(&serial.to_le_bytes());
        for (i, (tag, text)) in descriptors.iter().enumerate() {
            let d = &mut e[54 + 18 * i..72 + 18 * i];
            d[3] = *tag;
            let mut t = text.as_bytes().to_vec();
            t.push(b'\n');
            t.resize(13, b' ');
            d[5..].copy_from_slice(&t);
        }
        e
    }

    #[test]
    fn names_and_serial_from_descriptors() {
        let e = parse(&edid(b"ZZZ", 1, 7, &[(0xfc, "U32V3"), (0xff, "NVSLCJA002143")])).unwrap();
        assert_eq!(e.make, manufacturer("ZZZ").unwrap_or_else(|| "ZZZ".into()));
        assert_eq!(e.model, "U32V3");
        assert_eq!(e.serial.as_deref(), Some("NVSLCJA002143"));
    }

    #[test]
    fn falls_back_to_codes() {
        let e = parse(&edid(b"ZZZ", 0x1409, 0, &[])).unwrap();
        assert_eq!(e.model, "0x1409");
        assert_eq!(e.serial, None);
        let e = parse(&edid(b"ZZZ", 1, 0x1234, &[])).unwrap();
        assert_eq!(e.serial.as_deref(), Some("0x00001234"));
        assert!(parse(&[0; 128]).is_none());
    }
}
