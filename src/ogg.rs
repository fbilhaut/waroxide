//! Encode ripped CD audio (16-bit PCM WAV) to Ogg Vorbis, without ffmpeg.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom};
use std::num::{NonZeroU32, NonZeroU8};
use std::path::Path;

use anyhow::{bail, ensure, Context, Result};
use vorbis_rs::{VorbisBitrateManagementStrategy, VorbisEncoderBuilder};

/// Vorbis quality (-0.2..1). 0.4 is about 128 kbit/s for CD audio.
const QUALITY: f32 = 0.4;

/// Samples per channel handed to the encoder at a time.
const BLOCK: usize = 4096;

struct WavFormat {
    channels: u16,
    rate: u32,
    data_len: u64,
}

/// Find the format and position the reader at the start of the samples.
fn read_header(r: &mut (impl Read + Seek)) -> Result<WavFormat> {
    let mut riff = [0u8; 12];
    r.read_exact(&mut riff).context("not a WAV file")?;
    ensure!(&riff[..4] == b"RIFF" && &riff[8..] == b"WAVE", "not a WAV file");
    let mut format = None;
    loop {
        let mut chunk = [0u8; 8];
        r.read_exact(&mut chunk).context("WAV file has no data chunk")?;
        let len = u32::from_le_bytes(chunk[4..].try_into().unwrap()) as u64;
        match &chunk[..4] {
            b"fmt " => {
                let mut f = vec![0u8; len as usize];
                r.read_exact(&mut f)?;
                ensure!(f.len() >= 16, "truncated WAV format chunk");
                let tag = u16::from_le_bytes([f[0], f[1]]);
                let bits = u16::from_le_bytes([f[14], f[15]]);
                ensure!(tag == 1 && bits == 16, "only 16-bit PCM WAV files are supported");
                format = Some((u16::from_le_bytes([f[2], f[3]]), u32::from_le_bytes(f[4..8].try_into().unwrap())));
                if len % 2 == 1 {
                    r.seek(SeekFrom::Current(1))?;
                }
            }
            b"data" => {
                let Some((channels, rate)) = format else { bail!("WAV data before its format") };
                return Ok(WavFormat { channels, rate, data_len: len });
            }
            _ => {
                r.seek(SeekFrom::Current((len + len % 2) as i64))?;
            }
        }
    }
}

/// Encode the WAV file `wav` to `ogg`.
pub fn encode_wav(wav: &Path, ogg: &Path) -> Result<()> {
    let mut input = BufReader::new(File::open(wav).with_context(|| format!("cannot open {}", wav.display()))?);
    let fmt = read_header(&mut input).with_context(|| wav.display().to_string())?;
    let channels = NonZeroU8::new(u8::try_from(fmt.channels)?).context("WAV file has no channels")?;
    let rate = NonZeroU32::new(fmt.rate).context("WAV file has a zero sample rate")?;

    let out = BufWriter::new(File::create(ogg).with_context(|| format!("cannot create {}", ogg.display()))?);
    let mut encoder = VorbisEncoderBuilder::new(rate, channels, out)?
        .bitrate_management_strategy(VorbisBitrateManagementStrategy::QualityVbr { target_quality: QUALITY })
        .build()?;

    let ch = fmt.channels as usize;
    let mut raw = vec![0u8; BLOCK * ch * 2];
    let mut planes = vec![Vec::with_capacity(BLOCK); ch];
    let mut left = fmt.data_len;
    while left > 0 {
        let want = (raw.len() as u64).min(left) as usize;
        let n = read_up_to(&mut input, &mut raw[..want])?;
        if n == 0 {
            break; // truncated file: keep what we have
        }
        left -= n as u64;
        planes.iter_mut().for_each(Vec::clear);
        for frame in raw[..n - n % (ch * 2)].chunks_exact(ch * 2) {
            for (c, plane) in planes.iter_mut().enumerate() {
                let s = i16::from_le_bytes([frame[c * 2], frame[c * 2 + 1]]);
                plane.push(s as f32 / 32768.0);
            }
        }
        encoder.encode_audio_block(&planes)?;
    }
    encoder.finish()?;
    Ok(())
}

fn read_up_to(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_a_tone() {
        let dir = std::env::temp_dir().join(format!("orcrust-ogg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let samples: Vec<u8> = (0..44100)
            .flat_map(|i| {
                let s = ((i as f32 * 440.0 * std::f32::consts::TAU / 44100.0).sin() * 8000.0) as i16;
                [s.to_le_bytes(), s.to_le_bytes()].concat()
            })
            .collect();
        let mut wav = b"RIFF".to_vec();
        wav.extend((36 + samples.len() as u32).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend([1, 0, 2, 0]);
        wav.extend(44100u32.to_le_bytes());
        wav.extend((44100u32 * 4).to_le_bytes());
        wav.extend([4, 0, 16, 0]);
        wav.extend(b"data");
        wav.extend((samples.len() as u32).to_le_bytes());
        wav.extend(&samples);
        std::fs::write(dir.join("t.wav"), wav).unwrap();

        encode_wav(&dir.join("t.wav"), &dir.join("t.ogg")).unwrap();
        let ogg = std::fs::read(dir.join("t.ogg")).unwrap();
        assert_eq!(&ogg[..4], b"OggS");
        assert!(ogg.windows(6).any(|w| w == b"vorbis"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_non_pcm16() {
        let mut wav = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0".to_vec();
        wav.extend([1, 0, 2, 0, 0x44, 0xAC, 0, 0, 0, 0, 0, 0, 4, 0, 8, 0]);
        assert!(read_header(&mut std::io::Cursor::new(wav)).is_err());
    }
}
