//! Transcode Miles Sound System XMI music to a type-0 Standard MIDI File.
//!
//! Port of the CorsixTH transcoder used by Wargus. XMI differs from MIDI in
//! three ways handled here: delta times are sums of bytes below `0x80`, note
//! on events carry their duration (so matching note offs are synthesised),
//! and the tempo is fixed at 120 Hz ticks (hence the `* 3` scaling).

/// Where a token's payload bytes come from.
#[derive(Clone, Copy)]
enum Payload {
    None,
    /// One zero byte (velocity of a synthesised note off).
    Zero,
    /// Bytes copied from the input.
    Input {
        start: usize,
        len: usize,
    },
}

#[derive(Clone, Copy)]
struct Token {
    time: i32,
    kind: u8,
    data: u8,
    payload: Payload,
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn eof(&self) -> bool {
        self.pos >= self.data.len()
    }

    fn u8(&mut self) -> Option<u8> {
        let b = *self.data.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    /// Move by `distance`. Moving past the end clamps to the end (which
    /// terminates parsing); moving before the start fails.
    fn skip(&mut self, distance: i64) -> bool {
        let target = self.pos as i64 + distance;
        if target < 0 {
            return false;
        }
        self.pos = (target as usize).min(self.data.len());
        true
    }

    fn be24(&mut self) -> u32 {
        match (self.u8(), self.u8(), self.u8()) {
            (Some(a), Some(b), Some(c)) => (a as u32) << 16 | (b as u32) << 8 | c as u32,
            _ => 0,
        }
    }

    /// MIDI variable-length quantity (at most 4 bytes); 0 on EOF.
    fn uint_var(&mut self) -> u32 {
        let mut v = 0u32;
        for _ in 0..4 {
            let Some(b) = self.u8() else { return 0 };
            v = (v << 7) | (b & 0x7F) as u32;
            if b & 0x80 == 0 {
                break;
            }
        }
        v
    }

    fn find(&mut self, needle: &[u8]) -> bool {
        match self.data[self.pos..].windows(needle.len()).position(|w| w == needle) {
            Some(p) => {
                self.pos += p;
                true
            }
            None => false,
        }
    }
}

fn write_uint_var(out: &mut Vec<u8>, mut value: u32) {
    let mut buffer = value & 0x7F;
    let mut count = 1;
    loop {
        value >>= 7;
        if value == 0 {
            break;
        }
        buffer = (buffer << 8) | 0x80 | (value & 0x7F);
        count += 1;
    }
    for _ in 0..count {
        out.push(buffer as u8);
        buffer >>= 8;
    }
}

/// Convert XMI data to MIDI. Returns `None` if the input is not a usable
/// XMI file.
pub fn xmi_to_mid(xmi: &[u8]) -> Option<Vec<u8>> {
    let mut input = Reader { data: xmi, pos: 0 };
    if !input.find(b"EVNT") || !input.skip(8) {
        return None;
    }

    let mut tokens: Vec<Token> = Vec::new();
    let mut time: i32 = 0;
    let mut tempo: i32 = 500_000;
    let mut tempo_set = false;
    let mut end = false;

    while !input.eof() && !end {
        let kind = loop {
            let b = input.u8()?;
            if b & 0x80 != 0 {
                break b;
            }
            time += b as i32 * 3;
        };
        let mut token = Token { time, kind, data: 0, payload: Payload::Input { start: input.pos + 1, len: 1 } };
        match kind & 0xF0 {
            0xC0 | 0xD0 => {
                token.data = input.u8()?;
                token.payload = Payload::None;
                tokens.push(token);
            }
            0x80 | 0xA0 | 0xB0 | 0xE0 => {
                token.data = input.u8()?;
                if !input.skip(1) {
                    return None;
                }
                tokens.push(token);
            }
            0x90 => {
                let note = input.u8()?;
                token.data = note;
                if !input.skip(1) {
                    return None;
                }
                tokens.push(token);
                let duration = input.uint_var();
                tokens.push(Token {
                    time: (time as u32).wrapping_add(duration.wrapping_mul(3)) as i32,
                    kind,
                    data: note,
                    payload: Payload::Zero,
                });
            }
            _ => {
                // 0xF0: sysex and meta events.
                let mut meta = 0;
                if kind == 0xFF {
                    meta = input.u8()?;
                    if meta == 0x2F {
                        end = true;
                    } else if meta == 0x51 {
                        if tempo_set {
                            // Only the first tempo event is kept.
                            let len = input.uint_var();
                            if !input.skip(len as i64) {
                                return None;
                            }
                            continue;
                        }
                        input.skip(1);
                        tempo = (input.be24() as i32).wrapping_mul(3);
                        tempo_set = true;
                        input.skip(-4);
                    }
                }
                token.data = meta;
                let len = input.uint_var() as usize;
                let start = input.pos;
                token.payload = Payload::Input { start, len };
                if !input.skip(len as i64) {
                    return None;
                }
                tokens.push(token);
            }
        }
    }

    if tokens.is_empty() {
        return None;
    }

    let mut out = Vec::with_capacity(xmi.len() * 2);
    out.extend_from_slice(b"MThd\0\0\0\x06\0\0\0\x01");
    out.extend_from_slice(&((tempo.wrapping_mul(3) / 25000) as u16).to_be_bytes());
    out.extend_from_slice(b"MTrk\xBA\xAD\xF0\x0D");

    tokens.sort_by_key(|t| t.time);

    let payload = |out: &mut Vec<u8>, p: Payload| match p {
        Payload::None => {}
        Payload::Zero => out.push(0),
        Payload::Input { start, len } => {
            for i in start..start + len {
                out.push(xmi.get(i).copied().unwrap_or(0));
            }
        }
    };

    let mut last_time = 0i32;
    let mut status = 0u8;
    for t in &tokens {
        write_uint_var(&mut out, t.time.wrapping_sub(last_time) as u32);
        last_time = t.time;
        if t.kind >= 0xF0 {
            status = t.kind;
            out.push(t.kind);
            if t.kind == 0xFF {
                out.push(t.data);
            }
            let len = match t.payload {
                Payload::Input { len, .. } => len as u32,
                _ => 0,
            };
            write_uint_var(&mut out, len);
            payload(&mut out, t.payload);
            if t.kind == 0xFF && t.data == 0x2F {
                break;
            }
        } else {
            // Running status: only emit the status byte when it changes.
            if t.kind != status {
                status = t.kind;
                out.push(t.kind);
            }
            out.push(t.data);
            match t.payload {
                Payload::Input { start, .. } => out.push(xmi.get(start).copied().unwrap_or(0)),
                p => payload(&mut out, p),
            }
        }
    }

    let track_len = (out.len() - 22) as u32;
    out[18..22].copy_from_slice(&track_len.to_be_bytes());
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uint_var_encoding() {
        let enc = |v| {
            let mut o = Vec::new();
            write_uint_var(&mut o, v);
            o
        };
        assert_eq!(enc(0), [0x00]);
        assert_eq!(enc(0x7F), [0x7F]);
        assert_eq!(enc(0x80), [0x81, 0x00]);
        assert_eq!(enc(0x3FFF), [0xFF, 0x7F]);
    }

    #[test]
    fn transcodes_note_with_duration() {
        let mut xmi = b"FORM....XDIRINFO..CAT ....XMIDFORM....TIMBEVNT".to_vec();
        xmi.extend(4u32.to_be_bytes()); // EVNT chunk length (skipped)
        xmi.extend([0x90, 60, 100, 10]); // note on, duration 10 ticks
        xmi.extend([5]); // delay 5 ticks
        xmi.extend([0xFF, 0x2F, 0x00]); // end of track
        let mid = xmi_to_mid(&xmi).unwrap();

        assert_eq!(&mid[..14], b"MThd\0\0\0\x06\0\0\0\x01\x00\x3C"); // 500000*3/25000 = 60
        assert_eq!(&mid[14..18], b"MTrk");
        let track = &mid[22..];
        // t=0 note on; t=15 end of track (note off at t=30 is dropped after it)
        assert_eq!(track, [0x00, 0x90, 60, 100, 15, 0xFF, 0x2F, 0x00]);
        assert_eq!(u32::from_be_bytes(mid[18..22].try_into().unwrap()), track.len() as u32);
    }

    #[test]
    fn note_off_before_end() {
        let mut xmi = b"EVNT".to_vec();
        xmi.extend(4u32.to_be_bytes());
        xmi.extend([0x91, 64, 90, 2]); // note on, duration 2
        xmi.extend([10]); // delay
        xmi.extend([0xFF, 0x2F, 0x00]);
        let mid = xmi_to_mid(&xmi).unwrap();
        // note on t=0, note off (running status, velocity 0) t=6, end t=30
        assert_eq!(&mid[22..], [0x00, 0x91, 64, 90, 6, 64, 0, 24, 0xFF, 0x2F, 0x00]);
    }

    #[test]
    fn rejects_non_xmi() {
        assert!(xmi_to_mid(b"not music").is_none());
    }
}
