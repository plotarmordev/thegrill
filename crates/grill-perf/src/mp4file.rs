//! Checks sample timing and physical media ranges without decoding sample payloads.
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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
    EditDuration,
    MultipleVideoTracks,
    InvalidTiming,
    InvalidDimensions,
    SampleCountMismatch,
    InvalidSampleTable,
    SampleDataOutsideMdat,
    InsufficientSampleData,
    InvalidFragment,
    Overflow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrackKind {
    Video,
    Audio,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Rational {
    pub numerator: u64,
    pub denominator: u64,
}

impl Rational {
    fn new(numerator: u64, denominator: u64) -> Self {
        let (mut left, mut right) = (numerator, denominator);
        while right != 0 {
            (left, right) = (right, left % right);
        }
        Self {
            numerator: numerator / left,
            denominator: denominator / left,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Track {
    pub kind: TrackKind,
    pub duration_us: u64,
    pub media_duration_us: u64,
    pub mdhd_duration_us: u64,
    pub sample_count: u64,
    pub sample_bytes: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub display_width: Option<Rational>,
    pub display_height: Option<Rational>,
    pub fps: Option<Rational>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Checked {
    pub tracks: Vec<Track>,
}

struct BoxRef<'a> {
    tag: &'a [u8],
    data: &'a [u8],
    start: u64,
    payload_start: u64,
    end: u64,
}

struct Boxes<'a> {
    rest: &'a [u8],
    offset: u64,
}

impl<'a> Boxes<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            rest: bytes,
            offset: 0,
        }
    }
}

impl<'a> Iterator for Boxes<'a> {
    type Item = Result<BoxRef<'a>, Failure>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.rest.is_empty() {
            return None;
        }
        let bytes = std::mem::take(&mut self.rest);
        let result = parse_box(bytes, self.offset);
        if let Ok(item) = &result {
            self.rest = &bytes[(item.end - self.offset) as usize..];
            self.offset = item.end;
        }
        Some(result)
    }
}

fn parse_box(bytes: &[u8], start: u64) -> Result<BoxRef<'_>, Failure> {
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
    let data = bytes.get(header..length).ok_or(Failure::Truncated)?;
    Ok(BoxRef {
        tag,
        data,
        start,
        payload_start: add(start, header as u64)?,
        end: add(start, length as u64)?,
    })
}

fn word(bytes: &[u8], offset: usize) -> Result<u32, Failure> {
    let data = bytes
        .get(offset..offset.checked_add(4).ok_or(Failure::Overflow)?)
        .ok_or(Failure::Truncated)?;
    Ok(u32::from_be_bytes(data.try_into().unwrap()))
}

fn wide(bytes: &[u8], offset: usize) -> Result<u64, Failure> {
    let data = bytes
        .get(offset..offset.checked_add(8).ok_or(Failure::Overflow)?)
        .ok_or(Failure::Truncated)?;
    Ok(u64::from_be_bytes(data.try_into().unwrap()))
}

fn add(left: u64, right: u64) -> Result<u64, Failure> {
    left.checked_add(right).ok_or(Failure::Overflow)
}

fn mul(left: u64, right: u64) -> Result<u64, Failure> {
    left.checked_mul(right).ok_or(Failure::Overflow)
}

fn find<'a>(bytes: &'a [u8], tag: &[u8]) -> Result<Option<&'a [u8]>, Failure> {
    let mut found = None;
    for item in Boxes::new(bytes) {
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

fn validate_nesting(bytes: &[u8], depth: usize) -> Result<(), Failure> {
    if depth > 16 {
        return Err(Failure::NestingLimit);
    }
    for item in Boxes::new(bytes) {
        let item = item?;
        match item.tag {
            b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" | b"mvex" | b"moof" | b"traf"
            | b"edts" => validate_nesting(item.data, depth + 1)?,
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

fn exact_length(bytes: &[u8], expected: usize) -> Result<(), Failure> {
    match bytes.len().cmp(&expected) {
        std::cmp::Ordering::Less => Err(Failure::Truncated),
        std::cmp::Ordering::Greater => Err(Failure::InvalidBox),
        std::cmp::Ordering::Equal => Ok(()),
    }
}

fn table_length(bytes: &[u8], header: usize, count: usize, stride: usize) -> Result<(), Failure> {
    let expected = count
        .checked_mul(stride)
        .and_then(|n| n.checked_add(header))
        .ok_or(Failure::Overflow)?;
    exact_length(bytes, expected)
}

struct MediaData {
    ranges: Vec<Range<u64>>,
    total: u64,
}

impl MediaData {
    fn parse(bytes: &[u8]) -> Result<Self, Failure> {
        let mut ranges = Vec::new();
        let mut total = 0;
        for item in Boxes::new(bytes) {
            let item = item?;
            if item.tag == b"mdat" {
                total = add(total, item.data.len() as u64)?;
                ranges.push(item.payload_start..item.end);
            }
        }
        if ranges.is_empty() {
            return Err(Failure::MissingBox);
        }
        Ok(Self { ranges, total })
    }

    fn contains(&self, start: u64, length: u64) -> Result<(), Failure> {
        let end = add(start, length)?;
        if !self
            .ranges
            .iter()
            .any(|range| start >= range.start && end <= range.end)
        {
            return Err(Failure::SampleDataOutsideMdat);
        }
        Ok(())
    }
}

struct SampleSizes<'a> {
    count: u32,
    fixed: u32,
    bits: u32,
    entries: &'a [u8],
}

impl<'a> SampleSizes<'a> {
    fn parse(stbl: &'a [u8]) -> Result<Self, Failure> {
        let (bytes, compact) = match (find(stbl, b"stsz")?, find(stbl, b"stz2")?) {
            (Some(bytes), None) => (bytes, false),
            (None, Some(bytes)) => (bytes, true),
            (Some(_), Some(_)) => return Err(Failure::DuplicateBox),
            _ => return Err(Failure::MissingBox),
        };
        version(bytes, 0)?;
        let count = word(bytes, 8)?;
        let field = word(bytes, 4)?;
        let (fixed, bits) = if compact {
            if !matches!(field, 4 | 8 | 16) {
                return Err(Failure::InvalidSampleTable);
            }
            (0, field)
        } else {
            (field, 32)
        };
        let table_bytes = if fixed == 0 {
            (u64::from(count) * u64::from(bits)).div_ceil(8)
        } else {
            0
        };
        table_length(
            bytes,
            12,
            usize::try_from(table_bytes).map_err(|_| Failure::Overflow)?,
            1,
        )?;
        Ok(Self {
            count,
            fixed,
            bits,
            entries: &bytes[12..],
        })
    }

    fn sum(&self, first: u64, count: u64) -> Result<u64, Failure> {
        let end = add(first, count)?;
        if end > u64::from(self.count) {
            return Err(Failure::SampleCountMismatch);
        }
        if self.fixed != 0 {
            return mul(count, u64::from(self.fixed));
        }
        let mut total = 0;
        for index in first as usize..end as usize {
            let size = match self.bits {
                4 => {
                    u32::from((self.entries[index / 2] >> if index % 2 == 0 { 4 } else { 0 }) & 15)
                }
                8 => u32::from(self.entries[index]),
                16 => u32::from(u16::from_be_bytes(
                    self.entries[index * 2..index * 2 + 2].try_into().unwrap(),
                )),
                _ => word(self.entries, index * 4)?,
            };
            total = add(total, u64::from(size))?;
        }
        Ok(total)
    }
}

fn progressive_ranges(
    stbl: &[u8],
    sizes: &SampleSizes<'_>,
    media: &MediaData,
) -> Result<u64, Failure> {
    let (offsets, stride) = match (find(stbl, b"stco")?, find(stbl, b"co64")?) {
        (Some(bytes), None) => (bytes, 4),
        (None, Some(bytes)) => (bytes, 8),
        (Some(_), Some(_)) => return Err(Failure::DuplicateBox),
        _ => return Err(Failure::MissingBox),
    };
    version(offsets, 0)?;
    let chunks = word(offsets, 4)?;
    table_length(offsets, 8, chunks as usize, stride)?;
    let stsc = required(stbl, b"stsc")?;
    version(stsc, 0)?;
    let mappings = word(stsc, 4)?;
    table_length(stsc, 8, mappings as usize, 12)?;
    let mut previous = 0;
    for entry in stsc[8..].as_chunks::<12>().0 {
        let first = word(entry, 0)?;
        if first <= previous || first > chunks || word(entry, 4)? == 0 || word(entry, 8)? != 1 {
            return Err(Failure::InvalidSampleTable);
        }
        previous = first;
    }
    if chunks == 0 {
        return if sizes.count == 0 && mappings == 0 {
            Ok(0)
        } else {
            Err(Failure::SampleCountMismatch)
        };
    }
    if mappings == 0 || word(stsc, 8)? != 1 {
        return Err(Failure::InvalidSampleTable);
    }
    let (mut mapping, mut sample, mut total) = (0, 0, 0);
    for chunk in 0..chunks {
        if mapping + 1 < mappings && word(stsc, 8 + (mapping as usize + 1) * 12)? == chunk + 1 {
            mapping += 1;
        }
        let count = u64::from(word(stsc, 12 + mapping as usize * 12)?);
        let length = sizes.sum(sample, count)?;
        let index = 8 + chunk as usize * stride;
        let offset = if stride == 4 {
            u64::from(word(offsets, index)?)
        } else {
            wide(offsets, index)?
        };
        media.contains(offset, length)?;
        sample = add(sample, count)?;
        total = add(total, length)?;
    }
    if sample != u64::from(sizes.count) {
        return Err(Failure::SampleCountMismatch);
    }
    Ok(total)
}

fn timing(bytes: &[u8]) -> Result<(u64, u64), Failure> {
    version(bytes, 0)?;
    table_length(bytes, 8, word(bytes, 4)? as usize, 8)?;
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

struct VideoDimensions {
    width: u32,
    height: u32,
    display_width: Rational,
    display_height: Rational,
}

struct TrackMetadata {
    id: u32,
    kind: TrackKind,
    timescale: u32,
    declared_duration: u64,
    edit_duration: Option<u64>,
    dimensions: Option<VideoDimensions>,
}

#[derive(Clone, Copy)]
struct SampleDefaults {
    duration: u32,
    size: u32,
}

struct TrackState {
    metadata: TrackMetadata,
    sample_count: u64,
    sample_bytes: u64,
    duration: u64,
    defaults: Option<SampleDefaults>,
}

fn parse_track(bytes: &[u8], media: &MediaData) -> Result<TrackState, Failure> {
    let tkhd = required(bytes, b"tkhd")?;
    let (id_offset, dimensions_offset, length) = if version(tkhd, 1)? == 0 {
        (12, 76, 84)
    } else {
        (20, 88, 96)
    };
    exact_length(tkhd, length)?;
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
    let mdhd = required(mdia, b"mdhd")?;
    let (timescale, declared_duration) = if version(mdhd, 1)? == 0 {
        exact_length(mdhd, 24)?;
        (word(mdhd, 12)?, u64::from(word(mdhd, 16)?))
    } else {
        exact_length(mdhd, 36)?;
        (word(mdhd, 20)?, wide(mdhd, 24)?)
    };
    if timescale == 0 {
        return Err(Failure::InvalidTiming);
    }
    let stbl = required(required(mdia, b"minf")?, b"stbl")?;
    let dimensions = sample_description(required(stbl, b"stsd")?, kind, tkhd, dimensions_offset)?;
    let (sample_count, duration) = timing(required(stbl, b"stts")?)?;
    let sizes = SampleSizes::parse(stbl)?;
    if sample_count != u64::from(sizes.count) {
        return Err(Failure::SampleCountMismatch);
    }
    let sample_bytes = progressive_ranges(stbl, &sizes, media)?;
    let edit_duration = find(bytes, b"edts")?.map(parse_edit).transpose()?;
    Ok(TrackState {
        metadata: TrackMetadata {
            id,
            kind,
            timescale,
            declared_duration,
            edit_duration,
            dimensions,
        },
        sample_count,
        sample_bytes,
        duration,
        defaults: None,
    })
}

fn sample_description(
    bytes: &[u8],
    kind: TrackKind,
    tkhd: &[u8],
    dimensions_offset: usize,
) -> Result<Option<VideoDimensions>, Failure> {
    version(bytes, 0)?;
    if word(bytes, 4)? != 1 {
        return Err(Failure::InvalidSampleTable);
    }
    let mut entries = Boxes::new(&bytes[8..]);
    let entry = entries.next().ok_or(Failure::MissingBox)??;
    if entries.next().is_some() {
        return Err(Failure::InvalidBox);
    }
    if matches!(entry.tag, b"encv" | b"enca") {
        return Err(Failure::InvalidBox);
    }
    let data = entry.data;
    if data.get(6..8).ok_or(Failure::Truncated)? == [0, 0] {
        return Err(Failure::InvalidBox);
    }
    let (header, dimensions) = match kind {
        TrackKind::Video => {
            let coded = word(data, 24)?;
            let (width, height) = (coded >> 16, coded & 0xffff);
            let display_width = word(tkhd, dimensions_offset)?;
            let display_height = word(tkhd, dimensions_offset + 4)?;
            if width == 0 || height == 0 || display_width == 0 || display_height == 0 {
                return Err(Failure::InvalidDimensions);
            }
            (
                78,
                Some(VideoDimensions {
                    width,
                    height,
                    display_width: Rational::new(u64::from(display_width), 65536),
                    display_height: Rational::new(u64::from(display_height), 65536),
                }),
            )
        }
        TrackKind::Audio => {
            if data.get(8..10).ok_or(Failure::Truncated)? != [0, 0] {
                return Err(Failure::UnsupportedVersion);
            }
            if word(data, 16)? >> 16 == 0 || word(data, 24)? == 0 {
                return Err(Failure::InvalidBox);
            }
            (28, None)
        }
    };
    validate_nesting(data.get(header..).ok_or(Failure::Truncated)?, 6)?;
    Ok(dimensions)
}

fn parse_edit(bytes: &[u8]) -> Result<u64, Failure> {
    let elst = find(bytes, b"elst")?.ok_or(Failure::EditList)?;
    let v = version(elst, 1)?;
    if word(elst, 4)? != 1 {
        return Err(Failure::EditList);
    }
    let (duration, media_time, rate_offset) = if v == 0 {
        exact_length(elst, 20)?;
        (
            u64::from(word(elst, 8)?),
            i64::from(word(elst, 12)? as i32),
            16,
        )
    } else {
        exact_length(elst, 28)?;
        (wide(elst, 8)?, wide(elst, 16)? as i64, 24)
    };
    if media_time < 0 || word(elst, rate_offset)? != 0x00010000 {
        return Err(Failure::EditList);
    }
    Ok(duration)
}

fn movie_timescale(bytes: &[u8]) -> Result<u32, Failure> {
    let (length, offset) = if version(bytes, 1)? == 0 {
        (100, 12)
    } else {
        (112, 20)
    };
    exact_length(bytes, length)?;
    let timescale = word(bytes, offset)?;
    if timescale == 0 {
        return Err(Failure::InvalidTiming);
    }
    Ok(timescale)
}

fn parse_defaults(bytes: &[u8], tracks: &mut [TrackState]) -> Result<(), Failure> {
    for item in Boxes::new(bytes) {
        let item = item?;
        if item.tag != b"trex" {
            continue;
        }
        version(item.data, 0)?;
        exact_length(item.data, 24)?;
        if word(item.data, 8)? != 1 {
            return Err(Failure::InvalidFragment);
        }
        let id = word(item.data, 4)?;
        let track = tracks
            .iter_mut()
            .find(|track| track.metadata.id == id)
            .ok_or(Failure::InvalidFragment)?;
        let defaults = SampleDefaults {
            duration: word(item.data, 12)?,
            size: word(item.data, 16)?,
        };
        if track.defaults.replace(defaults).is_some() {
            return Err(Failure::DuplicateBox);
        }
    }
    if tracks.iter().any(|track| track.defaults.is_none()) {
        return Err(Failure::InvalidFragment);
    }
    Ok(())
}

struct FragmentHeader {
    id: u32,
    base: u64,
    defaults: SampleDefaults,
}

fn fragment_header(
    bytes: &[u8],
    tracks: &[TrackState],
    moof_start: u64,
    implicit_base: u64,
) -> Result<FragmentHeader, Failure> {
    version(bytes, 0)?;
    let flags = word(bytes, 0)? & 0xffffff;
    if flags & !0x03003b != 0 || flags & 0x010000 != 0 || flags & 0x020001 == 0x020001 {
        return Err(Failure::InvalidFragment);
    }
    let id = word(bytes, 4)?;
    let mut defaults = tracks
        .iter()
        .find(|track| track.metadata.id == id)
        .and_then(|track| track.defaults)
        .ok_or(Failure::InvalidFragment)?;
    let mut offset = 8;
    let base = if flags & 1 != 0 {
        offset += 8;
        wide(bytes, 8)?
    } else if flags & 0x020000 != 0 {
        moof_start
    } else {
        implicit_base
    };
    if flags & 2 != 0 {
        if word(bytes, offset)? != 1 {
            return Err(Failure::InvalidFragment);
        }
        offset += 4;
    }
    if flags & 8 != 0 {
        defaults.duration = word(bytes, offset)?;
        offset += 4;
    }
    if flags & 0x10 != 0 {
        defaults.size = word(bytes, offset)?;
        offset += 4;
    }
    if flags & 0x20 != 0 {
        offset += 4;
    }
    exact_length(bytes, offset)?;
    Ok(FragmentHeader { id, base, defaults })
}

struct RunTotals {
    count: u64,
    duration: u64,
    bytes: u64,
    data_offset: Option<i32>,
}

fn run_totals(bytes: &[u8], defaults: SampleDefaults) -> Result<RunTotals, Failure> {
    version(bytes, 1)?;
    let flags = word(bytes, 0)? & 0xffffff;
    if flags & !0x000f05 != 0 || flags & 0x404 == 0x404 {
        return Err(Failure::InvalidFragment);
    }
    let count = u64::from(word(bytes, 4)?);
    let data_offset = if flags & 1 != 0 {
        Some(word(bytes, 8)? as i32)
    } else {
        None
    };
    let header = 8 + if flags & 1 != 0 { 4 } else { 0 } + if flags & 4 != 0 { 4 } else { 0 };
    let stride = ((flags & 0xf00).count_ones() * 4) as usize;
    table_length(bytes, header, count as usize, stride)?;
    let mut duration = if flags & 0x100 == 0 {
        mul(count, u64::from(defaults.duration))?
    } else {
        0
    };
    let mut sample_bytes = if flags & 0x200 == 0 {
        mul(count, u64::from(defaults.size))?
    } else {
        0
    };
    if flags & 0x300 != 0 {
        for entry in bytes[header..].chunks_exact(stride) {
            if flags & 0x100 != 0 {
                let delta = word(entry, 0)?;
                if delta == 0 {
                    return Err(Failure::InvalidTiming);
                }
                duration = add(duration, u64::from(delta))?;
            }
            if flags & 0x200 != 0 {
                let offset = if flags & 0x100 != 0 { 4 } else { 0 };
                sample_bytes = add(sample_bytes, u64::from(word(entry, offset)?))?;
            }
        }
    }
    if count > 0 && duration == 0 {
        return Err(Failure::InvalidFragment);
    }
    Ok(RunTotals {
        count,
        duration,
        bytes: sample_bytes,
        data_offset,
    })
}

fn fragment(
    bytes: &[u8],
    tracks: &mut [TrackState],
    moof_start: u64,
    implicit_base: u64,
    media_range: &Range<u64>,
) -> Result<u64, Failure> {
    let header = fragment_header(required(bytes, b"tfhd")?, tracks, moof_start, implicit_base)?;
    let track = tracks
        .iter_mut()
        .find(|track| track.metadata.id == header.id)
        .ok_or(Failure::InvalidFragment)?;
    if let Some(tfdt) = find(bytes, b"tfdt")? {
        let base = if version(tfdt, 1)? == 0 {
            exact_length(tfdt, 8)?;
            u64::from(word(tfdt, 4)?)
        } else {
            exact_length(tfdt, 12)?;
            wide(tfdt, 4)?
        };
        if base != track.duration {
            return Err(Failure::InvalidTiming);
        }
    }
    let mut runs = false;
    let mut end = header.base;
    for item in Boxes::new(bytes) {
        let item = item?;
        if item.tag != b"trun" {
            continue;
        }
        runs = true;
        let run = run_totals(item.data, header.defaults)?;
        let start = match run.data_offset {
            Some(offset) => header
                .base
                .checked_add_signed(i64::from(offset))
                .ok_or(Failure::Overflow)?,
            None => end,
        };
        end = add(start, run.bytes)?;
        if run.count > 0 && (start < media_range.start || end > media_range.end) {
            return Err(Failure::SampleDataOutsideMdat);
        }
        track.sample_count = add(track.sample_count, run.count)?;
        track.sample_bytes = add(track.sample_bytes, run.bytes)?;
        track.duration = add(track.duration, run.duration)?;
    }
    if !runs {
        return Err(Failure::MissingBox);
    }
    Ok(end)
}

fn following_mdat(bytes: &[u8], moof_end: u64) -> Result<Range<u64>, Failure> {
    for item in (Boxes {
        rest: &bytes[moof_end as usize..],
        offset: moof_end,
    }) {
        let item = item?;
        match item.tag {
            b"mdat" => return Ok(item.payload_start..item.end),
            b"moof" => break,
            _ => (),
        }
    }
    Err(Failure::MissingBox)
}

fn parse_fragments(
    bytes: &[u8],
    tracks: &mut [TrackState],
    fragmented: bool,
) -> Result<(), Failure> {
    let mut found = false;
    for item in Boxes::new(bytes) {
        let item = item?;
        if item.tag != b"moof" {
            continue;
        }
        if !fragmented {
            return Err(Failure::InvalidFragment);
        }
        let media_range = following_mdat(bytes, item.end)?;
        let mut implicit_base = item.start;
        let mut trafs = false;
        for child in Boxes::new(item.data) {
            let child = child?;
            if child.tag == b"traf" {
                trafs = true;
                implicit_base =
                    fragment(child.data, tracks, item.start, implicit_base, &media_range)?;
            }
        }
        if !trafs {
            return Err(Failure::MissingBox);
        }
        found = true;
    }
    if fragmented && !found {
        return Err(Failure::InvalidFragment);
    }
    Ok(())
}

fn microseconds(duration: u64, timescale: u32) -> Result<u64, Failure> {
    u64::try_from(u128::from(duration) * 1_000_000 / u128::from(timescale))
        .map_err(|_| Failure::Overflow)
}

fn finish_track(state: TrackState, movie_timescale: u32) -> Result<Track, Failure> {
    let metadata = state.metadata;
    if state.sample_count == 0 || state.duration == 0 {
        return Err(Failure::InvalidTiming);
    }
    if state.sample_bytes == 0 {
        return Err(Failure::InsufficientSampleData);
    }
    let media_duration_us = microseconds(state.duration, metadata.timescale)?;
    let mdhd_duration_us = microseconds(metadata.declared_duration, metadata.timescale)?;
    if metadata.kind == TrackKind::Audio && media_duration_us == 0 {
        return Err(Failure::InvalidTiming);
    }
    let duration_us = if let Some(duration) = metadata.edit_duration {
        // Movie timescales can round encoder durations up by one movie tick.
        if u128::from(duration) * u128::from(metadata.timescale)
            > u128::from(state.duration) * u128::from(movie_timescale)
                + u128::from(metadata.timescale)
        {
            return Err(Failure::EditDuration);
        }
        microseconds(duration, movie_timescale)?
    } else {
        media_duration_us
    };
    let fps = if metadata.kind == TrackKind::Video {
        Some(Rational::new(
            mul(state.sample_count, u64::from(metadata.timescale))?,
            state.duration,
        ))
    } else {
        None
    };
    let (width, height, display_width, display_height) = match metadata.dimensions {
        Some(dimensions) => (
            Some(dimensions.width),
            Some(dimensions.height),
            Some(dimensions.display_width),
            Some(dimensions.display_height),
        ),
        None => (None, None, None, None),
    };
    Ok(Track {
        kind: metadata.kind,
        duration_us,
        media_duration_us,
        mdhd_duration_us,
        sample_count: state.sample_count,
        sample_bytes: state.sample_bytes,
        width,
        height,
        display_width,
        display_height,
        fps,
    })
}

pub(crate) fn check(bytes: &[u8]) -> Result<Checked, Failure> {
    let first = Boxes::new(bytes).next().ok_or(Failure::MissingFtyp)??;
    if first.tag != b"ftyp" {
        return Err(Failure::MissingFtyp);
    }
    if first.data.len() < 8 || first.data.len() % 4 != 0 {
        return Err(Failure::InvalidBox);
    }
    required(bytes, b"ftyp")?;
    validate_nesting(bytes, 0)?;
    let moov = required(bytes, b"moov")?;
    let movie_timescale = movie_timescale(required(moov, b"mvhd")?)?;
    let media = MediaData::parse(bytes)?;
    let mut tracks = Vec::new();
    for item in Boxes::new(moov) {
        let item = item?;
        if item.tag != b"trak" {
            continue;
        }
        let track = parse_track(item.data, &media)?;
        if tracks
            .iter()
            .any(|t: &TrackState| t.metadata.id == track.metadata.id)
        {
            return Err(Failure::DuplicateBox);
        }
        if track.metadata.kind == TrackKind::Video
            && tracks.iter().any(|t| t.metadata.kind == TrackKind::Video)
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
        parse_defaults(mvex, &mut tracks)?;
    }
    parse_fragments(bytes, &mut tracks, mvex.is_some())?;
    let mut total = 0;
    let mut checked = Vec::with_capacity(tracks.len());
    for state in tracks {
        total = add(total, state.sample_bytes)?;
        checked.push(finish_track(state, movie_timescale)?);
    }
    if total > media.total {
        return Err(Failure::InsufficientSampleData);
    }
    Ok(Checked { tracks: checked })
}

#[cfg(test)]
#[path = "../tests/support/mp4_fixture.rs"]
mod fixture;

#[cfg(test)]
mod tests {
    use super::*;
    use fixture::{TrackSpec, box_bytes, progressive, words};

    const STBL: &[&[u8; 4]] = &[b"moov", b"trak", b"mdia", b"minf", b"stbl"];

    fn video() -> TrackSpec {
        TrackSpec::video(640, 480, 30, 1000, 40)
    }

    fn replace(bytes: &[u8], path: &[&[u8; 4]], replacement: &[u8]) -> Vec<u8> {
        let mut output = Vec::new();
        let mut replaced = false;
        for item in Boxes::new(bytes) {
            let item = item.unwrap();
            if !replaced && item.tag == path[0] {
                if path.len() == 1 {
                    output.extend(replacement);
                } else {
                    output.extend(box_bytes(
                        path[0],
                        &replace(item.data, &path[1..], replacement),
                    ));
                }
                replaced = true;
            } else {
                output.extend(&bytes[item.start as usize..item.end as usize]);
            }
        }
        assert!(replaced, "missing fixture box");
        output
    }

    fn payload<'a>(mut bytes: &'a [u8], path: &[&[u8; 4]]) -> &'a [u8] {
        for tag in path {
            bytes = Boxes::new(bytes)
                .map(Result::unwrap)
                .find(|item| item.tag == *tag)
                .expect("missing fixture box")
                .data;
        }
        bytes
    }

    fn patch(bytes: &[u8], path: &[&[u8; 4]], offset: usize, value: u32) -> Vec<u8> {
        let mut data = payload(bytes, path).to_vec();
        data[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        replace(bytes, path, &box_bytes(path.last().unwrap(), &data))
    }

    fn table_path(tag: &'static [u8; 4]) -> Vec<&'static [u8; 4]> {
        [STBL, &[tag]].concat()
    }

    fn fragment_fixture(
        tfhd: &[u32],
        runs: &[Vec<u8>],
        defaults: (u32, u32),
        payload_len: usize,
    ) -> Vec<u8> {
        let ftyp = box_bytes(b"ftyp", b"isom\0\0\0\0isom");
        let empty = TrackSpec {
            sample_count: 0,
            ..video()
        };
        let moov = box_bytes(
            b"moov",
            &[
                fixture::movie_header(1000),
                fixture::track(&empty, 1, 0),
                box_bytes(
                    b"mvex",
                    &box_bytes(b"trex", &words(&[0, 1, 1, defaults.0, defaults.1, 0])),
                ),
            ]
            .concat(),
        );
        let mut traf = box_bytes(b"tfhd", &words(tfhd));
        for run in runs {
            traf.extend(box_bytes(b"trun", run));
        }
        let mut moof = box_bytes(b"moof", &box_bytes(b"traf", &traf));
        let relative_start = moof.len() as u32 + 8;
        if tfhd[0] & 1 != 0 {
            let absolute_start = (ftyp.len() + moov.len()) as u32 + relative_start;
            moof = patch(&moof, &[b"moof", b"traf", b"tfhd"], 12, absolute_start);
        }
        if word(&runs[0], 0).unwrap() & 1 != 0 {
            moof = patch(
                &moof,
                &[b"moof", b"traf", b"trun"],
                8,
                if tfhd[0] & 1 != 0 { 0 } else { relative_start },
            );
        }
        [ftyp, moov, moof, box_bytes(b"mdat", &vec![1; payload_len])].concat()
    }

    fn fragmented() -> Vec<u8> {
        fragment_fixture(&[0x020000, 1], &[words(&[1, 3, 0])], (40, 4), 12)
    }

    #[test]
    fn progressive_video_and_audio() {
        let result = check(&progressive(
            &[video(), TrackSpec::audio(60, 1000, 20)],
            1000,
        ))
        .unwrap();
        assert_eq!(result.tracks.len(), 2);
        assert_eq!(
            result.tracks[0],
            Track {
                kind: TrackKind::Video,
                duration_us: 1_200_000,
                media_duration_us: 1_200_000,
                mdhd_duration_us: 1_200_000,
                sample_count: 30,
                sample_bytes: 120,
                width: Some(640),
                height: Some(480),
                display_width: Some(Rational {
                    numerator: 640,
                    denominator: 1
                }),
                display_height: Some(Rational {
                    numerator: 480,
                    denominator: 1
                }),
                fps: Some(Rational {
                    numerator: 25,
                    denominator: 1
                }),
            }
        );
        let audio = &result.tracks[1];
        assert_eq!(
            (
                audio.kind,
                audio.sample_count,
                audio.sample_bytes,
                audio.duration_us
            ),
            (TrackKind::Audio, 60, 240, 1_200_000)
        );
        assert_eq!(
            (
                audio.width,
                audio.height,
                audio.display_width,
                audio.display_height,
                audio.fps
            ),
            (None, None, None, None, None)
        );
    }

    #[test]
    fn sample_sizes_and_chunk_mappings() {
        let bytes = progressive(
            &[TrackSpec {
                sample_count: 3,
                ..video()
            }],
            1000,
        );
        let stsz_path = table_path(b"stsz");
        for (tag, data) in [
            (b"stsz", words(&[0, 0, 3, 1, 2, 3])),
            (b"stz2", [words(&[0, 4, 3]), vec![0x12, 0x30]].concat()),
            (b"stz2", [words(&[0, 8, 3]), vec![1, 2, 3]].concat()),
            (
                b"stz2",
                [words(&[0, 16, 3]), vec![0, 1, 0, 2, 0, 3]].concat(),
            ),
        ] {
            let checked = check(&replace(&bytes, &stsz_path, &box_bytes(tag, &data))).unwrap();
            assert_eq!(checked.tracks[0].sample_bytes, 6);
            assert_eq!(checked.tracks[0].sample_count, 3);
        }
        let bytes = progressive(
            &[TrackSpec {
                sample_count: 6,
                ..video()
            }],
            1000,
        );
        let offset = word(payload(&bytes, &table_path(b"stco")), 8).unwrap();
        let bytes = replace(
            &bytes,
            &table_path(b"stsc"),
            &box_bytes(b"stsc", &words(&[0, 3, 1, 2, 1, 2, 1, 1, 3, 3, 1])),
        );
        let bytes = replace(
            &bytes,
            &table_path(b"stco"),
            &box_bytes(b"stco", &words(&[0, 3, offset, offset + 8, offset + 12])),
        );
        assert_eq!(check(&bytes).unwrap().tracks[0].sample_bytes, 24);
        let co64 = [
            words(&[0, 3]),
            u64::from(offset).to_be_bytes().to_vec(),
            u64::from(offset + 8).to_be_bytes().to_vec(),
            u64::from(offset + 12).to_be_bytes().to_vec(),
        ]
        .concat();
        assert_eq!(
            check(&replace(
                &bytes,
                &table_path(b"stco"),
                &box_bytes(b"co64", &co64)
            ))
            .unwrap()
            .tracks[0]
                .sample_bytes,
            24
        );
    }

    #[test]
    fn declared_samples_require_physical_bytes() {
        let bytes = progressive(
            &[TrackSpec {
                sample_count: 300,
                sample_size: 1,
                ..video()
            }],
            1000,
        );
        let tiny = replace(&bytes, &[b"mdat"], &box_bytes(b"mdat", &[1; 4]));
        assert_eq!(check(&tiny), Err(Failure::SampleDataOutsideMdat));
        let bytes = progressive(&[video()], 1000);
        let first = word(payload(&bytes, &table_path(b"stco")), 8).unwrap();
        for offset in [first - 1, first + 1, u32::MAX] {
            assert_eq!(
                check(&patch(&bytes, &table_path(b"stco"), 8, offset)),
                Err(Failure::SampleDataOutsideMdat)
            );
        }
        let co64 = [words(&[0, 1]), u64::MAX.to_be_bytes().to_vec()].concat();
        assert_eq!(
            check(&replace(
                &bytes,
                &table_path(b"stco"),
                &box_bytes(b"co64", &co64)
            )),
            Err(Failure::Overflow)
        );
        let moov = required(&bytes, b"moov").unwrap();
        let overlapping_audio = fixture::track(&TrackSpec::audio(30, 1000, 40), 2, first);
        let duplicated = replace(
            &bytes,
            &[b"moov"],
            &box_bytes(b"moov", &[moov, &overlapping_audio].concat()),
        );
        assert_eq!(check(&duplicated), Err(Failure::InsufficientSampleData));
    }

    #[test]
    fn malformed_sample_tables_fail_closed() {
        let bytes = progressive(&[video()], 1000);
        assert_eq!(
            check(&patch(&bytes, &table_path(b"stsz"), 8, 29)),
            Err(Failure::SampleCountMismatch)
        );
        assert_eq!(
            check(&patch(&bytes, &table_path(b"stsc"), 12, 29)),
            Err(Failure::SampleCountMismatch)
        );
        for (offset, value) in [(8, 0), (8, 2), (12, 0), (16, 2)] {
            assert_eq!(
                check(&patch(&bytes, &table_path(b"stsc"), offset, value)),
                Err(Failure::InvalidSampleTable)
            );
        }
        for tag in [b"stsz", b"stts", b"stco", b"stsc", b"stsd"] {
            assert_eq!(
                check(&replace(&bytes, &table_path(tag), &[])),
                Err(Failure::MissingBox)
            );
        }
        let compact = box_bytes(b"stz2", &words(&[0, 3, 30]));
        assert_eq!(
            check(&replace(&bytes, &table_path(b"stsz"), &compact)),
            Err(Failure::InvalidSampleTable)
        );
        assert_eq!(
            check(&patch(&bytes, &table_path(b"stts"), 12, 0)),
            Err(Failure::InvalidTiming)
        );
        let duplicate_sizes = [
            box_bytes(b"stsz", payload(&bytes, &table_path(b"stsz"))),
            box_bytes(b"stz2", &words(&[0, 8, 0])),
        ]
        .concat();
        assert_eq!(
            check(&replace(&bytes, &table_path(b"stsz"), &duplicate_sizes)),
            Err(Failure::DuplicateBox)
        );
    }

    #[test]
    fn coded_and_display_dimensions_are_distinct() {
        let spec = TrackSpec {
            display_width: (854 << 16) + 32768,
            ..TrackSpec::video(720, 480, 30, 1000, 40)
        };
        let checked = check(&progressive(&[spec], 1000)).unwrap();
        assert_eq!(
            (checked.tracks[0].width, checked.tracks[0].height),
            (Some(720), Some(480))
        );
        assert_eq!(
            checked.tracks[0].display_width,
            Some(Rational {
                numerator: 1709,
                denominator: 2
            })
        );
        for spec in [
            TrackSpec {
                width: 0,
                ..video()
            },
            TrackSpec {
                display_height: 0,
                ..video()
            },
        ] {
            assert_eq!(
                check(&progressive(&[spec], 1000)),
                Err(Failure::InvalidDimensions)
            );
        }
    }

    #[test]
    fn audio_requires_samples_bytes_and_duration() {
        for spec in [
            TrackSpec::audio(0, 1000, 40),
            TrackSpec::audio(1, 1000, 0),
            TrackSpec::audio(1, u32::MAX, 1),
        ] {
            assert_eq!(
                check(&progressive(&[video(), spec], 1000)),
                Err(Failure::InvalidTiming)
            );
        }
        let spec = TrackSpec {
            sample_size: 0,
            ..TrackSpec::audio(1, 1000, 40)
        };
        assert_eq!(
            check(&progressive(&[video(), spec], 1000)),
            Err(Failure::InsufficientSampleData)
        );
    }

    #[test]
    fn fragmented_defaults_explicit_entries_and_contiguous_runs() {
        let checked = check(&fragmented()).unwrap();
        assert_eq!(
            (
                checked.tracks[0].sample_count,
                checked.tracks[0].sample_bytes,
                checked.tracks[0].media_duration_us
            ),
            (3, 12, 120_000)
        );
        let explicit = fragment_fixture(
            &[0x020018, 1, 20, 2],
            &[words(&[1, 2, 0]), words(&[0x300, 2, 40, 3, 80, 5])],
            (40, 4),
            12,
        );
        let checked = check(&explicit).unwrap();
        assert_eq!(
            (
                checked.tracks[0].sample_count,
                checked.tracks[0].sample_bytes,
                checked.tracks[0].media_duration_us
            ),
            (4, 12, 160_000)
        );
        assert_eq!(
            checked.tracks[0].fps,
            Some(Rational {
                numerator: 25,
                denominator: 1
            })
        );
        let absolute = fragment_fixture(&[1, 1, 0, 0], &[words(&[0, 3])], (40, 4), 12);
        assert_eq!(check(&absolute).unwrap().tracks[0].sample_bytes, 12);
        let sizes_only =
            fragment_fixture(&[0x020000, 1], &[words(&[0x201, 2, 0, 3, 5])], (40, 0), 8);
        assert_eq!(check(&sizes_only).unwrap().tracks[0].sample_bytes, 8);
    }

    #[test]
    fn fragment_ranges_must_fit_following_mdat() {
        let bytes = fragmented();
        let path = &[b"moof", b"traf", b"trun"];
        let offset = word(payload(&bytes, path), 8).unwrap();
        for offset in [offset - 1, offset + 1, u32::MAX] {
            assert_eq!(
                check(&patch(&bytes, path, 8, offset)),
                Err(Failure::SampleDataOutsideMdat)
            );
        }
        assert_eq!(
            check(&replace(&bytes, &[b"mdat"], &box_bytes(b"mdat", &[1; 4]))),
            Err(Failure::SampleDataOutsideMdat)
        );
        assert_eq!(
            check(&fragment_fixture(
                &[0x020000, 1],
                &[words(&[1, 3, 0])],
                (40, 0),
                12
            )),
            Err(Failure::InsufficientSampleData)
        );
        let explicit = fragment_fixture(
            &[0x020000, 1],
            &[words(&[0x301, 2, 0, 40, 3, 40, 10])],
            (0, 0),
            12,
        );
        assert_eq!(check(&explicit), Err(Failure::SampleDataOutsideMdat));
        let moof = Boxes::new(&bytes)
            .map(Result::unwrap)
            .find(|item| item.tag == b"moof")
            .unwrap();
        let mdat = box_bytes(b"mdat", &[1; 12]);
        let misplaced = [
            bytes[..moof.start as usize].to_vec(),
            mdat,
            bytes[moof.start as usize..moof.end as usize].to_vec(),
        ]
        .concat();
        assert_eq!(check(&misplaced), Err(Failure::MissingBox));
    }

    #[test]
    fn fragment_versions_defaults_and_timestamps_fail_closed() {
        let bytes = fragmented();
        assert_eq!(
            check(&patch(&bytes, &[b"moof", b"traf", b"tfhd"], 4, 2)),
            Err(Failure::InvalidFragment)
        );
        assert_eq!(
            check(&patch(&bytes, &[b"moof", b"traf", b"tfhd"], 0, 0x010000)),
            Err(Failure::InvalidFragment)
        );
        assert_eq!(
            check(&patch(&bytes, &[b"moov", b"mvex", b"trex"], 12, 0)),
            Err(Failure::InvalidFragment)
        );
        assert_eq!(
            check(&patch(&bytes, &[b"moov", b"mvex", b"trex"], 8, 2)),
            Err(Failure::InvalidFragment)
        );
        assert_eq!(
            check(&replace(&bytes, &[b"moov", b"mvex", b"trex"], &[])),
            Err(Failure::InvalidFragment)
        );
        assert_eq!(
            check(&replace(&bytes, &[b"moov", b"mvex"], &[])),
            Err(Failure::InvalidFragment)
        );
        assert_eq!(
            check(&replace(&bytes, &[b"moof"], &[])),
            Err(Failure::InvalidFragment)
        );
        let tfhd = box_bytes(b"tfhd", payload(&bytes, &[b"moof", b"traf", b"tfhd"]));
        let discontinuous = replace(
            &bytes,
            &[b"moof", b"traf", b"tfhd"],
            &[tfhd, box_bytes(b"tfdt", &words(&[0, 1]))].concat(),
        );
        assert_eq!(check(&discontinuous), Err(Failure::InvalidTiming));
        assert_eq!(
            check(&fragment_fixture(
                &[0x020000, 1],
                &[words(&[0x101, 1, 0, 0])],
                (40, 4),
                4
            )),
            Err(Failure::InvalidTiming)
        );
        assert_eq!(
            check(&fragment_fixture(
                &[0x020000, 1],
                &[words(&[0x405, 1, 0, 0, 0])],
                (40, 4),
                4
            )),
            Err(Failure::InvalidFragment)
        );
    }

    #[test]
    fn identity_delay_and_rounded_edits_preserve_media_duration() {
        for version_one in [false, true] {
            let video = TrackSpec {
                edit: Some((1200, 0)),
                version_one,
                ..video()
            };
            let audio = TrackSpec {
                edit: Some((1100, 100)),
                version_one,
                ..TrackSpec::audio(60, 1000, 20)
            };
            let checked = check(&progressive(&[video, audio], 1000)).unwrap();
            assert_eq!(
                (
                    checked.tracks[0].duration_us,
                    checked.tracks[0].media_duration_us
                ),
                (1_200_000, 1_200_000)
            );
            assert_eq!(
                (
                    checked.tracks[1].duration_us,
                    checked.tracks[1].media_duration_us
                ),
                (1_100_000, 1_200_000)
            );
        }
        let delayed = TrackSpec {
            edit: Some((2000, 200)),
            ..video()
        };
        let checked = check(&progressive(&[delayed], 2000)).unwrap();
        assert_eq!(
            (
                checked.tracks[0].duration_us,
                checked.tracks[0].media_duration_us
            ),
            (1_000_000, 1_200_000)
        );
        let rounded = TrackSpec {
            edit: Some((9534, 1024)),
            ..TrackSpec::video(16, 16, 286, 15360, 512)
        };
        let checked = check(&progressive(&[rounded], 1000)).unwrap();
        assert_eq!(
            (
                checked.tracks[0].duration_us,
                checked.tracks[0].media_duration_us
            ),
            (9_534_000, 9_533_333)
        );
        assert_eq!(checked.tracks[0].sample_count, 286);
        assert_eq!(
            checked.tracks[0].fps,
            Some(Rational {
                numerator: 30,
                denominator: 1
            })
        );
        let inflated = TrackSpec {
            edit: Some((3_600_000, 1100)),
            ..video()
        };
        assert_eq!(
            check(&progressive(&[inflated], 1000)),
            Err(Failure::EditDuration)
        );
        let beyond_tick = TrackSpec {
            edit: Some((9535, 1024)),
            ..rounded
        };
        assert_eq!(
            check(&progressive(&[beyond_tick], 1000)),
            Err(Failure::EditDuration)
        );
    }

    #[test]
    fn unsupported_edit_shapes_fail_closed() {
        let path = &[b"moov", b"trak", b"edts", b"elst"];
        let bytes = progressive(
            &[TrackSpec {
                edit: Some((1200, 0)),
                ..video()
            }],
            1000,
        );
        let two_entries = words(&[0, 2, 600, 0, 0x00010000, 600, 600, 0x00010000]);
        assert_eq!(
            check(&replace(&bytes, path, &box_bytes(b"elst", &two_entries))),
            Err(Failure::EditList)
        );
        for (offset, value) in [(4, 0), (12, u32::MAX), (16, 0x00020000), (16, 0x00010001)] {
            assert_eq!(
                check(&patch(&bytes, path, offset, value)),
                Err(Failure::EditList)
            );
        }
        assert_eq!(check(&replace(&bytes, path, &[])), Err(Failure::EditList));
        let negative_v1 = TrackSpec {
            version_one: true,
            edit: Some((1200, -1)),
            ..video()
        };
        assert_eq!(
            check(&progressive(&[negative_v1], 1000)),
            Err(Failure::EditList)
        );
    }

    #[test]
    fn versioned_headers_exact_fps_and_overflow() {
        let spec = TrackSpec {
            version_one: true,
            ..TrackSpec::video(640, 480, 30, 30000, 1001)
        };
        let bytes = progressive(&[spec], 1000);
        let checked = check(&bytes).unwrap();
        assert_eq!(
            checked.tracks[0].fps,
            Some(Rational {
                numerator: 30000,
                denominator: 1001
            })
        );
        assert_eq!(checked.tracks[0].media_duration_us, 1_001_000);
        let mut mvhd = vec![0; 112];
        mvhd[0] = 1;
        mvhd[20..24].copy_from_slice(&1000u32.to_be_bytes());
        assert!(
            check(&replace(
                &bytes,
                &[b"moov", b"mvhd"],
                &box_bytes(b"mvhd", &mvhd)
            ))
            .is_ok()
        );
        let overflow = words(&[0, 2, u32::MAX, u32::MAX, u32::MAX, u32::MAX]);
        assert_eq!(
            check(&replace(
                &bytes,
                &table_path(b"stts"),
                &box_bytes(b"stts", &overflow)
            )),
            Err(Failure::Overflow)
        );
        let fragmented_overflow = fragment_fixture(
            &[0x020000, 1],
            &[words(&[1, u32::MAX, 0]), words(&[0, u32::MAX])],
            (u32::MAX, 0),
            1,
        );
        assert_eq!(check(&fragmented_overflow), Err(Failure::Overflow));
        let converted_overflow = fragment_fixture(
            &[0x020000, 1],
            &[words(&[1, u32::MAX, 0]), words(&[0x300, 1, 1, 1])],
            (u32::MAX, 0),
            1,
        );
        let converted_overflow = patch(
            &converted_overflow,
            &[b"moov", b"trak", b"mdia", b"mdhd"],
            12,
            1,
        );
        assert_eq!(check(&converted_overflow), Err(Failure::Overflow));
    }

    #[test]
    fn framing_headers_and_required_boxes_fail_closed() {
        let bytes = progressive(&[video()], 1000);
        assert_eq!(check(&[]), Err(Failure::MissingFtyp));
        assert_eq!(check(&box_bytes(b"free", &[])), Err(Failure::MissingFtyp));
        let mut extended = bytes.clone();
        extended.extend(words(&[1]));
        extended.extend(b"free");
        extended.extend(16u64.to_be_bytes());
        extended.extend(words(&[0]));
        extended.extend(b"mdat");
        extended.extend([1; 8]);
        assert!(check(&extended).is_ok());
        assert_eq!(
            check(&[bytes.as_slice(), &[0, 0, 0, 4, b'f', b'r', b'e', b'e']].concat()),
            Err(Failure::InvalidBox)
        );
        for end in 0..bytes.len() {
            assert!(check(&bytes[..end]).is_err(), "prefix {end}");
        }
        for path in [&[b"moov"][..], &[b"mdat"][..], &[b"moov", b"mvhd"][..]] {
            assert_eq!(check(&replace(&bytes, path, &[])), Err(Failure::MissingBox));
        }
        assert_eq!(
            check(&[bytes.as_slice(), &box_bytes(b"moov", &[])].concat()),
            Err(Failure::DuplicateBox)
        );
        assert_eq!(
            check(&progressive(&[video(), video()], 1000)),
            Err(Failure::MultipleVideoTracks)
        );
        let moov = required(&bytes, b"moov").unwrap();
        assert_eq!(
            check(&replace(
                &bytes,
                &[b"moov"],
                &box_bytes(
                    b"moov",
                    &[moov, &box_bytes(b"trak", required(moov, b"trak").unwrap())].concat()
                )
            )),
            Err(Failure::DuplicateBox)
        );
        let mut nested = Vec::new();
        for _ in 0..18 {
            nested = box_bytes(b"moov", &nested);
        }
        assert_eq!(
            check(&[box_bytes(b"ftyp", b"isom\0\0\0\0"), nested].concat()),
            Err(Failure::NestingLimit)
        );
        assert_eq!(
            check(&patch(
                &bytes,
                &[b"moov", b"trak", b"mdia", b"hdlr"],
                8,
                u32::from_be_bytes(*b"text")
            )),
            Err(Failure::UnsupportedHandler)
        );
        assert_eq!(
            check(&patch(
                &bytes,
                &[b"moov", b"trak", b"mdia", b"mdhd"],
                0,
                0x02000000
            )),
            Err(Failure::UnsupportedVersion)
        );
        assert_eq!(
            check(&patch(&bytes, &[b"moov", b"mvhd"], 12, 0)),
            Err(Failure::InvalidTiming)
        );
        assert_eq!(
            check(&patch(&bytes, &[b"moov", b"trak", b"mdia", b"mdhd"], 12, 0)),
            Err(Failure::InvalidTiming)
        );
    }

    #[test]
    fn typed_failure_and_rational_round_trip() {
        let bytes = progressive(
            &[TrackSpec {
                width: 0,
                ..video()
            }],
            1000,
        );
        let failure = check(&bytes).unwrap_err();
        let encoded = serde_json::to_string(&failure).unwrap();
        assert_eq!(encoded, "\"invalid_dimensions\"");
        assert_eq!(serde_json::from_str::<Failure>(&encoded).unwrap(), failure);
        let rate = check(&progressive(&[video()], 1000)).unwrap().tracks[0]
            .fps
            .unwrap();
        let encoded = serde_json::to_string(&rate).unwrap();
        assert_eq!(encoded, "{\"numerator\":25,\"denominator\":1}");
        assert_eq!(serde_json::from_str::<Rational>(&encoded).unwrap(), rate);
        assert!(
            serde_json::from_str::<Rational>("{\"numerator\":25,\"denominator\":1,\"extra\":0}")
                .is_err()
        );
    }

    fn two_track_fragment(audio_base_is_moof: bool) -> (Vec<u8>, Vec<u8>) {
        let ftyp = box_bytes(b"ftyp", b"isom\0\0\0\0isom");
        let empty_video = TrackSpec {
            sample_count: 0,
            ..video()
        };
        let empty_audio = TrackSpec::audio(0, 1000, 20);
        let moov = box_bytes(
            b"moov",
            &[
                fixture::movie_header(1000),
                fixture::track(&empty_video, 1, 0),
                fixture::track(&empty_audio, 2, 0),
                box_bytes(
                    b"mvex",
                    &[
                        box_bytes(b"trex", &words(&[0, 1, 1, 40, 4, 0])),
                        box_bytes(b"trex", &words(&[0, 2, 1, 20, 4, 0])),
                    ]
                    .concat(),
                ),
            ]
            .concat(),
        );
        let video_traf = box_bytes(
            b"traf",
            &[
                box_bytes(b"tfhd", &words(&[0, 1])),
                box_bytes(b"trun", &words(&[1, 2, 0])),
            ]
            .concat(),
        );
        let audio_flags = if audio_base_is_moof { 0x020000 } else { 0 };
        let audio_run = if audio_base_is_moof {
            words(&[1, 3, 0])
        } else {
            words(&[0, 3])
        };
        let mut audio_traf = box_bytes(
            b"traf",
            &[
                box_bytes(b"tfhd", &words(&[audio_flags, 2])),
                box_bytes(b"trun", &audio_run),
            ]
            .concat(),
        );
        if audio_base_is_moof {
            let moof_size = 8 + video_traf.len() + audio_traf.len();
            let audio_offset = moof_size + 8 + 8;
            audio_traf = patch(&audio_traf, &[b"traf", b"trun"], 8, audio_offset as u32);
        }
        let mut moof = box_bytes(b"moof", &[video_traf, audio_traf].concat());
        moof = patch(
            &moof,
            &[b"moof", b"traf", b"trun"],
            8,
            moof.len() as u32 + 8,
        );
        let bytes = [ftyp, moov, moof.clone(), box_bytes(b"mdat", &[1; 20])].concat();
        (bytes, moof)
    }

    #[test]
    fn multiple_fragment_tracks_and_fragments_accumulate() {
        let (mut bytes, moof) = two_track_fragment(false);
        let checked = check(&bytes).unwrap();
        assert_eq!(
            (
                checked.tracks[0].sample_bytes,
                checked.tracks[0].sample_count
            ),
            (8, 2)
        );
        assert_eq!(
            (
                checked.tracks[1].sample_bytes,
                checked.tracks[1].sample_count
            ),
            (12, 3)
        );
        bytes.extend(moof);
        bytes.extend(box_bytes(b"mdat", &[1; 20]));
        let checked = check(&bytes).unwrap();
        assert_eq!(
            (
                checked.tracks[0].sample_count,
                checked.tracks[0].media_duration_us
            ),
            (4, 160_000)
        );
        assert_eq!(
            (
                checked.tracks[1].sample_count,
                checked.tracks[1].media_duration_us
            ),
            (6, 120_000)
        );
    }

    #[test]
    fn signed_fragment_offset_and_version_one_decode_time() {
        let bytes = fragment_fixture(&[1, 1, 0, 0], &[words(&[1, 3, 0])], (40, 4), 12);
        let base = word(payload(&bytes, &[b"moof", b"traf", b"tfhd"]), 12).unwrap();
        let bytes = patch(&bytes, &[b"moof", b"traf", b"tfhd"], 12, base + 4);
        let bytes = patch(&bytes, &[b"moof", b"traf", b"trun"], 8, (-4i32) as u32);
        assert_eq!(check(&bytes).unwrap().tracks[0].sample_bytes, 12);
        let bytes = fragmented();
        let tfhd = box_bytes(b"tfhd", payload(&bytes, &[b"moof", b"traf", b"tfhd"]));
        let bytes = replace(
            &bytes,
            &[b"moof", b"traf", b"tfhd"],
            &[tfhd, box_bytes(b"tfdt", &words(&[0x01000000, 0, 0]))].concat(),
        );
        let moof_size = payload(&bytes, &[b"moof"]).len() as u32 + 8;
        let bytes = patch(&bytes, &[b"moof", b"traf", b"trun"], 8, moof_size + 8);
        assert_eq!(check(&bytes).unwrap().tracks[0].sample_count, 3);
        assert_eq!(
            check(&patch(&bytes, &[b"moof", b"traf", b"tfdt"], 8, 1)),
            Err(Failure::InvalidTiming)
        );
    }

    #[test]
    fn extra_table_bytes_are_not_reported_as_truncation() {
        let bytes = progressive(&[video()], 1000);
        assert_eq!(
            check(&patch(&bytes, &table_path(b"stco"), 4, 0)),
            Err(Failure::InvalidBox)
        );
    }

    #[test]
    fn declared_mdhd_duration_is_descriptive() {
        let bytes = progressive(&[video()], 1000);
        let mismatched = patch(&bytes, &[b"moov", b"trak", b"mdia", b"mdhd"], 16, 1201);
        let checked = check(&mismatched).unwrap();
        let track = &checked.tracks[0];
        assert_eq!(track.mdhd_duration_us, 1_201_000);
        assert_eq!(track.media_duration_us, 1_200_000);
        assert_eq!(track.duration_us, 1_200_000);
        assert_eq!(track.sample_count, 30);
        assert_eq!(
            track.fps,
            Some(Rational {
                numerator: 25,
                denominator: 1
            })
        );
        let fragment = fragmented();
        assert_eq!(check(&fragment).unwrap().tracks[0].mdhd_duration_us, 0);
        let mismatched = patch(&fragment, &[b"moov", b"trak", b"mdia", b"mdhd"], 16, 999);
        let checked = check(&mismatched).unwrap();
        assert_eq!(checked.tracks[0].mdhd_duration_us, 999_000);
        assert_eq!(checked.tracks[0].media_duration_us, 120_000);
        assert_eq!(checked.tracks[0].sample_count, 3);
        assert_eq!(
            checked.tracks[0].fps,
            Some(Rational {
                numerator: 25,
                denominator: 1
            })
        );
    }

    #[test]
    fn second_traf_default_base_is_moof_not_previous_traf_end() {
        let (bytes, _) = two_track_fragment(true);
        let checked = check(&bytes).unwrap();
        assert_eq!(checked.tracks[0].kind, TrackKind::Video);
        assert_eq!(checked.tracks[1].kind, TrackKind::Audio);
        assert_eq!(
            (
                checked.tracks[0].sample_count,
                checked.tracks[0].sample_bytes
            ),
            (2, 8)
        );
        assert_eq!(
            (
                checked.tracks[1].sample_count,
                checked.tracks[1].sample_bytes
            ),
            (3, 12)
        );
        assert_eq!(checked.tracks[1].media_duration_us, 60_000);
    }
}
