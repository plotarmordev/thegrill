//! Container framing and sample metadata only; sample values are not decoded.
//! Durations use integer division, rounding down to whole microseconds.

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Checked {
    pub channels: u16,
    pub sample_rate: u32,
    pub duration_us: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Failure {
    BadSignature,
    Truncated,
    InvalidLength,
    TrailingBytes,
    MissingFmt,
    DuplicateFmt,
    MissingData,
    DuplicateData,
    UnsupportedFormat,
    InvalidFormat,
    InvalidBlockAlign,
    InvalidByteRate,
    InvalidDataLength,
}

struct Format {
    channels: u16,
    sample_rate: u32,
    block_align: u16,
}

fn u16_le(bytes: &[u8]) -> u16 {
    u16::from_le_bytes(bytes[..2].try_into().unwrap())
}

fn u32_le(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes[..4].try_into().unwrap())
}

fn format(bytes: &[u8]) -> Result<Format, Failure> {
    if bytes.len() < 16 {
        return Err(Failure::InvalidLength);
    }
    if bytes.len() != 16
        && (bytes.len() < 18 || usize::from(u16_le(&bytes[16..])) != bytes.len() - 18)
    {
        return Err(Failure::InvalidLength);
    }
    let mut tag = u16_le(bytes);
    if tag == 0xfffe {
        if bytes.len() < 40 || u16_le(&bytes[16..]) < 22 {
            return Err(Failure::InvalidLength);
        }
        // The GUID's remaining bytes distinguish WAVE subformats from arbitrary GUIDs.
        if bytes[26..40] != [0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71] {
            return Err(Failure::UnsupportedFormat);
        }
        tag = u16_le(&bytes[24..]);
        let valid_bits = u16_le(&bytes[18..]);
        if valid_bits == 0 || valid_bits > u16_le(&bytes[14..]) {
            return Err(Failure::InvalidFormat);
        }
    }
    if !matches!(tag, 1 | 3) {
        return Err(Failure::UnsupportedFormat);
    }
    let channels = u16_le(&bytes[2..]);
    let sample_rate = u32_le(&bytes[4..]);
    let block_align = u16_le(&bytes[12..]);
    let bits = u16_le(&bytes[14..]);
    if channels == 0
        || sample_rate == 0
        || bits == 0
        || !bits.is_multiple_of(8)
        || (tag == 3 && !matches!(bits, 32 | 64))
    {
        return Err(Failure::InvalidFormat);
    }
    if u32::from(block_align) != u32::from(channels) * u32::from(bits / 8) {
        return Err(Failure::InvalidBlockAlign);
    }
    if u64::from(u32_le(&bytes[8..])) != u64::from(sample_rate) * u64::from(block_align) {
        return Err(Failure::InvalidByteRate);
    }
    Ok(Format {
        channels,
        sample_rate,
        block_align,
    })
}

pub(crate) fn check(bytes: &[u8]) -> Result<Checked, Failure> {
    if bytes.len() < 12 {
        return Err(Failure::Truncated);
    }
    if &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(Failure::BadSignature);
    }
    let length = u64::from(u32_le(&bytes[4..])) + 8;
    if length < 12 {
        return Err(Failure::InvalidLength);
    }
    if length > bytes.len() as u64 {
        return Err(Failure::Truncated);
    }
    let length = length as usize;
    // Tolerate a final zero pad outside the declared RIFF extent.
    if bytes.len() != length && bytes[length..] != [0] {
        return Err(Failure::TrailingBytes);
    }
    let mut rest = &bytes[12..length];
    let mut fmt = None;
    let mut data_len = None;
    while !rest.is_empty() {
        if rest.len() < 8 {
            return Err(Failure::Truncated);
        }
        let size = u32_le(&rest[4..]) as usize;
        let payload = &rest[8..];
        if size > payload.len() {
            return Err(Failure::Truncated);
        }
        // Some writers omit the final chunk pad or place it outside the RIFF extent.
        let padding = if size == payload.len() { 0 } else { size % 2 };
        match &rest[..4] {
            b"fmt " => {
                if fmt.is_some() {
                    return Err(Failure::DuplicateFmt);
                }
                fmt = Some(format(&payload[..size])?);
            }
            b"data" => {
                if fmt.is_none() {
                    return Err(Failure::MissingFmt);
                }
                if data_len.replace(size).is_some() {
                    return Err(Failure::DuplicateData);
                }
            }
            _ => {}
        }
        rest = &payload[size + padding..];
    }
    let format = fmt.ok_or(Failure::MissingFmt)?;
    let data_len = data_len.ok_or(Failure::MissingData)?;
    if !data_len.is_multiple_of(usize::from(format.block_align)) {
        return Err(Failure::InvalidDataLength);
    }
    let frames = (data_len / usize::from(format.block_align)) as u64;
    // RIFF lengths are bounded by u32, so the product fits in u64.
    Ok(Checked {
        channels: format.channels,
        sample_rate: format.sample_rate,
        duration_us: frames * 1_000_000 / u64::from(format.sample_rate),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(tag: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut bytes = tag.to_vec();
        bytes.extend((data.len() as u32).to_le_bytes());
        bytes.extend(data);
        if !data.len().is_multiple_of(2) {
            bytes.push(0);
        }
        bytes
    }

    fn riff(chunks: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend((4 + chunks.iter().map(Vec::len).sum::<usize>() as u32).to_le_bytes());
        bytes.extend(b"WAVE");
        for chunk in chunks {
            bytes.extend(chunk);
        }
        bytes
    }

    fn fmt(tag: u16, channels: u16, bits: u16) -> Vec<u8> {
        let align = channels * (bits / 8);
        let mut bytes = tag.to_le_bytes().to_vec();
        bytes.extend(channels.to_le_bytes());
        bytes.extend(44_100u32.to_le_bytes());
        bytes.extend((44_100 * u32::from(align)).to_le_bytes());
        bytes.extend(align.to_le_bytes());
        bytes.extend(bits.to_le_bytes());
        bytes
    }

    fn wav(fmt: &[u8], data: &[u8]) -> Vec<u8> {
        riff(&[chunk(b"fmt ", fmt), chunk(b"data", data)])
    }

    #[test]
    fn pcm_float_and_extensible_metadata() {
        for (tag, channels, bits) in [(1, 1, 16), (1, 2, 16), (3, 2, 32)] {
            let data = vec![0; usize::from(channels * bits / 8)];
            let mut format = fmt(tag, channels, bits);
            for extensible in [false, true] {
                if extensible {
                    format[..2].copy_from_slice(&0xfffeu16.to_le_bytes());
                    format.extend(22u16.to_le_bytes());
                    format.extend(bits.to_le_bytes());
                    format.extend(0u32.to_le_bytes());
                    format.extend(u32::from(tag).to_le_bytes());
                    format.extend([0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71]);
                }
                assert_eq!(
                    check(&wav(&format, &data)),
                    Ok(Checked {
                        channels,
                        sample_rate: 44_100,
                        duration_us: 22
                    })
                );
            }
        }
        assert_eq!(check(&wav(&fmt(1, 1, 16), &[])).unwrap().duration_us, 0);
    }

    #[test]
    fn malformed_containers_report_typed_failures() {
        let format = fmt(1, 1, 16);
        let valid = wav(&format, &[0, 0]);
        let mut bad_signature = valid.clone();
        bad_signature[0] = 0;
        let mut short_riff = valid.clone();
        short_riff[4..8].copy_from_slice(&0u32.to_le_bytes());
        let mut trailing = valid.clone();
        trailing.extend([0, 0]);
        let mut malformed_chunk = chunk(b"JUNK", &[]);
        malformed_chunk[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        let cases = [
            (bad_signature, Failure::BadSignature),
            (valid[..valid.len() - 1].to_vec(), Failure::Truncated),
            (short_riff, Failure::InvalidLength),
            (trailing, Failure::TrailingBytes),
            (
                riff(&[chunk(b"data", &[]), chunk(b"fmt ", &format)]),
                Failure::MissingFmt,
            ),
            (
                riff(&[chunk(b"fmt ", &format), chunk(b"fmt ", &format)]),
                Failure::DuplicateFmt,
            ),
            (riff(&[chunk(b"fmt ", &format)]), Failure::MissingData),
            (
                riff(&[
                    chunk(b"fmt ", &format),
                    chunk(b"data", &[]),
                    chunk(b"data", &[]),
                ]),
                Failure::DuplicateData,
            ),
            (wav(&fmt(2, 1, 16), &[]), Failure::UnsupportedFormat),
            (wav(&fmt(1, 0, 16), &[]), Failure::InvalidFormat),
            (wav(&format, &[0]), Failure::InvalidDataLength),
            (riff(&[malformed_chunk]), Failure::Truncated),
        ];
        for (bytes, failure) in cases {
            assert_eq!(check(&bytes), Err(failure));
        }
        for (offset, failure) in [
            (12, Failure::InvalidBlockAlign),
            (8, Failure::InvalidByteRate),
            (4, Failure::InvalidFormat),
            (14, Failure::InvalidFormat),
        ] {
            let mut broken = format.clone();
            let width = if offset == 4 || offset == 8 { 4 } else { 2 };
            broken[offset..offset + width].fill(0);
            assert_eq!(check(&wav(&broken, &[])), Err(failure));
        }
    }

    #[test]
    fn padding_and_chunk_boundaries() {
        let format = chunk(b"fmt ", &fmt(1, 1, 8));
        let data = chunk(b"data", &[0]);
        let bytes = riff(&[chunk(b"JUNK", &[1, 2, 3]), format.clone(), data.clone()]);
        assert_eq!(check(&bytes).unwrap().duration_us, 22);
        let mut padded = bytes.clone();
        padded.push(0);
        assert!(check(&padded).is_ok());
        *padded.last_mut().unwrap() = 1;
        assert_eq!(check(&padded), Err(Failure::TrailingBytes));
        let unpadded = riff(&[format.clone(), data[..data.len() - 1].to_vec()]);
        assert_eq!(check(&unpadded), check(&bytes));
        let mut external_pad = unpadded.clone();
        external_pad.push(0);
        assert_eq!(check(&external_pad), check(&bytes));
        let mut truncated = unpadded.clone();
        truncated.pop();
        assert_eq!(check(&truncated), Err(Failure::Truncated));
        assert_eq!(
            check(&riff(&[format.clone(), data[..data.len() - 2].to_vec()])),
            Err(Failure::Truncated)
        );
        for tag in [b"JUNK", b"LIST"] {
            let chunk = chunk(tag, &[0]);
            let mut final_chunk = riff(&[
                format.clone(),
                data.clone(),
                chunk[..chunk.len() - 1].to_vec(),
            ]);
            assert_eq!(check(&final_chunk), check(&bytes));
            final_chunk.push(0);
            assert_eq!(check(&final_chunk), check(&bytes));
            assert_eq!(
                check(&riff(&[
                    format.clone(),
                    data.clone(),
                    chunk[..chunk.len() - 2].to_vec(),
                ])),
                Err(Failure::Truncated)
            );
        }
        assert_eq!(
            check(&riff(&[format, data, vec![0; 7]])),
            Err(Failure::Truncated)
        );
    }

    #[test]
    fn invalid_format_extensions_and_widths() {
        assert_eq!(check(&wav(&[0; 15], &[])), Err(Failure::InvalidLength));
        for extra in [1, 2] {
            let mut format = fmt(1, 1, 16);
            format.extend(vec![1; extra]);
            assert_eq!(check(&wav(&format, &[])), Err(Failure::InvalidLength));
        }
        assert_eq!(
            check(&wav(&fmt(0xfffe, 1, 16), &[])),
            Err(Failure::InvalidLength)
        );
        assert_eq!(
            check(&wav(&fmt(3, 1, 16), &[])),
            Err(Failure::InvalidFormat)
        );
        assert_eq!(
            check(&wav(&fmt(1, 1, 12), &[])),
            Err(Failure::InvalidFormat)
        );
        let mut format = fmt(0xfffe, 1, 16);
        format.extend(22u16.to_le_bytes());
        format.extend(16u16.to_le_bytes());
        format.extend(0u32.to_le_bytes());
        format.extend(1u32.to_le_bytes());
        format.extend([0; 12]);
        assert_eq!(check(&wav(&format, &[])), Err(Failure::UnsupportedFormat));
        format[28..40].copy_from_slice(&[0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71]);
        format[18..20].copy_from_slice(&17u16.to_le_bytes());
        assert_eq!(check(&wav(&format, &[])), Err(Failure::InvalidFormat));
    }
}
