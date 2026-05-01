//! Utility functions: datestamp conversion, protection bits, charset.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Amiga epoch: 1978-01-01 00:00:00 UTC.
/// Offset from Unix epoch (1970-01-01) in seconds = 2922 days.
const AMIGA_EPOCH_OFFSET: u64 = 2922 * 86400;

/// Convert Amiga datestamp (days, minutes, ticks) to SystemTime.
/// Ticks are 1/50th of a second.
pub fn amiga_to_systime(days: u16, minutes: u16, ticks: u16) -> SystemTime {
    let secs = AMIGA_EPOCH_OFFSET + days as u64 * 86400 + minutes as u64 * 60 + ticks as u64 / 50;
    let nanos = (ticks as u64 % 50) * 20_000_000; // 1/50s = 20ms
    UNIX_EPOCH + Duration::new(secs, nanos as u32)
}

/// Convert Amiga datestamp to a human-readable string.
pub fn amiga_date_string(days: u16, minutes: u16, ticks: u16) -> String {
    let t = amiga_to_systime(days, minutes, ticks);
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => {
            let secs = d.as_secs();
            let time_of_day = secs % 86400;
            let hours = time_of_day / 3600;
            let mins = (time_of_day % 3600) / 60;
            let s = time_of_day % 60;
            let (y, m, d) = days_to_ymd((secs / 86400) as u32);
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                y, m, d, hours, mins, s
            )
        }
        Err(_) => "invalid".into(),
    }
}

/// Convert days since Unix epoch (1970-01-01) to (year, month, day).
fn days_to_ymd(days: u32) -> (u32, u32, u32) {
    // Civil calendar algorithm from Howard Hinnant
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Convert Amiga protection bits to Unix mode.
///
/// Amiga bits (byte): HSPARWED
///   H=hidden, S=script, P=pure, A=archive, R=read, W=write, E=execute, D=delete
///   SET = DENIED (inverted logic for RWED)
///
/// We map: R→r, W→w, E→x for owner. Group/other get same as owner.
pub fn amiga_protection_to_mode(prot: u8, is_dir: bool) -> u32 {
    let mut mode: u32 = if is_dir { 0o40000 } else { 0o100000 };
    // Amiga: bit set = denied. Bit 3=R, 2=W, 1=E, 0=D
    if prot & 0x08 == 0 {
        mode |= 0o444;
    } // read
    if prot & 0x04 == 0 {
        mode |= 0o222;
    } // write
    if prot & 0x02 == 0 {
        mode |= 0o111;
    } // execute
    mode
}

/// Convert Unix file mode to Amiga protection bits.
///
/// Maps owner permissions: r→R, w→W, x→E. Delete bit follows write.
/// Amiga logic is inverted: bit SET = DENIED.
pub fn unix_mode_to_amiga_protection(mode: u32) -> u8 {
    let mut prot: u8 = 0x0F; // all denied
    if mode & 0o400 != 0 {
        prot &= !0x08;
    } // read
    if mode & 0o200 != 0 {
        prot &= !0x04;
        prot &= !0x01;
    } // write + delete
    if mode & 0o100 != 0 {
        prot &= !0x02;
    } // execute
    prot
}

/// Format Amiga protection bits as a human-readable string (e.g. "----rwed").
///
/// Bits: H=hidden, S=script, P=pure, A=archive, R=read, W=write, E=execute, D=delete.
/// Lowercase = granted, dash = denied.
pub fn amiga_protection_string(prot: u8) -> String {
    let mut s = String::with_capacity(8);
    s.push(if prot & 0x80 != 0 { 'h' } else { '-' });
    s.push(if prot & 0x40 != 0 { 's' } else { '-' });
    s.push(if prot & 0x20 != 0 { 'p' } else { '-' });
    s.push(if prot & 0x10 != 0 { 'a' } else { '-' });
    // RWED: inverted — bit SET = denied
    s.push(if prot & 0x08 == 0 { 'r' } else { '-' });
    s.push(if prot & 0x04 == 0 { 'w' } else { '-' });
    s.push(if prot & 0x02 == 0 { 'e' } else { '-' });
    s.push(if prot & 0x01 == 0 { 'd' } else { '-' });
    s
}

/// Parse an Amiga protection spec string into a protection byte.
///
/// Supports absolute ("rwed", "hsparwed"), additive ("+p"), and subtractive ("-wd") specs.
/// Returns `None` if the spec contains invalid characters.
pub fn parse_amiga_protection(current: u8, spec: &str) -> Option<u8> {
    if spec.is_empty() {
        return None;
    }
    let (mode, chars) = if let Some(rest) = spec.strip_prefix('+') {
        ('+', rest)
    } else if let Some(rest) = spec.strip_prefix('-') {
        ('-', rest)
    } else {
        ('=', spec)
    };
    let mut prot = if mode == '=' { 0x0Fu8 } else { current };
    for ch in chars.chars() {
        match (ch, mode) {
            ('h', '-') => prot &= !0x80,
            ('h', _) => prot |= 0x80,
            ('s', '-') => prot &= !0x40,
            ('s', _) => prot |= 0x40,
            ('p', '-') => prot &= !0x20,
            ('p', _) => prot |= 0x20,
            ('a', '-') => prot &= !0x10,
            ('a', _) => prot |= 0x10,
            ('r', '-') => prot |= 0x08,
            ('r', _) => prot &= !0x08,
            ('w', '-') => prot |= 0x04,
            ('w', _) => prot &= !0x04,
            ('e', '-') => prot |= 0x02,
            ('e', _) => prot &= !0x02,
            ('d', '-') => prot |= 0x01,
            ('d', _) => prot &= !0x01,
            _ => return None,
        }
    }
    Some(prot)
}

/// Convert Latin-1 (ISO 8859-1) bytes to a Rust String (UTF-8).
pub fn latin1_to_string(data: &[u8]) -> String {
    data.iter().map(|&b| b as char).collect()
}

/// Case-insensitive comparison for Amiga filenames (Latin-1).
pub fn name_eq_ci(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// Return the current time as an Amiga datestamp (days, minutes, ticks).
pub fn current_amiga_datestamp() -> (u16, u16, u16) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let amiga_secs = secs.saturating_sub(AMIGA_EPOCH_OFFSET);
    let days = (amiga_secs / 86400) as u16;
    let mins = ((amiga_secs % 86400) / 60) as u16;
    let ticks = ((amiga_secs % 60) * 50) as u16;
    (days, mins, ticks)
}

/// Join a parent path and a filename with a single '/'.
pub fn join_pfs3_path(parent: &str, name: &str) -> String {
    if parent.ends_with('/') {
        format!("{}{}", parent, name)
    } else {
        format!("{}/{}", parent, name)
    }
}
