use super::*;
use std::{fs, io::Read};

pub(super) const FILE: &str = "markers.ndjson";
pub(super) const CAP: usize = 16 * 1024;

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(super) enum Transport {
    File,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Name {
    RunnerStarted,
    ModelLoaded,
    GenerationStarted,
    GenerationFinished,
    OutputWritten,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Line {
    grill_media: u64,
    event: Name,
    monotonic_ns: u64,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Event {
    event: Name,
    monotonic_ns: u64,
    offset_us: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub(super) enum Status {
    Valid,
    Missing,
    Unreadable,
    Oversized,
    Invalid,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Markers {
    path: String,
    clock: String,
    pub status: Status,
    detail: Option<String>,
    sha256: Option<String>,
    bytes: Option<u64>,
    events: Vec<Event>,
    pub generation_us: Option<u64>,
    pub load_to_generation_us: Option<u64>,
}
impl Markers {
    fn empty(status: Status) -> Self {
        Self {
            path: FILE.into(),
            clock: "runner-monotonic-ns".into(),
            status,
            detail: None,
            sha256: None,
            bytes: None,
            events: Vec::new(),
            generation_us: None,
            load_to_generation_us: None,
        }
    }
    pub(super) fn delta_ns(&self, metric: Metric) -> Option<u64> {
        if self.status != Status::Valid {
            return None;
        }
        let (start, end) = match metric {
            Metric::Generation => (Name::GenerationStarted, Name::GenerationFinished),
            Metric::LoadToGeneration => (Name::RunnerStarted, Name::GenerationStarted),
            Metric::EndToEnd => return None,
        };
        let stamp = |name| {
            self.events
                .iter()
                .find(|event| event.event == name)
                .map(|event| event.monotonic_ns)
        };
        Some(stamp(end)? - stamp(start)?)
    }
}

pub(super) fn collect(dir: &Path, end_to_end_us: u64) -> Markers {
    let path = dir.join(FILE);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) => {
            return Markers::empty(if error.kind() == std::io::ErrorKind::NotFound {
                Status::Missing
            } else {
                Status::Unreadable
            });
        }
    };
    if !metadata.is_file() {
        return Markers::empty(Status::Unreadable);
    }
    if metadata.len() > CAP as u64 {
        return Markers::empty(Status::Oversized);
    }
    let mut bytes = Vec::new();
    let read =
        fs::File::open(path).and_then(|file| file.take(CAP as u64 + 1).read_to_end(&mut bytes));
    if read.is_err() {
        return Markers::empty(Status::Unreadable);
    }
    if bytes.len() > CAP {
        return Markers::empty(Status::Oversized);
    }
    parse(&bytes, end_to_end_us)
}

pub(super) fn parse(bytes: &[u8], end_to_end_us: u64) -> Markers {
    let mut record = Markers::empty(Status::Invalid);
    record.sha256 = Some(evidence::digest(bytes));
    record.bytes = Some(bytes.len() as u64);
    match events(bytes, end_to_end_us) {
        Ok(events) => {
            record.status = Status::Valid;
            record.events = events;
            record.generation_us = record.delta_ns(Metric::Generation).map(|ns| ns / 1000);
            record.load_to_generation_us = record
                .delta_ns(Metric::LoadToGeneration)
                .map(|ns| ns / 1000);
        }
        Err(detail) => record.detail = Some(detail),
    }
    record
}

fn events(bytes: &[u8], end_to_end_us: u64) -> Result<Vec<Event>> {
    if bytes.is_empty() {
        return Err("line 1: empty marker file".into());
    }
    let mut events: Vec<Event> = Vec::new();
    for (index, bytes) in bytes.split_inclusive(|byte| *byte == b'\n').enumerate() {
        let fail = |detail: &str| format!("line {}: {detail}", index + 1);
        if !bytes.ends_with(b"\n") {
            return Err(fail("missing final newline"));
        }
        let line: Line = super::parse(bytes).map_err(|error| fail(&error))?;
        if line.grill_media != 1 {
            return Err(fail("grill_media must be 1"));
        }
        if let Some(previous) = events.last() {
            if line.event <= previous.event {
                return Err(fail("duplicate or out-of-order event"));
            }
            if line.monotonic_ns < previous.monotonic_ns {
                return Err(fail("monotonic_ns regression"));
            }
        }
        let first = events
            .first()
            .map_or(line.monotonic_ns, |event| event.monotonic_ns);
        let span = line.monotonic_ns - first;
        if u128::from(span) > u128::from(end_to_end_us) * 1000 {
            return Err(fail("marker span exceeds end_to_end_us"));
        }
        events.push(Event {
            event: line.event,
            monotonic_ns: line.monotonic_ns,
            offset_us: span / 1000,
        });
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_lines_and_truncation() {
        for bytes in [
            b"".as_slice(), b"\xff\n", b"{\n", b"[]\n", b"{}\n",
            b"{\"grill_media\":1,\"grill_media\":1,\"event\":\"runner_started\",\"monotonic_ns\":0}\n",
            b"{\"grill_media\":1,\"event\":\"runner_started\",\"monotonic_ns\":-1}\n",
            b"{\"grill_media\":1,\"event\":\"runner_started\",\"monotonic_ns\":18446744073709551616}\n",
            b"{\"grill_media\":1,\"event\":\"runner_started\",\"monotonic_ns\":0}",
        ] {
            let record = parse(bytes, u64::MAX);
            assert_eq!(record.status, Status::Invalid);
            assert!(record.detail.unwrap().starts_with("line 1:"));
            assert!(record.events.is_empty());
            assert!(record.sha256.is_some());
        }
    }

    #[test]
    fn exact_differences_and_overflow_safe_containment() {
        let bytes = b"{\"grill_media\":1,\"event\":\"runner_started\",\"monotonic_ns\":0}\n{\"grill_media\":1,\"event\":\"generation_started\",\"monotonic_ns\":1001}\n{\"grill_media\":1,\"event\":\"generation_finished\",\"monotonic_ns\":2002}\n";
        let record = parse(bytes, 3);
        assert_eq!(record.status, Status::Valid);
        assert_eq!(record.generation_us, Some(1));
        assert_eq!(record.delta_ns(Metric::Generation), Some(1001));
        assert_eq!(record.load_to_generation_us, Some(1));
        assert_eq!(parse(bytes, 2).status, Status::Invalid);
        assert_eq!(parse(bytes, u64::MAX).status, Status::Valid);
        let equal = b"{\"grill_media\":1,\"event\":\"generation_started\",\"monotonic_ns\":18446744073709551615}\n{\"grill_media\":1,\"event\":\"generation_finished\",\"monotonic_ns\":18446744073709551615}\n";
        assert_eq!(parse(equal, 0).generation_us, Some(0));
    }
}
