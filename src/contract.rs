use serde::{Deserialize, Serialize};

pub(crate) const PACK_CAP: usize = 16 * 1024 * 1024;
pub(crate) const SUBMISSION_CAP: usize = 32 * 1024 * 1024;
pub(crate) const RECEIPT_CAP: usize = 1024 * 1024;
pub(crate) const VIEW_CAP: usize = PACK_CAP + SUBMISSION_CAP + RECEIPT_CAP;
pub(crate) const CASE_CAP: usize = 1024;
pub(crate) const ARTIFACT_CAP: usize = 1024 * 1024;
pub(crate) const MESSAGE_CAP: usize = 256 * 1024;
// Runner collection-protection ceilings (engineering limits, not measured optima).
pub(crate) const REQUEST_CAP: usize = 256 * 1024;
pub(crate) const RESPONSE_CAP: usize = 8 * 1024 * 1024;
pub(crate) const SSE_LINE_CAP: usize = 256 * 1024;
pub(crate) const SSE_EVENT_CAP: usize = 256 * 1024;
pub(crate) const PLAN_CAP: usize = 64 * 1024;
// A 256 KiB request escaped as a JSON string expands at most sixfold.
pub(crate) const RESERVATION_CAP: usize = 2 * 1024 * 1024;
pub(crate) const TERMINAL_CAP: usize = 64 * 1024;
pub(crate) const DETAIL_CAP: usize = 256;
pub(crate) type Result<T, E = String> = std::result::Result<T, E>;

#[derive(Debug, Serialize)]
pub(crate) struct Pack {
    pub version: u32,
    pub label: String,
    pub worlds: Vec<String>,
    pub groups: Vec<String>,
    pub cases: Vec<Case>,
}

#[derive(Debug)]
pub(crate) struct Case {
    pub id: String,
    pub world: String,
    pub group: String,
    pub messages: Vec<Message>,
    pub acceptance: Acceptance,
    pub qualification: Qualification,
    pub provenance: Option<CaseProvenance>,
}

// Archival v1 remains a distinct relation, never inferred from a v2 string set.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum Acceptance {
    #[serde(skip)]
    ArchivalV1 {
        accepted: Vec<String>,
    },
    StringSet {
        accepted: Vec<String>,
    },
    GridSet {
        outputs: Grids,
    },
    // Final-marker envelope over the same typed answers: prose preamble, exactly
    // one standalone ===FINAL=== line, then one strict answer object with only
    // trailing whitespace. Grading parses only the borrowed suffix.
    FinalStringSet {
        accepted: Vec<String>,
    },
    FinalGridSet {
        outputs: Grids,
    },
    FinalTextSet {
        accepted: Vec<String>,
    },
    FinalNumber {
        number: NumberAcceptance,
    },
    JsonExact {
        expected: crate::grade::atlas::ExactJson,
    },
    TextConstraints {
        rules: crate::record::Bounded<TextRule, 16>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CaseProvenance {
    pub repository: String,
    pub revision: String,
    pub path: String,
    pub sha256: String,
    pub item_id: String,
    pub license: String,
    pub changes: crate::record::Bounded<String, 64>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NumberAcceptance {
    pub expected: f64,
    pub absolute_tolerance: f64,
    pub relative_tolerance: f64,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum TextRule {
    Words { min: u32, max: u32 },
    Characters { min: u32, max: u32 },
    StartsWith { text: String },
    EndsWith { text: String },
    Contains { text: String },
    Excludes { text: String },
    Lowercase,
    Uppercase,
}

// Present fields must have their real type, not null; derive rejects duplicates.
fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> std::result::Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

impl<'de> Deserialize<'de> for TextRule {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Rule {
            kind: String,
            #[serde(default, deserialize_with = "present")]
            min: Option<u32>,
            #[serde(default, deserialize_with = "present")]
            max: Option<u32>,
            #[serde(default, deserialize_with = "present")]
            text: Option<String>,
        }
        let rule: Rule = crate::record::object(d)?;
        match (rule.kind.as_str(), rule.min, rule.max, rule.text) {
            ("words", Some(min), Some(max), None) => Ok(Self::Words { min, max }),
            ("characters", Some(min), Some(max), None) => Ok(Self::Characters { min, max }),
            ("starts-with", None, None, Some(text)) => Ok(Self::StartsWith { text }),
            ("ends-with", None, None, Some(text)) => Ok(Self::EndsWith { text }),
            ("contains", None, None, Some(text)) => Ok(Self::Contains { text }),
            ("excludes", None, None, Some(text)) => Ok(Self::Excludes { text }),
            ("lowercase", None, None, None) => Ok(Self::Lowercase),
            ("uppercase", None, None, None) => Ok(Self::Uppercase),
            _ => Err(serde::de::Error::custom("invalid closed text rule")),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub(crate) struct Grids(pub crate::record::Bounded<Grid, 8>);

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(transparent)]
pub(crate) struct Grid(pub crate::record::Bounded<Row, 30>);

pub(crate) type Row = crate::record::Bounded<Cell, 30>;

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(transparent)]
pub(crate) struct Cell(pub u8);

impl<'de> Deserialize<'de> for Cell {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let value = u8::deserialize(d)?;
        if value > 9 {
            return Err(serde::de::Error::custom(
                "grid cell must be an integer 0..=9",
            ));
        }
        Ok(Self(value))
    }
}

impl<'de> Deserialize<'de> for Grid {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let rows = crate::record::Bounded::<Row, 30>::deserialize(d)?;
        let width = rows.0[0].0.len();
        if rows.0.iter().any(|row| row.0.len() != width) {
            return Err(serde::de::Error::custom("grid must be rectangular"));
        }
        Ok(Self(rows))
    }
}

impl Serialize for Case {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut record = s.serialize_struct("Case", 6 + usize::from(self.provenance.is_some()))?;
        record.serialize_field("id", &self.id)?;
        record.serialize_field("world", &self.world)?;
        record.serialize_field("group", &self.group)?;
        record.serialize_field("messages", &self.messages)?;
        match &self.acceptance {
            Acceptance::ArchivalV1 { accepted } => record.serialize_field("accepted", accepted)?,
            acceptance => record.serialize_field("acceptance", acceptance)?,
        }
        record.serialize_field("qualification", &self.qualification)?;
        if let Some(provenance) = &self.provenance {
            record.serialize_field("provenance", provenance)?;
        }
        record.end()
    }
}

impl<'de> Deserialize<'de> for Acceptance {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        use serde::de::{self, MapAccess, Visitor};
        struct AcceptanceVisitor;
        impl<'de> Visitor<'de> for AcceptanceVisitor {
            type Value = Acceptance;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a closed acceptance object")
            }
            fn visit_map<M: MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Acceptance, M::Error> {
                #[derive(Deserialize)]
                #[serde(field_identifier, rename_all = "snake_case")]
                enum Field {
                    Kind,
                    Accepted,
                    Outputs,
                    Number,
                    Expected,
                    Rules,
                }
                let (mut kind, mut accepted, mut outputs) = (None, None, None);
                let (mut number, mut expected, mut rules) = (None, None, None);
                while let Some(field) = map.next_key()? {
                    match field {
                        Field::Kind => {
                            if kind.is_some() {
                                return Err(de::Error::duplicate_field("kind"));
                            }
                            kind = Some(map.next_value::<String>()?);
                        }
                        Field::Accepted => {
                            if accepted.is_some() {
                                return Err(de::Error::duplicate_field("accepted"));
                            }
                            accepted =
                                Some(map.next_value::<crate::record::Bounded<String, 64>>()?);
                        }
                        Field::Outputs => {
                            if outputs.is_some() {
                                return Err(de::Error::duplicate_field("outputs"));
                            }
                            outputs = Some(map.next_value::<Grids>()?);
                        }
                        Field::Number => {
                            if number.is_some() {
                                return Err(de::Error::duplicate_field("number"));
                            }
                            number = Some(map.next_value::<crate::grade::atlas::NumberObject>()?.0);
                        }
                        Field::Expected => {
                            if expected.is_some() {
                                return Err(de::Error::duplicate_field("expected"));
                            }
                            expected = Some(map.next_value::<crate::grade::atlas::ExactJson>()?);
                        }
                        Field::Rules => {
                            if rules.is_some() {
                                return Err(de::Error::duplicate_field("rules"));
                            }
                            rules = Some(map.next_value::<crate::record::Bounded<TextRule, 16>>()?);
                        }
                    }
                }
                match (kind.as_deref(), accepted, outputs, number, expected, rules) {
                    (Some("string-set"), Some(accepted), None, None, None, None) => {
                        Ok(Acceptance::StringSet {
                            accepted: accepted.0,
                        })
                    }
                    (Some("grid-set"), None, Some(outputs), None, None, None) => {
                        Ok(Acceptance::GridSet { outputs })
                    }
                    (Some("final-string-set"), Some(accepted), None, None, None, None) => {
                        Ok(Acceptance::FinalStringSet {
                            accepted: accepted.0,
                        })
                    }
                    (Some("final-grid-set"), None, Some(outputs), None, None, None) => {
                        Ok(Acceptance::FinalGridSet { outputs })
                    }
                    (Some("final-text-set"), Some(accepted), None, None, None, None) => {
                        Ok(Acceptance::FinalTextSet {
                            accepted: accepted.0,
                        })
                    }
                    (Some("final-number"), None, None, Some(number), None, None) => {
                        Ok(Acceptance::FinalNumber { number })
                    }
                    (Some("json-exact"), None, None, None, Some(expected), None) => {
                        Ok(Acceptance::JsonExact { expected })
                    }
                    (Some("text-constraints"), None, None, None, None, Some(rules)) => {
                        Ok(Acceptance::TextConstraints { rules })
                    }
                    _ => Err(de::Error::custom(
                        "acceptance requires exactly kind and its associated field",
                    )),
                }
            }
        }
        d.deserialize_map(AcceptanceVisitor)
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Message {
    #[serde(deserialize_with = "crate::record::string_enum")]
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Qualification {
    pub valid: Vec<String>,
    pub wrong: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct System {
    pub name: String,
    pub model: String,
    pub endpoint: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Submission {
    pub version: u32,
    #[serde(deserialize_with = "crate::record::object")]
    pub system: System,
    #[serde(deserialize_with = "crate::record::object")]
    pub protocol: Protocol,
    #[serde(deserialize_with = "crate::record::objects")]
    pub answers: Vec<AnswerEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AnswerEntry {
    pub case_id: String,
    pub artifact: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Protocol {
    #[serde(deserialize_with = "crate::record::string_enum")]
    pub profile: Profile,
    pub stream: bool,
    #[serde(deserialize_with = "crate::record::object")]
    pub token_cap: TokenCap,
    pub temperature_milli: Option<u16>,
    pub top_p_milli: Option<u16>,
    pub seed: Option<i64>,
    // Profile v2 request controls, optional strictly by absence: a present
    // field must carry its real value and an explicit null is rejected, so the
    // archival v1 wire cannot silently grow spellings its frozen reader
    // refuses. None is never serialized, keeping v1 records and identities
    // byte-exact; validation ties presence to the declared profile.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::record::present_string_enum"
    )]
    pub reasoning_effort: Option<Effort>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::record::present_bool"
    )]
    pub include_usage: Option<bool>,
    #[serde(deserialize_with = "crate::record::object")]
    pub collection: Collection,
    #[serde(deserialize_with = "crate::record::object")]
    pub rendering: Rendering,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Profile {
    DeclaredChatCompletionsV1,
    DeclaredChatCompletionsV2,
}

/// The closed request-schema reasoning-effort ladder, sent verbatim when
/// requested. Per-model support and effective backend effort are not verified
/// or negotiated by Grill; an unsupported value fails loudly without retry.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Effort {
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TokenCap {
    #[serde(deserialize_with = "crate::record::string_enum")]
    pub field: TokenField,
    pub value: u32,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TokenField {
    MaxTokens,
    MaxCompletionTokens,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Collection {
    pub total_ms: u32,
    pub idle_ms: u32,
    pub response_bytes: u32,
    pub artifact_bytes: u32,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RenderingStatus {
    Known,
    Unknown,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Rendering {
    #[serde(deserialize_with = "crate::record::string_enum")]
    pub status: RenderingStatus,
    pub template: Option<String>,
    pub tokenizer: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct PackIdentities {
    pub source: String,
    pub tasks: String,
    pub target: String,
    pub qualification: String,
    pub grading: String,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Identities {
    #[serde(deserialize_with = "crate::record::object")]
    pub pack: PackIdentities,
    pub submission: String,
    pub protocol: String,
    pub system: String,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Evidence {
    Submitted,
    Missing,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Outcome {
    Success,
    Wrong,
    Malformed,
    Refused,
    Unknown,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CaseGrade {
    pub case_id: String,
    pub input: String,
    pub artifact: Option<String>,
    #[serde(deserialize_with = "crate::record::string_enum")]
    pub evidence: Evidence,
    #[serde(deserialize_with = "crate::record::string_enum")]
    pub outcome: Outcome,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct View {
    pub version: u32,
    #[serde(deserialize_with = "crate::record::string_enum")]
    pub claim: ViewClaim,
    #[serde(deserialize_with = "crate::record::object")]
    pub identities: Identities,
    #[serde(deserialize_with = "crate::record::objects")]
    pub cases: Vec<CaseGrade>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ViewClaim {
    SubmittedArtifactsNotVerifiedExecution,
}

// ----- Runner records: client-collected evidence, never authenticated execution -----

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Transport {
    pub profile: String,
    pub http: String,
    pub tls: String,
    pub auth_env: Option<String>,
    pub redirects: String,
    pub retries: String,
    pub proxy: String,
    pub content_encoding: String,
    pub commitment: String,
    pub request_cap: u32,
    pub sse_line_cap: u32,
    pub sse_event_cap: u32,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Provenance {
    pub grill: String,
    pub lockfile: String,
    pub os: String,
    pub arch: String,
    pub started_unix_ms: u64,
    pub pid: u32,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Plan {
    pub version: u32,
    #[serde(deserialize_with = "crate::record::string_enum")]
    pub claim: PlanClaim,
    #[serde(deserialize_with = "crate::record::object")]
    pub pack: PackIdentities,
    pub cases: u32,
    #[serde(deserialize_with = "crate::record::object")]
    pub system: System,
    #[serde(deserialize_with = "crate::record::object")]
    pub protocol: Protocol,
    #[serde(deserialize_with = "crate::record::object")]
    pub transport: Transport,
    #[serde(deserialize_with = "crate::record::object")]
    pub provenance: Provenance,
    pub bound_bytes: u64,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum PlanClaim {
    ClientCollectionIntentNotAuthenticatedExecution,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Reservation {
    pub version: u32,
    pub attempt: u32,
    pub case_id: String,
    pub input: String,
    pub plan: String,
    pub endpoint: String,
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub auth_env: Option<String>,
    pub request: String,
    pub body: String,
    pub started_unix_ms: u64,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Reason {
    Committed,
    Refused,
    UnsupportedResponse,
    MalformedEnvelope,
    ProviderError,
    HttpStatus,
    HttpEntity,
    TransportFailure,
    StreamIncomplete,
    TotalDeadline,
    IdleDeadline,
    ResponseCap,
    ArtifactCap,
    FrameCap,
    Interrupted,
    LocalIo,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Commitment {
    Committed,
    Uncommitted,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Finish {
    Stop,
    Length,
    ContentFilter,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq, Default, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct Usage {
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Http {
    pub status: u16,
    pub version: String,
    pub content_type: Option<String>,
    pub content_length: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Body {
    pub bytes: u64,
    pub sha256: String,
    pub complete: bool,
    pub truncated: bool,
    pub terminal_offset: Option<u64>,
    pub surplus_retained: u64,
    pub surplus_observed: u64,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Artifact {
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Timing {
    pub started_unix_ms: u64,
    pub reserve_ms: u64,
    pub headers_ms: Option<u64>,
    pub first_body_ms: Option<u64>,
    pub settle_ms: u64,
    pub local_ms: u64,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Terminal {
    pub version: u32,
    pub attempt: u32,
    pub case_id: String,
    pub input: String,
    pub reservation: String,
    pub request: String,
    pub dispatched: bool,
    #[serde(deserialize_with = "crate::record::string_enum")]
    pub reason: Reason,
    pub detail: String,
    #[serde(deserialize_with = "crate::record::string_enum")]
    pub commitment: Commitment,
    #[serde(deserialize_with = "crate::record::nullable_string_enum")]
    pub stop: Option<Finish>,
    #[serde(deserialize_with = "crate::record::nullable_object")]
    pub http: Option<Http>,
    #[serde(deserialize_with = "crate::record::object")]
    pub body: Body,
    #[serde(rename = "final", deserialize_with = "crate::record::nullable_object")]
    pub final_artifact: Option<Artifact>,
    #[serde(deserialize_with = "crate::record::nullable_object")]
    pub usage: Option<Usage>,
    #[serde(deserialize_with = "crate::record::object")]
    pub timing: Timing,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RunEvidence {
    Collected,
    Refused,
    Uncommitted,
    Unresolved,
    NotStarted,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunCaseGrade {
    pub case_id: String,
    pub input: String,
    pub attempt: u32,
    pub terminal: Option<String>,
    #[serde(deserialize_with = "crate::record::nullable_string_enum")]
    pub reason: Option<Reason>,
    pub artifact: Option<String>,
    #[serde(deserialize_with = "crate::record::string_enum")]
    pub evidence: RunEvidence,
    #[serde(deserialize_with = "crate::record::string_enum")]
    pub outcome: Outcome,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunIdentities {
    #[serde(deserialize_with = "crate::record::object")]
    pub pack: PackIdentities,
    pub plan: String,
    pub protocol: String,
    pub system: String,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunView {
    pub version: u32,
    #[serde(deserialize_with = "crate::record::string_enum")]
    pub claim: RunViewClaim,
    #[serde(deserialize_with = "crate::record::object")]
    pub identities: RunIdentities,
    #[serde(deserialize_with = "crate::record::objects")]
    pub cases: Vec<RunCaseGrade>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RunViewClaim {
    ClientCollectedArtifactsNotAuthenticatedExecution,
}
