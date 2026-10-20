//! Structural timing metadata only; sample payloads are not decoded or validated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    Truncated,
    InvalidBox,
    NestingLimit,
    MissingFtyp,
    MissingBox,
    DuplicateBox,
    UnsupportedVersion,
    UnsupportedHandler,
    EditList,
    MultipleVideoTracks,
    InvalidTiming,
    InvalidDimensions,
    SampleCountMismatch,
    InvalidFragment,
    Overflow,
}
impl Failure {
    pub(crate) fn as_str(self) -> &'static str {
        "unsupported_container"
    }
    pub(crate) fn detail(self) -> &'static str {
        match self {
            Self::Truncated => "truncated_box_or_table",
            Self::InvalidBox => "invalid_box_size_or_fields",
            Self::NestingLimit => "box_nesting_limit",
            Self::MissingFtyp => "ftyp_must_be_first",
            Self::MissingBox => "missing_required_box_or_table",
            Self::DuplicateBox => "duplicate_required_box_or_track_id",
            Self::UnsupportedVersion => "unsupported_box_version",
            Self::UnsupportedHandler => "unsupported_track_handler",
            Self::EditList => "unsupported_edit_list",
            Self::MultipleVideoTracks => "multiple_video_tracks",
            Self::InvalidTiming => "invalid_or_inconsistent_timing",
            Self::InvalidDimensions => "invalid_video_dimensions",
            Self::SampleCountMismatch => "sample_count_mismatch",
            Self::InvalidFragment => "invalid_fragment_or_missing_defaults",
            Self::Overflow => "metadata_overflow",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrackKind {
    Video,
    Audio,
}
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Rational {
    pub numerator: u64,
    pub denominator: u64,
}
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Track {
    pub kind: TrackKind,
    pub timescale: u32,
    pub duration_us: u64,
    pub media_duration_us: u64,
    pub sample_count: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<Rational>,
}
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Checked {
    pub tracks: Vec<Track>,
}
struct BoxRef<'a> {
    tag: &'a [u8],
    data: &'a [u8],
}
struct Boxes<'a>(&'a [u8]);
impl<'a> Iterator for Boxes<'a> {
    type Item = Result<BoxRef<'a>, Failure>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.0.is_empty() {
            return None;
        }
        let bytes = std::mem::take(&mut self.0);
        Some((|| {
            let size = word(bytes, 0)?;
            let tag = bytes.get(4..8).ok_or(Failure::Truncated)?;
            let (length, header) = match size {
                0 => (bytes.len(), 8),
                1 => (
                    usize::try_from(wide(bytes, 8)?).map_err(|_| Failure::Overflow)?,
                    16,
                ),
                _ => (size as usize, 8),
            };
            if length < header {
                return Err(Failure::InvalidBox);
            }
            let body = bytes.get(header..length).ok_or(Failure::Truncated)?;
            self.0 = &bytes[length..];
            Ok(BoxRef { tag, data: body })
        })())
    }
}
fn word(bytes: &[u8], offset: usize) -> Result<u32, Failure> {
    let value = bytes
        .get(offset..offset.checked_add(4).ok_or(Failure::Overflow)?)
        .ok_or(Failure::Truncated)?;
    Ok(u32::from_be_bytes(value.try_into().unwrap()))
}
fn wide(bytes: &[u8], offset: usize) -> Result<u64, Failure> {
    let value = bytes
        .get(offset..offset.checked_add(8).ok_or(Failure::Overflow)?)
        .ok_or(Failure::Truncated)?;
    Ok(u64::from_be_bytes(value.try_into().unwrap()))
}
fn add(left: u64, right: u64) -> Result<u64, Failure> {
    left.checked_add(right).ok_or(Failure::Overflow)
}
fn mul(left: u64, right: u64) -> Result<u64, Failure> {
    left.checked_mul(right).ok_or(Failure::Overflow)
}
fn find<'a>(bytes: &'a [u8], tag: &[u8]) -> Result<Option<&'a [u8]>, Failure> {
    let mut found = None;
    for item in Boxes(bytes) {
        let item = item?;
        if item.tag == tag && found.replace(item.data).is_some() {
            return Err(Failure::DuplicateBox);
        }
    }
    Ok(found)
}
fn required<'a>(bytes: &'a [u8], tag: &[u8]) -> Result<&'a [u8], Failure> {
    find(bytes, tag)?.ok_or(Failure::MissingBox)
}
fn validate(bytes: &[u8], depth: usize) -> Result<(), Failure> {
    if depth > 16 {
        return Err(Failure::NestingLimit);
    }
    for item in Boxes(bytes) {
        let item = item?;
        match item.tag {
            b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" | b"mvex" | b"moof" | b"traf"
            | b"edts" => validate(item.data, depth + 1)?,
            _ => (),
        }
    }
    Ok(())
}
fn version(bytes: &[u8], maximum: u8) -> Result<u8, Failure> {
    let version = (word(bytes, 0)? >> 24) as u8;
    if version > maximum {
        return Err(Failure::UnsupportedVersion);
    }
    Ok(version)
}
struct State {
    id: u32,
    track: Track,
    declared_duration: u64,
    duration: u64,
    presentation_duration: Option<u64>,
    default_duration: Option<u32>,
}
fn timing(bytes: &[u8]) -> Result<(u64, u64), Failure> {
    version(bytes, 0)?;
    let entries = word(bytes, 4)? as usize;
    table_length(bytes, 8, entries, 8)?;
    let (mut samples, mut duration) = (0, 0);
    for entry in bytes[8..].as_chunks::<8>().0 {
        let count = u64::from(word(entry, 0)?);
        let delta = u64::from(word(entry, 4)?);
        if count == 0 || delta == 0 {
            return Err(Failure::InvalidTiming);
        }
        samples = add(samples, count)?;
        duration = add(duration, mul(count, delta)?)?;
    }
    Ok((samples, duration))
}
fn table_length(bytes: &[u8], header: usize, count: usize, stride: usize) -> Result<(), Failure> {
    let expected = count
        .checked_mul(stride)
        .and_then(|n| n.checked_add(header))
        .ok_or(Failure::Overflow)?;
    if bytes.len() != expected {
        return Err(Failure::Truncated);
    }
    Ok(())
}
fn sample_count(stbl: &[u8]) -> Result<u64, Failure> {
    let normal = find(stbl, b"stsz")?;
    let compact = find(stbl, b"stz2")?;
    match (normal, compact) {
        (Some(bytes), None) => {
            version(bytes, 0)?;
            let count = word(bytes, 8)?;
            table_length(
                bytes,
                12,
                count as usize,
                if word(bytes, 4)? == 0 { 4 } else { 0 },
            )?;
            Ok(u64::from(count))
        }
        (None, Some(bytes)) => {
            version(bytes, 0)?;
            let bits = word(bytes, 4)?;
            if !matches!(bits, 4 | 8 | 16) {
                return Err(Failure::InvalidBox);
            }
            let count = word(bytes, 8)?;
            let length = (u64::from(count) * u64::from(bits)).div_ceil(8);
            table_length(
                bytes,
                12,
                usize::try_from(length).map_err(|_| Failure::Overflow)?,
                1,
            )?;
            Ok(u64::from(count))
        }
        (Some(_), Some(_)) => Err(Failure::DuplicateBox),
        _ => Err(Failure::MissingBox),
    }
}
fn parse_track(bytes: &[u8]) -> Result<State, Failure> {
    let presentation_duration = find(bytes, b"edts")?.map(edit_duration).transpose()?;
    let tkhd = required(bytes, b"tkhd")?;
    let v = version(tkhd, 1)?;
    let (id_offset, dimensions_offset, length) = if v == 0 { (12, 76, 84) } else { (20, 88, 96) };
    table_length(tkhd, length, 0, 0)?;
    let id = word(tkhd, id_offset)?;
    if id == 0 {
        return Err(Failure::InvalidBox);
    }
    let mdia = required(bytes, b"mdia")?;
    let hdlr = required(mdia, b"hdlr")?;
    version(hdlr, 0)?;
    let kind = match hdlr.get(8..12).ok_or(Failure::Truncated)? {
        b"vide" => TrackKind::Video,
        b"soun" => TrackKind::Audio,
        _ => return Err(Failure::UnsupportedHandler),
    };
    let (width, height) = if kind == TrackKind::Video {
        let width = word(tkhd, dimensions_offset)?;
        let height = word(tkhd, dimensions_offset + 4)?;
        if width == 0 || height == 0 || width & 0xffff != 0 || height & 0xffff != 0 {
            return Err(Failure::InvalidDimensions);
        }
        (Some(width >> 16), Some(height >> 16))
    } else {
        (None, None)
    };
    let mdhd = required(mdia, b"mdhd")?;
    let (timescale, declared_duration) = if version(mdhd, 1)? == 0 {
        table_length(mdhd, 24, 0, 0)?;
        (word(mdhd, 12)?, u64::from(word(mdhd, 16)?))
    } else {
        table_length(mdhd, 36, 0, 0)?;
        (word(mdhd, 20)?, wide(mdhd, 24)?)
    };
    if timescale == 0 {
        return Err(Failure::InvalidTiming);
    }
    let stbl = required(required(mdia, b"minf")?, b"stbl")?;
    sample_description(required(stbl, b"stsd")?, kind, width, height)?;
    let (samples, duration) = timing(required(stbl, b"stts")?)?;
    if samples != sample_count(stbl)? {
        return Err(Failure::SampleCountMismatch);
    }
    Ok(State {
        id,
        declared_duration,
        duration,
        presentation_duration,
        default_duration: None,
        track: Track {
            kind,
            timescale,
            duration_us: 0,
            media_duration_us: 0,
            sample_count: samples,
            width,
            height,
            fps: None,
        },
    })
}
fn defaults(bytes: &[u8], tracks: &mut [State]) -> Result<(), Failure> {
    for item in Boxes(bytes) {
        let item = item?;
        if item.tag != b"trex" {
            continue;
        }
        version(item.data, 0)?;
        table_length(item.data, 24, 0, 0)?;
        let id = word(item.data, 4)?;
        let track = tracks
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or(Failure::InvalidFragment)?;
        if track
            .default_duration
            .replace(word(item.data, 12)?)
            .is_some()
        {
            return Err(Failure::DuplicateBox);
        }
    }
    if tracks.iter().any(|t| t.default_duration.is_none()) {
        return Err(Failure::InvalidFragment);
    }
    Ok(())
}
fn fragment(bytes: &[u8], tracks: &mut [State]) -> Result<(), Failure> {
    let tfhd = required(bytes, b"tfhd")?;
    version(tfhd, 0)?;
    let flags = word(tfhd, 0)? & 0xffffff;
    if flags & !0x03003b != 0 || flags & 0x010000 != 0 {
        return Err(Failure::InvalidFragment);
    }
    let id = word(tfhd, 4)?;
    let track = tracks
        .iter_mut()
        .find(|t| t.id == id)
        .ok_or(Failure::InvalidFragment)?;
    let mut offset = 8;
    if flags & 1 != 0 {
        offset += 8;
    }
    if flags & 2 != 0 {
        offset += 4;
    }
    let mut default = track.default_duration.ok_or(Failure::InvalidFragment)?;
    if flags & 8 != 0 {
        default = word(tfhd, offset)?;
        offset += 4;
    }
    if flags & 0x10 != 0 {
        offset += 4;
    }
    if flags & 0x20 != 0 {
        offset += 4;
    }
    table_length(tfhd, offset, 0, 0)?;
    if let Some(tfdt) = find(bytes, b"tfdt")? {
        let base = if version(tfdt, 1)? == 0 {
            table_length(tfdt, 8, 0, 0)?;
            u64::from(word(tfdt, 4)?)
        } else {
            table_length(tfdt, 12, 0, 0)?;
            wide(tfdt, 4)?
        };
        if base != track.duration {
            return Err(Failure::InvalidTiming);
        }
    }
    let mut runs = false;
    for item in Boxes(bytes) {
        let item = item?;
        if item.tag == b"trun" {
            runs = true;
            let (count, duration) = run(item.data, default)?;
            track.track.sample_count = add(track.track.sample_count, count)?;
            track.duration = add(track.duration, duration)?;
        }
    }
    if !runs {
        return Err(Failure::MissingBox);
    }
    Ok(())
}
fn run(bytes: &[u8], default: u32) -> Result<(u64, u64), Failure> {
    version(bytes, 1)?;
    let flags = word(bytes, 0)? & 0xffffff;
    if flags & !0x000f05 != 0 || flags & 0x404 == 0x404 {
        return Err(Failure::InvalidFragment);
    }
    let count = word(bytes, 4)?;
    let header = 8 + if flags & 1 != 0 { 4 } else { 0 } + if flags & 4 != 0 { 4 } else { 0 };
    let stride = ((flags & 0xf00).count_ones() * 4) as usize;
    table_length(bytes, header, count as usize, stride)?;
    if count == 0 {
        return Ok((0, 0));
    }
    if flags & 0x100 == 0 {
        if default == 0 {
            return Err(Failure::InvalidFragment);
        }
        return Ok((u64::from(count), mul(u64::from(count), u64::from(default))?));
    }
    let mut duration = 0;
    for entry in bytes[header..].chunks_exact(stride) {
        let delta = word(entry, 0)?;
        if delta == 0 {
            return Err(Failure::InvalidTiming);
        }
        duration = add(duration, u64::from(delta))?;
    }
    Ok((u64::from(count), duration))
}
fn sample_description(
    bytes: &[u8],
    kind: TrackKind,
    width: Option<u32>,
    height: Option<u32>,
) -> Result<(), Failure> {
    version(bytes, 0)?;
    if word(bytes, 4)? != 1 {
        return Err(Failure::InvalidBox);
    }
    let mut entries = Boxes(&bytes[8..]);
    let entry = entries.next().ok_or(Failure::MissingBox)??;
    if entries.next().is_some() {
        return Err(Failure::InvalidBox);
    }
    if matches!(entry.tag, b"encv" | b"enca") {
        return Err(Failure::InvalidBox);
    }
    let data = entry.data;
    let reference = data.get(6..8).ok_or(Failure::Truncated)?;
    if reference == [0, 0] {
        return Err(Failure::InvalidBox);
    }
    let header = match kind {
        TrackKind::Video => {
            let dimensions = word(data, 24)?;
            if Some(dimensions >> 16) != width || Some(dimensions & 0xffff) != height {
                return Err(Failure::InvalidDimensions);
            }
            78
        }
        TrackKind::Audio => {
            if data.get(8..10).ok_or(Failure::Truncated)? != [0, 0] {
                return Err(Failure::UnsupportedVersion);
            }
            if word(data, 16)? >> 16 == 0 || word(data, 24)? == 0 {
                return Err(Failure::InvalidBox);
            }
            28
        }
    };
    validate(data.get(header..).ok_or(Failure::Truncated)?, 0)
}
fn edit_duration(bytes: &[u8]) -> Result<u64, Failure> {
    let elst = find(bytes, b"elst")?.ok_or(Failure::EditList)?;
    let v = version(elst, 1)?;
    if word(elst, 4)? != 1 {
        return Err(Failure::EditList);
    }
    let (duration, media_time, rate_offset) = if v == 0 {
        table_length(elst, 20, 0, 0)?;
        (
            u64::from(word(elst, 8)?),
            i64::from(word(elst, 12)? as i32),
            16,
        )
    } else {
        table_length(elst, 28, 0, 0)?;
        (wide(elst, 8)?, wide(elst, 16)? as i64, 24)
    };
    if media_time < 0 || word(elst, rate_offset)? != 0x00010000 {
        return Err(Failure::EditList);
    }
    Ok(duration)
}
fn microseconds(duration: u64, timescale: u32) -> Result<u64, Failure> {
    u64::try_from(u128::from(duration) * 1_000_000 / u128::from(timescale))
        .map_err(|_| Failure::Overflow)
}
fn movie_header(bytes: &[u8]) -> Result<u32, Failure> {
    let (length, offset) = if version(bytes, 1)? == 0 {
        (100, 12)
    } else {
        (112, 20)
    };
    table_length(bytes, length, 0, 0)?;
    if word(bytes, offset)? == 0 {
        return Err(Failure::InvalidTiming);
    }
    word(bytes, offset)
}
pub(crate) fn check(bytes: &[u8]) -> Result<Checked, Failure> {
    let first = Boxes(bytes).next().ok_or(Failure::MissingFtyp)??;
    if first.tag != b"ftyp" {
        return Err(Failure::MissingFtyp);
    }
    if first.data.len() < 8 || first.data.len() % 4 != 0 {
        return Err(Failure::InvalidBox);
    }
    required(bytes, b"ftyp")?;
    validate(bytes, 0)?;
    let moov = required(bytes, b"moov")?;
    let movie_timescale = movie_header(required(moov, b"mvhd")?)?;
    let mut media_data = false;
    for item in Boxes(bytes) {
        let item = item?;
        media_data |= item.tag == b"mdat" && !item.data.is_empty();
    }
    if !media_data {
        return Err(Failure::MissingBox);
    }
    let mut tracks = Vec::new();
    for item in Boxes(moov) {
        let item = item?;
        if item.tag != b"trak" {
            continue;
        }
        let track = parse_track(item.data)?;
        if tracks.iter().any(|t: &State| t.id == track.id) {
            return Err(Failure::DuplicateBox);
        }
        if track.track.kind == TrackKind::Video
            && tracks.iter().any(|t| t.track.kind == TrackKind::Video)
        {
            return Err(Failure::MultipleVideoTracks);
        }
        tracks.push(track);
    }
    if tracks.is_empty() {
        return Err(Failure::MissingBox);
    }
    let mvex = find(moov, b"mvex")?;
    if let Some(mvex) = mvex {
        defaults(mvex, &mut tracks)?;
    }
    let mut fragments = false;
    for item in Boxes(bytes) {
        let item = item?;
        if item.tag != b"moof" {
            continue;
        }
        if mvex.is_none() {
            return Err(Failure::InvalidFragment);
        }
        let mut trafs = false;
        for child in Boxes(item.data) {
            let child = child?;
            if child.tag == b"traf" {
                trafs = true;
                fragment(child.data, &mut tracks)?;
            }
        }
        if !trafs {
            return Err(Failure::MissingBox);
        }
        fragments = true;
    }
    if mvex.is_some() && !fragments {
        return Err(Failure::InvalidFragment);
    }
    let mut checked = Vec::with_capacity(tracks.len());
    for mut state in tracks {
        if state.duration == 0 || state.track.sample_count == 0 {
            return Err(Failure::InvalidTiming);
        }
        if state.declared_duration != state.duration && !(fragments && state.declared_duration == 0)
        {
            return Err(Failure::InvalidTiming);
        }
        state.track.media_duration_us = microseconds(state.duration, state.track.timescale)?;
        state.track.duration_us = match state.presentation_duration {
            Some(duration) => microseconds(duration, movie_timescale)?,
            None => state.track.media_duration_us,
        };
        if state.track.kind == TrackKind::Video {
            let numerator = mul(state.track.sample_count, u64::from(state.track.timescale))?;
            let divisor = gcd(numerator, state.duration);
            state.track.fps = Some(Rational {
                numerator: numerator / divisor,
                denominator: state.duration / divisor,
            });
        }
        checked.push(state.track);
    }
    Ok(Checked { tracks: checked })
}
fn gcd(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left
}

#[cfg(test)]
mod tests {
    use super::*;
    fn words(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_be_bytes()).collect()
    }
    fn bx(tag: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut bytes = words(&[u32::try_from(data.len() + 8).unwrap()]);
        bytes.extend(tag);
        bytes.extend(data);
        bytes
    }
    fn track(id: u32, audio: bool, entries: &[u32], count: u32, duration: u32) -> Vec<u8> {
        let mut tkhd = vec![0; 84];
        tkhd[12..16].copy_from_slice(&id.to_be_bytes());
        tkhd[76..80].copy_from_slice(&(640u32 << 16).to_be_bytes());
        tkhd[80..84].copy_from_slice(&(480u32 << 16).to_be_bytes());
        let mut handler = vec![0; 24];
        handler[8..12].copy_from_slice(if audio { b"soun" } else { b"vide" });
        let mut stts = words(&[0, (entries.len() / 2) as u32]);
        stts.extend(words(entries));
        let mut entry = vec![0; if audio { 28 } else { 78 }];
        entry[7] = 1;
        if audio {
            entry[16..18].copy_from_slice(&2u16.to_be_bytes());
            entry[18..20].copy_from_slice(&16u16.to_be_bytes());
            entry[24..28].copy_from_slice(&(48000u32 << 16).to_be_bytes());
        } else {
            entry[24..26].copy_from_slice(&640u16.to_be_bytes());
            entry[26..28].copy_from_slice(&480u16.to_be_bytes());
        }
        let stsd = bx(
            b"stsd",
            &[
                words(&[0, 1]),
                bx(if audio { b"mp4a" } else { b"avc1" }, &entry),
            ]
            .concat(),
        );
        let stbl = [
            stsd,
            bx(b"stts", &stts),
            bx(b"stsz", &words(&[0, 1, count])),
        ]
        .concat();
        let mdia = [
            bx(b"mdhd", &words(&[0, 0, 0, 1000, duration, 0])),
            bx(b"hdlr", &handler),
            bx(b"minf", &bx(b"stbl", &stbl)),
        ]
        .concat();
        bx(b"trak", &[bx(b"tkhd", &tkhd), bx(b"mdia", &mdia)].concat())
    }
    fn file(moov: &[u8], suffix: &[u8]) -> Vec<u8> {
        let mut mvhd = vec![0; 100];
        mvhd[12..16].copy_from_slice(&1000u32.to_be_bytes());
        [
            bx(b"ftyp", b"isom\0\0\0\0isom"),
            bx(b"moov", &[bx(b"mvhd", &mvhd), moov.to_vec()].concat()),
            suffix.to_vec(),
            bx(b"mdat", &[1]),
        ]
        .concat()
    }
    fn progressive() -> Vec<u8> {
        file(&track(1, false, &[30, 40], 30, 1200), &[])
    }
    #[test]
    fn progressive_video_and_audio() {
        let result = check(&file(
            &[
                track(1, false, &[30, 40], 30, 1200),
                track(2, true, &[60, 20], 60, 1200),
            ]
            .concat(),
            &[],
        ))
        .unwrap();
        assert_eq!(result.tracks.len(), 2);
        let video = &result.tracks[0];
        assert_eq!(
            (
                video.width,
                video.height,
                video.sample_count,
                video.duration_us
            ),
            (Some(640), Some(480), 30, 1_200_000)
        );
        assert_eq!(
            video.fps,
            Some(Rational {
                numerator: 25,
                denominator: 1
            })
        );
        assert_eq!(result.tracks[1].kind, TrackKind::Audio);
        assert_eq!(result.tracks[1].fps, None);
    }
    #[test]
    fn fragmented_defaults_and_explicit_durations() {
        let moov = [
            track(1, false, &[], 0, 0),
            bx(b"mvex", &bx(b"trex", &words(&[0, 1, 1, 40, 1, 0]))),
        ]
        .concat();
        let traf = [
            bx(b"tfhd", &words(&[0, 1])),
            bx(b"trun", &words(&[0, 2])),
            bx(b"trun", &words(&[0x100, 2, 40, 80])),
        ]
        .concat();
        let result = check(&file(&moov, &bx(b"moof", &bx(b"traf", &traf)))).unwrap();
        assert_eq!(result.tracks[0].sample_count, 4);
        assert_eq!(result.tracks[0].duration_us, 200_000);
        assert_eq!(
            result.tracks[0].fps,
            Some(Rational {
                numerator: 20,
                denominator: 1
            })
        );
        let traf = [
            bx(b"tfhd", &words(&[8, 1, 20])),
            bx(b"trun", &words(&[0, 3])),
        ]
        .concat();
        assert_eq!(
            check(&file(&moov, &bx(b"moof", &bx(b"traf", &traf))))
                .unwrap()
                .tracks[0]
                .duration_us,
            60_000
        );
    }
    #[test]
    fn extended_and_eof_boxes() {
        let mut bytes = progressive();
        bytes.extend(words(&[1]));
        bytes.extend(b"free");
        bytes.extend(16u64.to_be_bytes());
        bytes.extend(words(&[0]));
        bytes.extend(b"mdat");
        bytes.extend([0; 8]);
        assert!(check(&bytes).is_ok());
        assert_eq!(
            check(&file(
                &track(1, false, &[1, 1], 1, 1),
                &bx(b"free", &[])[..7]
            )),
            Err(Failure::Truncated)
        );
    }
    #[test]
    fn table_failures_and_overflow() {
        assert_eq!(
            check(&file(&track(1, false, &[2, 40], 1, 80), &[])),
            Err(Failure::SampleCountMismatch)
        );
        assert_eq!(
            check(&file(&track(1, false, &[2, 40], 2, 81), &[])),
            Err(Failure::InvalidTiming)
        );
        assert_eq!(
            check(&file(
                &track(1, false, &[u32::MAX, u32::MAX, u32::MAX, u32::MAX], 1, 1),
                &[]
            )),
            Err(Failure::Overflow)
        );
        assert_eq!(mul(u64::MAX, 2), Err(Failure::Overflow));
        assert_eq!(add(u64::MAX, 1), Err(Failure::Overflow));
        assert_eq!(table_length(&[], 8, usize::MAX, 8), Err(Failure::Overflow));
        assert_eq!(
            sample_count(&bx(b"stz2", &words(&[0, 3, 0]))),
            Err(Failure::InvalidBox)
        );
        assert_eq!(sample_count(&[]), Err(Failure::MissingBox));
        assert_eq!(timing(&words(&[0, 1, 1, 0])), Err(Failure::InvalidTiming));
    }
    #[test]
    fn container_failures() {
        assert_eq!(check(&[]), Err(Failure::MissingFtyp));
        assert_eq!(check(&bx(b"free", &[])), Err(Failure::MissingFtyp));
        assert_eq!(check(&file(&[], &[])), Err(Failure::MissingBox));
        assert_eq!(
            check(&file(&track(1, false, &[1, 1], 1, 1), &bx(b"moov", &[]))),
            Err(Failure::DuplicateBox)
        );
        assert_eq!(
            check(&file(
                &[
                    track(1, false, &[1, 1], 1, 1),
                    track(2, false, &[1, 1], 1, 1)
                ]
                .concat(),
                &[]
            )),
            Err(Failure::MultipleVideoTracks)
        );
        assert_eq!(
            check(&file(&track(1, false, &[1, 1], 1, 1), &bx(b"moof", &[]))),
            Err(Failure::InvalidFragment)
        );
        let mut nested = Vec::new();
        for _ in 0..18 {
            nested = bx(b"moov", &nested);
        }
        assert_eq!(
            check(&[bx(b"ftyp", b"isom\0\0\0\0"), nested].concat()),
            Err(Failure::NestingLimit)
        );
        assert_eq!(
            version(&words(&[0x02000000]), 1),
            Err(Failure::UnsupportedVersion)
        );
        assert_eq!(
            Boxes(&words(&[4, 0])).next().unwrap().err(),
            Some(Failure::InvalidBox)
        );
        for failure in [
            Failure::Truncated,
            Failure::InvalidBox,
            Failure::NestingLimit,
            Failure::MissingFtyp,
            Failure::MissingBox,
            Failure::DuplicateBox,
            Failure::UnsupportedVersion,
            Failure::UnsupportedHandler,
            Failure::EditList,
            Failure::MultipleVideoTracks,
            Failure::InvalidTiming,
            Failure::InvalidDimensions,
            Failure::SampleCountMismatch,
            Failure::InvalidFragment,
            Failure::Overflow,
        ] {
            assert_eq!(failure.as_str(), "unsupported_container");
            assert!(!failure.detail().is_empty());
        }
    }
    #[test]
    fn track_header_failures_and_compact_sizes() {
        let valid = track(1, false, &[1, 1], 1, 1);
        let body = &valid[8..];
        assert!(matches!(
            parse_track(&[body, &bx(b"edts", &[])].concat()),
            Err(Failure::EditList)
        ));
        let tkhd = required(body, b"tkhd").unwrap();
        let mdia = required(body, b"mdia").unwrap();
        let mut zero = tkhd.to_vec();
        zero[76..80].fill(0);
        assert!(matches!(
            parse_track(&[bx(b"tkhd", &zero), bx(b"mdia", mdia)].concat()),
            Err(Failure::InvalidDimensions)
        ));
        let mut handler = required(mdia, b"hdlr").unwrap().to_vec();
        handler[8..12].copy_from_slice(b"text");
        let bad_mdia = [
            bx(b"hdlr", &handler),
            bx(b"mdhd", required(mdia, b"mdhd").unwrap()),
            bx(b"minf", required(mdia, b"minf").unwrap()),
        ]
        .concat();
        assert!(matches!(
            parse_track(&[bx(b"tkhd", tkhd), bx(b"mdia", &bad_mdia)].concat()),
            Err(Failure::UnsupportedHandler)
        ));
        for (bits, data) in [
            (4, vec![0x12, 0x30]),
            (8, vec![1, 2, 3]),
            (16, vec![0, 1, 0, 2, 0, 3]),
        ] {
            assert_eq!(
                sample_count(&bx(b"stz2", &[words(&[0, bits, 3]), data].concat())),
                Ok(3)
            );
        }
    }
    fn version_one_track(count: u32, delta: u32, timescale: u32) -> Vec<u8> {
        let original = track(1, false, &[count, delta], count, 0);
        let body = &original[8..];
        let old_tkhd = required(body, b"tkhd").unwrap();
        let mut tkhd = vec![0; 96];
        tkhd[0] = 1;
        tkhd[20..24].copy_from_slice(&1u32.to_be_bytes());
        tkhd[88..96].copy_from_slice(&old_tkhd[76..84]);
        let mdia = required(body, b"mdia").unwrap();
        let mut mdhd = vec![0; 36];
        mdhd[0] = 1;
        mdhd[20..24].copy_from_slice(&timescale.to_be_bytes());
        mdhd[24..32].copy_from_slice(&(u64::from(count) * u64::from(delta)).to_be_bytes());
        let mdia = [
            bx(b"mdhd", &mdhd),
            bx(b"hdlr", required(mdia, b"hdlr").unwrap()),
            bx(b"minf", required(mdia, b"minf").unwrap()),
        ]
        .concat();
        bx(b"trak", &[bx(b"tkhd", &tkhd), bx(b"mdia", &mdia)].concat())
    }
    #[test]
    fn version_one_and_conversion_overflow() {
        let checked = check(&file(&version_one_track(30, 1001, 30000), &[])).unwrap();
        assert_eq!(
            checked.tracks[0].fps,
            Some(Rational {
                numerator: 30000,
                denominator: 1001
            })
        );
        assert_eq!(checked.tracks[0].duration_us, 1_001_000);
        assert_eq!(
            check(&file(&version_one_track(u32::MAX, u32::MAX, 1), &[])),
            Err(Failure::Overflow)
        );
        assert_eq!(
            check(&file(&version_one_track(1, 1, 0), &[])),
            Err(Failure::InvalidTiming)
        );
    }
    #[test]
    fn truncated_prefixes_fail_closed() {
        let bytes = progressive();
        for end in 0..bytes.len() {
            assert!(check(&bytes[..end]).is_err(), "prefix {end}");
        }
    }
    #[test]
    fn fragment_failures() {
        assert_eq!(run(&words(&[0, 1]), 0), Err(Failure::InvalidFragment));
        assert_eq!(run(&words(&[0x100, 1, 0]), 1), Err(Failure::InvalidTiming));
        assert_eq!(
            run(&words(&[0x404, 1, 0, 0]), 1),
            Err(Failure::InvalidFragment)
        );
        assert_eq!(run(&words(&[0x100, 2, 1]), 1), Err(Failure::Truncated));
        let original = track(1, false, &[], 0, 0);
        let mut tracks = [parse_track(&original[8..]).unwrap()];
        assert_eq!(defaults(&[], &mut tracks), Err(Failure::InvalidFragment));
        defaults(&bx(b"trex", &words(&[0, 1, 1, 40, 1, 0])), &mut tracks).unwrap();
        let traf = [
            bx(b"tfhd", &words(&[0, 1])),
            bx(b"tfdt", &words(&[0, 1])),
            bx(b"trun", &words(&[0, 1])),
        ]
        .concat();
        assert_eq!(fragment(&traf, &mut tracks), Err(Failure::InvalidTiming));
        assert_eq!(
            fragment(&bx(b"tfhd", &words(&[0, 2])), &mut tracks),
            Err(Failure::InvalidFragment)
        );
    }
    #[test]
    fn required_movie_sample_description_and_media_data() {
        let valid = progressive();
        let moov = required(&valid, b"moov").unwrap();
        let without_data = [bx(b"ftyp", b"isom\0\0\0\0"), bx(b"moov", moov)].concat();
        assert_eq!(check(&without_data), Err(Failure::MissingBox));
        let track = required(moov, b"trak").unwrap();
        let without_header = [
            bx(b"ftyp", b"isom\0\0\0\0"),
            bx(b"moov", &bx(b"trak", track)),
            bx(b"mdat", &[1]),
        ]
        .concat();
        assert_eq!(check(&without_header), Err(Failure::MissingBox));
        assert_eq!(movie_header(&[0; 100]), Err(Failure::InvalidTiming));
        assert_eq!(movie_header(&[0; 99]), Err(Failure::Truncated));
        let mdia = required(track, b"mdia").unwrap();
        let stbl = required(required(mdia, b"minf").unwrap(), b"stbl").unwrap();
        let stsd = required(stbl, b"stsd").unwrap();
        assert_eq!(
            sample_description(stsd, TrackKind::Video, Some(16), Some(16)),
            Err(Failure::InvalidDimensions)
        );
        assert_eq!(
            sample_description(&words(&[0, 0]), TrackKind::Video, Some(640), Some(480)),
            Err(Failure::InvalidBox)
        );
        let mut mvhd = vec![0; 112];
        mvhd[0] = 1;
        mvhd[20..24].copy_from_slice(&1000u32.to_be_bytes());
        assert_eq!(movie_header(&mvhd), Ok(1000));
    }
    fn edited_track(id: u32, audio: bool, duration: u32, media_time: u32) -> Vec<u8> {
        let original = track(id, audio, &[30, 40], 30, 1200);
        bx(
            b"trak",
            &[
                original[8..].to_vec(),
                bx(
                    b"edts",
                    &bx(b"elst", &words(&[0, 1, duration, media_time, 0x00010000])),
                ),
            ]
            .concat(),
        )
    }
    #[test]
    fn identity_and_encoder_delay_edits() {
        let identity = check(&file(&edited_track(1, false, 1200, 0), &[])).unwrap();
        assert_eq!(identity.tracks[0].duration_us, 1_200_000);
        assert_eq!(identity.tracks[0].media_duration_us, 1_200_000);
        let delayed = check(&file(
            &[
                edited_track(1, false, 1000, 200),
                edited_track(2, true, 1100, 100),
            ]
            .concat(),
            &[],
        ))
        .unwrap();
        assert_eq!(delayed.tracks[0].duration_us, 1_000_000);
        assert_eq!(delayed.tracks[1].duration_us, 1_100_000);
        for track in &delayed.tracks {
            assert_eq!(track.media_duration_us, 1_200_000);
            assert_eq!(track.sample_count, 30);
        }
        assert_eq!(
            delayed.tracks[0].fps,
            Some(Rational {
                numerator: 25,
                denominator: 1
            })
        );
        let mut mvhd = vec![0; 100];
        mvhd[12..16].copy_from_slice(&2000u32.to_be_bytes());
        let movie = [
            bx(b"ftyp", b"isom\0\0\0\0"),
            bx(
                b"moov",
                &[bx(b"mvhd", &mvhd), edited_track(1, false, 2000, 200)].concat(),
            ),
            bx(b"mdat", &[1]),
        ]
        .concat();
        let different_timescales = check(&movie).unwrap();
        assert_eq!(different_timescales.tracks[0].duration_us, 1_000_000);
        assert_eq!(different_timescales.tracks[0].media_duration_us, 1_200_000);
    }
    #[test]
    fn edit_versions_rates_and_overflow() {
        let version_one = [
            words(&[0x01000000, 1]),
            1000u64.to_be_bytes().to_vec(),
            200u64.to_be_bytes().to_vec(),
            words(&[0x00010000]),
        ]
        .concat();
        assert_eq!(edit_duration(&bx(b"elst", &version_one)), Ok(1000));
        let two_entries = bx(
            b"elst",
            &words(&[0, 2, 600, 0, 0x00010000, 600, 600, 0x00010000]),
        );
        assert_eq!(edit_duration(&two_entries), Err(Failure::EditList));
        let original = track(1, false, &[30, 40], 30, 1200);
        let edited = bx(
            b"trak",
            &[original[8..].to_vec(), bx(b"edts", &two_entries)].concat(),
        );
        assert_eq!(check(&file(&edited, &[])), Err(Failure::EditList));
        for (media_time, rate) in [(u32::MAX, 0x00010000), (0, 0x00020000), (0, 0x00010001)] {
            assert_eq!(
                edit_duration(&bx(b"elst", &words(&[0, 1, 1000, media_time, rate]))),
                Err(Failure::EditList)
            );
        }
        let negative_v1 = [
            words(&[0x01000000, 1]),
            1000u64.to_be_bytes().to_vec(),
            u64::MAX.to_be_bytes().to_vec(),
            words(&[0x00010000]),
        ]
        .concat();
        assert_eq!(
            edit_duration(&bx(b"elst", &negative_v1)),
            Err(Failure::EditList)
        );
        assert_eq!(microseconds(u64::MAX, 1), Err(Failure::Overflow));
        assert_eq!(microseconds(90000, 90000), Ok(1_000_000));
    }
}
