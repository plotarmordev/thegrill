#[derive(Clone, Copy)]
pub struct TrackSpec {
    pub audio: bool,
    pub sample_count: u32,
    pub timescale: u32,
    pub sample_delta: u32,
    pub sample_size: u32,
    pub width: u16,
    pub height: u16,
    pub display_width: u32,
    pub display_height: u32,
    pub edit: Option<(u64, i64)>,
    pub version_one: bool,
}

impl TrackSpec {
    pub fn video(
        width: u16,
        height: u16,
        sample_count: u32,
        timescale: u32,
        sample_delta: u32,
    ) -> Self {
        Self {
            audio: false,
            sample_count,
            timescale,
            sample_delta,
            sample_size: 4,
            width,
            height,
            display_width: u32::from(width) << 16,
            display_height: u32::from(height) << 16,
            edit: None,
            version_one: false,
        }
    }

    pub fn audio(sample_count: u32, timescale: u32, sample_delta: u32) -> Self {
        Self {
            audio: true,
            ..Self::video(0, 0, sample_count, timescale, sample_delta)
        }
    }
}

pub fn words(values: &[u32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_be_bytes())
        .collect()
}

pub fn box_bytes(tag: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut bytes = words(&[u32::try_from(data.len() + 8).unwrap()]);
    bytes.extend(tag);
    bytes.extend(data);
    bytes
}

pub fn movie_header(timescale: u32) -> Vec<u8> {
    let mut data = vec![0; 100];
    data[12..16].copy_from_slice(&timescale.to_be_bytes());
    box_bytes(b"mvhd", &data)
}

fn sample_description(spec: &TrackSpec) -> Vec<u8> {
    let mut entry = vec![0; if spec.audio { 28 } else { 78 }];
    entry[7] = 1;
    if spec.audio {
        entry[16..18].copy_from_slice(&2u16.to_be_bytes());
        entry[18..20].copy_from_slice(&16u16.to_be_bytes());
        entry[24..28].copy_from_slice(&(48000u32 << 16).to_be_bytes());
    } else {
        entry[24..26].copy_from_slice(&spec.width.to_be_bytes());
        entry[26..28].copy_from_slice(&spec.height.to_be_bytes());
    }
    box_bytes(
        b"stsd",
        &[
            words(&[0, 1]),
            box_bytes(if spec.audio { b"mp4a" } else { b"avc1" }, &entry),
        ]
        .concat(),
    )
}

fn edit_list(spec: &TrackSpec) -> Vec<u8> {
    let Some((duration, media_time)) = spec.edit else {
        return Vec::new();
    };
    let data = if spec.version_one {
        [
            words(&[0x01000000, 1]),
            duration.to_be_bytes().to_vec(),
            media_time.to_be_bytes().to_vec(),
            words(&[0x00010000]),
        ]
        .concat()
    } else {
        words(&[
            0,
            1,
            u32::try_from(duration).unwrap(),
            i32::try_from(media_time).unwrap() as u32,
            0x00010000,
        ])
    };
    box_bytes(b"edts", &box_bytes(b"elst", &data))
}

pub fn track(spec: &TrackSpec, id: u32, chunk_offset: u32) -> Vec<u8> {
    let (length, id_offset, dimensions_offset) = if spec.version_one {
        (96, 20, 88)
    } else {
        (84, 12, 76)
    };
    let mut tkhd = vec![0; length];
    tkhd[0] = u8::from(spec.version_one);
    tkhd[id_offset..id_offset + 4].copy_from_slice(&id.to_be_bytes());
    tkhd[dimensions_offset..dimensions_offset + 4]
        .copy_from_slice(&spec.display_width.to_be_bytes());
    tkhd[dimensions_offset + 4..dimensions_offset + 8]
        .copy_from_slice(&spec.display_height.to_be_bytes());
    let duration = u64::from(spec.sample_count) * u64::from(spec.sample_delta);
    let mdhd = if spec.version_one {
        [
            words(&[0x01000000, 0, 0, 0, 0, spec.timescale]),
            duration.to_be_bytes().to_vec(),
            words(&[0]),
        ]
        .concat()
    } else {
        words(&[0, 0, 0, spec.timescale, u32::try_from(duration).unwrap(), 0])
    };
    let mut handler = vec![0; 24];
    handler[8..12].copy_from_slice(if spec.audio { b"soun" } else { b"vide" });
    let (stts, stsc, stco) = if spec.sample_count == 0 {
        (words(&[0, 0]), words(&[0, 0]), words(&[0, 0]))
    } else {
        (
            words(&[0, 1, spec.sample_count, spec.sample_delta]),
            words(&[0, 1, 1, spec.sample_count, 1]),
            words(&[0, 1, chunk_offset]),
        )
    };
    let stsz = if spec.sample_size == 0 {
        [
            words(&[0, 0, spec.sample_count]),
            vec![0; spec.sample_count as usize * 4],
        ]
        .concat()
    } else {
        words(&[0, spec.sample_size, spec.sample_count])
    };
    let stbl = [
        sample_description(spec),
        box_bytes(b"stts", &stts),
        box_bytes(b"stsz", &stsz),
        box_bytes(b"stsc", &stsc),
        box_bytes(b"stco", &stco),
    ]
    .concat();
    let mdia = [
        box_bytes(b"mdhd", &mdhd),
        box_bytes(b"hdlr", &handler),
        box_bytes(b"minf", &box_bytes(b"stbl", &stbl)),
    ]
    .concat();
    box_bytes(
        b"trak",
        &[
            box_bytes(b"tkhd", &tkhd),
            edit_list(spec),
            box_bytes(b"mdia", &mdia),
        ]
        .concat(),
    )
}

pub fn progressive(tracks: &[TrackSpec], movie_timescale: u32) -> Vec<u8> {
    let ftyp = box_bytes(b"ftyp", b"isom\0\0\0\0isom");
    let mut moov = movie_header(movie_timescale);
    let mut payload = Vec::new();
    for (index, spec) in tracks.iter().enumerate() {
        let offset = u32::try_from(ftyp.len() + 8 + payload.len()).unwrap();
        moov.extend(track(spec, u32::try_from(index + 1).unwrap(), offset));
        payload.resize(
            payload.len() + spec.sample_count as usize * spec.sample_size as usize,
            1,
        );
    }
    [
        ftyp,
        box_bytes(b"mdat", &payload),
        box_bytes(b"moov", &moov),
    ]
    .concat()
}
