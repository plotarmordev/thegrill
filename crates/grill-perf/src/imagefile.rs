//! `container_checked` validates framing and dimensions, never decoded pixels.
use crate::media::Container;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Checked {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Failure {
    BadSignature,
    Truncated,
    CrcMismatch,
    MissingIhdr,
    MissingIend,
    TrailingBytes,
    MissingSof,
    MissingEoi,
    ZeroDimension,
    InvalidLength,
    InvalidMarker,
    DuplicateIhdr,
    InvalidHeader,
    MissingIdat,
    MissingSos,
}

pub(crate) fn check(bytes: &[u8], kind: Container) -> Result<Checked, Failure> {
    match kind {
        Container::Png => png(bytes),
        Container::Jpeg => jpeg(bytes),
    }
}

const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const CRC_TABLE: [u32; 256] = {
    let mut table = [0; 256];
    let mut i = 0;
    while i < table.len() {
        let mut value = i as u32;
        let mut bit = 0;
        while bit < 8 {
            value = (value >> 1) ^ (0xedb8_8320 & (0u32.wrapping_sub(value & 1)));
            bit += 1;
        }
        table[i] = value;
        i += 1;
    }
    table
};
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc = CRC_TABLE[((crc ^ u32::from(*byte)) & 255) as usize] ^ (crc >> 8);
    }
    !crc
}
fn u32_be(bytes: &[u8]) -> u32 {
    u32::from_be_bytes(bytes[..4].try_into().unwrap())
}
fn dimensions(width: u32, height: u32) -> Result<Checked, Failure> {
    if width == 0 || height == 0 {
        return Err(Failure::ZeroDimension);
    }
    Ok(Checked { width, height })
}
fn png(bytes: &[u8]) -> Result<Checked, Failure> {
    let mut rest = bytes
        .strip_prefix(PNG_SIGNATURE)
        .ok_or(Failure::BadSignature)?;
    let mut checked = None;
    let mut has_idat = false;
    while !rest.is_empty() {
        if rest.len() < 12 {
            return Err(Failure::Truncated);
        }
        let length = u32_be(rest);
        if length > 0x7fff_ffff {
            return Err(Failure::InvalidLength);
        }
        let length = length as usize;
        if length > rest.len() - 12 {
            return Err(Failure::Truncated);
        }
        let tag = &rest[4..8];
        let data = &rest[8..8 + length];
        if crc32(&rest[4..8 + length]) != u32_be(&rest[8 + length..]) {
            return Err(Failure::CrcMismatch);
        }
        if tag == b"IHDR" && checked.is_some() {
            return Err(Failure::DuplicateIhdr);
        }
        if checked.is_none() {
            if tag != b"IHDR" {
                return Err(Failure::MissingIhdr);
            }
            if length != 13 {
                return Err(Failure::InvalidLength);
            }
            let valid_depth = match data[9] {
                0 => matches!(data[8], 1 | 2 | 4 | 8 | 16),
                2 | 4 | 6 => matches!(data[8], 8 | 16),
                3 => matches!(data[8], 1 | 2 | 4 | 8),
                _ => false,
            };
            if !valid_depth || data[10] != 0 || data[11] != 0 || data[12] > 1 {
                return Err(Failure::InvalidHeader);
            }
            checked = Some(dimensions(u32_be(data), u32_be(&data[4..]))?);
        }
        has_idat |= tag == b"IDAT";
        rest = &rest[length + 12..];
        if tag == b"IEND" {
            if length != 0 {
                return Err(Failure::InvalidLength);
            }
            if !rest.is_empty() {
                return Err(Failure::TrailingBytes);
            }
            if !has_idat {
                return Err(Failure::MissingIdat);
            }
            return checked.ok_or(Failure::MissingIhdr);
        }
    }
    Err(if checked.is_some() {
        Failure::MissingIend
    } else {
        Failure::MissingIhdr
    })
}
fn jpeg(bytes: &[u8]) -> Result<Checked, Failure> {
    let mut rest = bytes
        .strip_prefix(&[0xff, 0xd8])
        .ok_or(Failure::BadSignature)?;
    let mut checked = None;
    while !rest.is_empty() {
        if rest[0] != 0xff {
            return Err(Failure::InvalidMarker);
        }
        while rest.first() == Some(&0xff) {
            rest = &rest[1..];
        }
        let marker = *rest.first().ok_or(Failure::Truncated)?;
        rest = &rest[1..];
        match marker {
            0xd9 => {
                if !rest.is_empty() {
                    return Err(Failure::TrailingBytes);
                }
                return Err(if checked.is_some() {
                    Failure::MissingSos
                } else {
                    Failure::MissingSof
                });
            }
            0x01 | 0xd0..=0xd7 => continue,
            0x00 | 0xd8 => return Err(Failure::InvalidMarker),
            _ => {}
        }
        if rest.len() < 2 {
            return Err(Failure::Truncated);
        }
        let length = usize::from(u16::from_be_bytes([rest[0], rest[1]]));
        if length < 2 {
            return Err(Failure::InvalidLength);
        }
        if length > rest.len() {
            return Err(Failure::Truncated);
        }
        let data = &rest[2..length];
        rest = &rest[length..];
        if (0xc0..=0xcf).contains(&marker)
            && ![0xc4, 0xc8, 0xcc].contains(&marker)
            && checked.is_none()
        {
            if data.len() < 6 {
                return Err(Failure::Truncated);
            }
            if data[5] == 0 || length != 8 + 3 * usize::from(data[5]) {
                return Err(Failure::InvalidLength);
            }
            checked = Some(dimensions(
                u32::from(u16::from_be_bytes([data[3], data[4]])),
                u32::from(u16::from_be_bytes([data[1], data[2]])),
            )?);
        }
        if marker == 0xda {
            if !rest.ends_with(&[0xff, 0xd9]) {
                return Err(Failure::MissingEoi);
            }
            return checked.ok_or(Failure::MissingSof);
        }
    }
    Err(Failure::MissingEoi)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(tag: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut bytes = (data.len() as u32).to_be_bytes().to_vec();
        bytes.extend(tag);
        bytes.extend(data);
        bytes.extend(crc32(&bytes[4..]).to_be_bytes());
        bytes
    }
    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut data = width.to_be_bytes().to_vec();
        data.extend(height.to_be_bytes());
        data.extend([8, 2, 0, 0, 0]);
        let mut bytes = PNG_SIGNATURE.to_vec();
        bytes.extend(chunk(b"IHDR", &data));
        bytes
    }
    fn segment(marker: u8, data: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0xff, marker];
        bytes.extend(((data.len() + 2) as u16).to_be_bytes());
        bytes.extend(data);
        bytes
    }
    fn jpeg_header(marker: u8, width: u16, height: u16) -> Vec<u8> {
        let mut data = vec![8];
        data.extend(height.to_be_bytes());
        data.extend(width.to_be_bytes());
        data.extend([1, 1, 0x11, 0]);
        let mut bytes = vec![0xff, 0xd8];
        bytes.extend(segment(0xe1, b"synthetic APP metadata"));
        bytes.extend(segment(marker, &data));
        bytes
    }
    #[test]
    fn png_checks_dimensions_and_all_chunk_crcs() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(
            chunk(b"IEND", &[]),
            [0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130]
        );
        let mut bytes = png_header(17, 29);
        bytes.extend(chunk(b"IDAT", &[]));
        bytes.extend(chunk(b"IEND", &[]));
        assert_eq!(
            check(&bytes, Container::Png),
            Ok(Checked {
                width: 17,
                height: 29
            })
        );
        for index in [16, 37, bytes.len() - 1] {
            let mut corrupt = bytes.clone();
            corrupt[index] ^= 1;
            assert_eq!(check(&corrupt, Container::Png), Err(Failure::CrcMismatch));
        }
    }
    #[test]
    fn png_failures_are_specific() {
        let cases = [
            (vec![], Failure::BadSignature),
            (
                [PNG_SIGNATURE.as_slice(), &[0]].concat(),
                Failure::Truncated,
            ),
            (PNG_SIGNATURE.to_vec(), Failure::MissingIhdr),
            (
                [PNG_SIGNATURE.as_slice(), &chunk(b"IDAT", &[])].concat(),
                Failure::MissingIhdr,
            ),
            (png_header(17, 29), Failure::MissingIend),
            (png_header(0, 29), Failure::ZeroDimension),
            (png_header(17, 0), Failure::ZeroDimension),
            (
                [PNG_SIGNATURE.as_slice(), &chunk(b"IHDR", &[0; 12])].concat(),
                Failure::InvalidLength,
            ),
            (
                [
                    PNG_SIGNATURE.as_slice(),
                    &[0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                ]
                .concat(),
                Failure::InvalidLength,
            ),
            (
                [png_header(17, 29), chunk(b"IEND", &[0])].concat(),
                Failure::InvalidLength,
            ),
            (
                [png_header(17, 29), chunk(b"IEND", &[]), vec![0]].concat(),
                Failure::TrailingBytes,
            ),
            (
                [
                    PNG_SIGNATURE.as_slice(),
                    &[0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 0],
                ]
                .concat(),
                Failure::Truncated,
            ),
        ];
        for (bytes, reason) in cases {
            assert_eq!(check(&bytes, Container::Png), Err(reason), "{reason:?}");
        }
    }
    #[test]
    fn jpeg_walks_metadata_standalone_markers_and_sof_variants() {
        for marker in 0xc0..=0xcf {
            let mut bytes = jpeg_header(marker, 17, 29);
            bytes.extend([0xff, 0x01, 0xff, 0xd0, 0xff, 0xff, 0xd7]);
            bytes.extend(segment(0xda, &[1, 1, 0, 0, 63, 0]));
            bytes.extend([1, 0xff, 0, 0xd9, 2, 0xff, 0xd9]);
            let expected = if [0xc4, 0xc8, 0xcc].contains(&marker) {
                Err(Failure::MissingSof)
            } else {
                Ok(Checked {
                    width: 17,
                    height: 29,
                })
            };
            assert_eq!(check(&bytes, Container::Jpeg), expected);
        }
        let mut bytes = jpeg_header(0xc0, 17, 29);
        bytes.extend(segment(0xc2, &[8, 0, 3, 0, 4, 0]));
        bytes.extend([0xff, 0xd9]);
        assert_eq!(check(&bytes, Container::Jpeg), Err(Failure::MissingSos));
        bytes.truncate(bytes.len() - 2);
        bytes.extend(segment(0xda, &[1, 1, 0, 0, 63, 0]));
        bytes.extend([0xff, 0xd9]);
        assert_eq!(check(&bytes, Container::Jpeg).unwrap().width, 17);
    }
    #[test]
    fn jpeg_failures_are_specific() {
        let cases = [
            (vec![0xff, 0], Failure::BadSignature),
            (vec![0xff, 0xd8, 0xff], Failure::Truncated),
            (vec![0xff, 0xd8, 0xff, 0xe0, 0], Failure::Truncated),
            (vec![0xff, 0xd8, 0xff, 0xe0, 0, 8], Failure::Truncated),
            (vec![0xff, 0xd8, 0xff, 0xe0, 0, 1], Failure::InvalidLength),
            (vec![0xff, 0xd8, 0], Failure::InvalidMarker),
            (vec![0xff, 0xd8, 0xff, 0], Failure::InvalidMarker),
            (vec![0xff, 0xd8, 0xff, 0xd8], Failure::InvalidMarker),
            (vec![0xff, 0xd8, 0xff, 0xd9], Failure::MissingSof),
            (jpeg_header(0xc0, 17, 29), Failure::MissingEoi),
            (jpeg_header(0xc0, 0, 29), Failure::ZeroDimension),
            (jpeg_header(0xc0, 17, 0), Failure::ZeroDimension),
            (
                [vec![0xff, 0xd8], segment(0xc0, &[8, 0, 29])].concat(),
                Failure::Truncated,
            ),
            (
                [jpeg_header(0xc0, 17, 29), segment(0xda, &[])].concat(),
                Failure::MissingEoi,
            ),
            (
                [jpeg_header(0xc0, 17, 29), vec![0xff, 0xd9, 0]].concat(),
                Failure::TrailingBytes,
            ),
            (
                [
                    jpeg_header(0xc0, 17, 29),
                    segment(0xda, &[]),
                    vec![0xff, 0xd9, 0],
                ]
                .concat(),
                Failure::MissingEoi,
            ),
        ];
        for (bytes, reason) in cases {
            assert_eq!(check(&bytes, Container::Jpeg), Err(reason), "{reason:?}");
        }
    }
    #[test]
    fn png_rejects_duplicate_ihdr() {
        let header = png_header(17, 29);
        let bytes = [header.as_slice(), &header[8..], &chunk(b"IEND", &[])].concat();
        assert_eq!(check(&bytes, Container::Png), Err(Failure::DuplicateIhdr));
    }

    #[test]
    fn png_checks_header_fields() {
        for (index, value) in [(8, 4), (9, 1), (10, 1), (11, 1), (12, 2)] {
            let header = png_header(17, 29);
            let mut data = header[16..29].to_vec();
            data[index] = value;
            let bytes = [
                PNG_SIGNATURE.as_slice(),
                &chunk(b"IHDR", &data),
                &chunk(b"IEND", &[]),
            ]
            .concat();
            assert_eq!(check(&bytes, Container::Png), Err(Failure::InvalidHeader));
        }
    }

    #[test]
    fn jpeg_allows_eoi_bytes_inside_later_dht() {
        let bytes = [
            jpeg_header(0xc2, 17, 29),
            segment(0xda, &[1, 1, 0, 0, 63, 0]),
            segment(0xc4, &[0, 0xff, 0xd9, 0]),
            segment(0xda, &[1, 1, 0, 0, 63, 0]),
            vec![0xff, 0xd9],
        ]
        .concat();
        assert_eq!(
            check(&bytes, Container::Jpeg),
            Ok(Checked {
                width: 17,
                height: 29,
            })
        );
    }

    #[test]
    fn jpeg_rejects_invalid_sof_components() {
        for data in [
            &[8, 0, 29, 0, 17, 0][..],
            &[8, 0, 29, 0, 17, 1][..],
            &[8, 0, 29, 0, 17, 1, 1, 0x11, 0, 0][..],
        ] {
            let bytes = [vec![0xff, 0xd8], segment(0xc0, data), vec![0xff, 0xd9]].concat();
            assert_eq!(check(&bytes, Container::Jpeg), Err(Failure::InvalidLength));
        }
    }

    #[test]
    fn png_requires_idat_before_iend() {
        let bytes = [png_header(17, 29), chunk(b"IEND", &[])].concat();
        assert_eq!(check(&bytes, Container::Png), Err(Failure::MissingIdat));
    }
}
