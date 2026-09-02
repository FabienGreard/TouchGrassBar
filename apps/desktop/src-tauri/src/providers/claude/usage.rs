use std::{
    collections::{BTreeMap, BTreeSet},
    env, fmt, fs,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{Duration as StdDuration, Instant},
};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{
    Deserialize, Deserializer,
    de::{IgnoredAny, MapAccess, SeqAccess, Visitor},
};
use time::{Date, Duration, OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};

use crate::daily_usage_aggregate::{
    DailyCostEvidence, DailyUsageEvidence, ProviderUsageEvidence, calculate_daily_usage_aggregates,
    calculate_usage_periods, checked_sum, period_days,
};
use crate::providers::ProviderCorrection;
use crate::sanitized::{
    ApiEquivalentCostQuality, TopModelUsage, UsageCoverage, UsagePeriods, UsageScanStatus,
    UsageTotal,
};

const MAX_TRANSCRIPT_LINE_BYTES: usize = 4 * 1024 * 1024;
const MAX_TRANSCRIPT_FILE_SCAN_BYTES: u64 = 256 * 1024 * 1024;
const MAX_TRANSCRIPT_SCAN_BYTES: u64 = 512 * 1024 * 1024;
const MAX_TRANSCRIPT_SCAN_MILLIS: u128 = 2_000;
const RESUME_ANCHOR_BYTES: u64 = 64 * 1024;
const TOKEN_HISTORY_RETENTION_DAYS: i64 = 60;
const COST_DETAIL_RETENTION_DAYS: i64 = 30;
const SUPPORTED_CLAUDE_CODE_VERSIONS: [&str; 3] = ["2.1.223", "2.1.224", "2.1.241"];
const MAX_SUPERSEDED_FRAMES: usize = 64;
const MAX_ASSISTANT_CONTENT_BLOCKS: usize = 4_096;
const MAX_CONTENT_METADATA_BYTES: usize = 128;
const MAX_UNKNOWN_USAGE_FIELDS: usize = 64;
const MAX_UNKNOWN_USAGE_KEY_BYTES: usize = 128;
const MAX_PRICING_BASIS_BYTES: usize = 256;
const INVALID_PRICING_MODIFIER: &str = "__invalid__";
const TRANSCRIPT_PARSER_VERSION: i64 = 7;
pub(crate) const USAGE_INDEX_SCHEMA_MODULE: &str = "claude-usage-index";
pub(crate) const USAGE_INDEX_SCHEMA_VERSION: i64 = 7;
const USAGE_AGGREGATE_PARSER_VERSION_KEY: &str = "usage_aggregate_parser_version";
const PARSER_CORRECTION_PROVENANCE: &str = "parser-correction";

#[derive(Clone, Copy)]
struct ScanBudget {
    max_bytes: u64,
    max_file_bytes: u64,
    max_millis: u128,
}

const DEFAULT_SCAN_BUDGET: ScanBudget = ScanBudget {
    max_bytes: MAX_TRANSCRIPT_SCAN_BYTES,
    max_file_bytes: MAX_TRANSCRIPT_FILE_SCAN_BYTES,
    max_millis: MAX_TRANSCRIPT_SCAN_MILLIS,
};

#[cfg(debug_assertions)]
fn debug_usage_event(event: &str) {
    eprintln!("[TouchGrassBar][claude-usage] {event}");
}

#[cfg(not(debug_assertions))]
fn debug_usage_event(_event: &str) {}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ClaudeTokenUsage {
    input: u64,
    cache_creation_input: u64,
    cache_read_input: u64,
    output: u64,
    cache_creation_5m_input: Option<u64>,
    cache_creation_1h_input: Option<u64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ClaudePricingMetadata {
    service_tier: Option<String>,
    inference_geo: Option<String>,
    speed: Option<String>,
    web_search_requests: Option<u64>,
    web_fetch_requests: Option<u64>,
    code_execution_requests: Option<u64>,
    has_unknown_paid_server_tool: bool,
}

impl ClaudeTokenUsage {
    fn observed_tokens(self) -> Option<u64> {
        self.input
            .checked_add(self.cache_creation_input)?
            .checked_add(self.cache_read_input)?
            .checked_add(self.output)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NormalizedFrame {
    frame_key: String,
    supersedes_frame_keys: Vec<String>,
    day: Date,
    observed_at: OffsetDateTime,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NormalizedMessage {
    frame_key: String,
    supersedes_frame_keys: Vec<String>,
    message_key: String,
    day: Date,
    observed_at: OffsetDateTime,
    model: String,
    usage: ClaudeTokenUsage,
    pricing: ClaudePricingMetadata,
    complete: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum TranscriptLineOutcome {
    Ignored,
    Invalid,
    FrameOnly(NormalizedFrame),
    Usage(Box<NormalizedMessage>),
}

fn parse_transcript_line(_line: &[u8], _dedupe_salt: &[u8; 32]) -> TranscriptLineOutcome {
    let Ok(header) = serde_json::from_slice::<RawTranscriptHeader>(_line) else {
        return TranscriptLineOutcome::Invalid;
    };
    if header.record_type != "assistant" {
        return TranscriptLineOutcome::Ignored;
    }
    let Ok(envelope) = serde_json::from_slice::<RawAssistantEnvelope>(_line) else {
        return TranscriptLineOutcome::Invalid;
    };
    if !supported_claude_code_version(&envelope.version) {
        return TranscriptLineOutcome::Invalid;
    }
    if envelope.record_type != "assistant"
        || !valid_provider_identifier(&envelope.uuid)
        || envelope.supersedes.len() > MAX_SUPERSEDED_FRAMES
        || envelope
            .supersedes
            .iter()
            .any(|value| !valid_provider_identifier(value))
    {
        return TranscriptLineOutcome::Invalid;
    }
    let Ok(observed_at) = OffsetDateTime::parse(&envelope.timestamp, &Rfc3339) else {
        return TranscriptLineOutcome::Invalid;
    };
    let frame = NormalizedFrame {
        frame_key: dedupe_key(&envelope.uuid, _dedupe_salt),
        supersedes_frame_keys: envelope
            .supersedes
            .iter()
            .map(|value| dedupe_key(value, _dedupe_salt))
            .collect(),
        day: observed_at.to_offset(UtcOffset::UTC).date(),
        observed_at,
    };
    let Ok(line) = serde_json::from_slice::<RawAssistantLine>(_line) else {
        return TranscriptLineOutcome::FrameOnly(frame);
    };
    if line.record_type != "assistant"
        || line.message.message_type != "message"
        || line.message.role != "assistant"
        || !valid_provider_identifier(&line.message.id)
    {
        return TranscriptLineOutcome::FrameOnly(frame);
    }
    if line.message.model == "<synthetic>" && is_reviewed_zero_usage_api_error(_line) {
        return TranscriptLineOutcome::Ignored;
    }
    if !valid_model_name(&line.message.model) {
        return TranscriptLineOutcome::FrameOnly(frame);
    }
    let cache_creation_known = line.message.usage.cache_creation_input_tokens.is_some();
    let cache_read_known = line.message.usage.cache_read_input_tokens.is_some();
    let reviewed_extended_usage = envelope.version == "2.1.241"
        && line
            .message
            .usage
            .iterations
            .as_ref()
            .is_none_or(|iterations| iterations.matches(&line.message.usage))
        && line
            .message
            .usage
            .output_tokens_details
            .as_ref()
            .is_none_or(|details| details.matches(line.message.usage.output_tokens));
    let usage_schema_known = line.message.usage.unknown.is_empty()
        && line.message.usage.fallback_credit.is_none()
        && (line.message.usage.iterations.is_none()
            && line.message.usage.output_tokens_details.is_none()
            || reviewed_extended_usage)
        && line
            .message
            .usage
            .cache_creation
            .as_ref()
            .is_none_or(|cache| cache.unknown.is_empty())
        && line
            .message
            .usage
            .server_tool_use
            .as_ref()
            .is_none_or(|tools| tools.unknown.is_empty());
    let mut usage = ClaudeTokenUsage {
        input: line.message.usage.input_tokens,
        cache_creation_input: line.message.usage.cache_creation_input_tokens.unwrap_or(0),
        cache_read_input: line.message.usage.cache_read_input_tokens.unwrap_or(0),
        output: line.message.usage.output_tokens,
        cache_creation_5m_input: line
            .message
            .usage
            .cache_creation
            .as_ref()
            .and_then(|cache| cache.ephemeral_5m_input_tokens),
        cache_creation_1h_input: line
            .message
            .usage
            .cache_creation
            .as_ref()
            .and_then(|cache| cache.ephemeral_1h_input_tokens),
    };
    if usage.observed_tokens().is_none() {
        return TranscriptLineOutcome::FrameOnly(frame);
    }
    let cache_breakdown = usage
        .cache_creation_5m_input
        .zip(usage.cache_creation_1h_input)
        .and_then(|(five_minutes, one_hour)| five_minutes.checked_add(one_hour));
    let cache_breakdown_matches = cache_breakdown == Some(usage.cache_creation_input);
    if cache_breakdown.is_some() && !cache_breakdown_matches {
        usage.cache_creation_5m_input = None;
        usage.cache_creation_1h_input = None;
    }
    let complete = cache_creation_known
        && cache_read_known
        && usage_schema_known
        && !line.aborted
        && (usage.cache_creation_input == 0 || cache_breakdown_matches);
    let pricing = ClaudePricingMetadata {
        service_tier: normalized_modifier(line.message.usage.service_tier),
        inference_geo: normalized_inference_geo(line.message.usage.inference_geo),
        speed: normalized_modifier(line.message.usage.speed),
        web_search_requests: line
            .message
            .usage
            .server_tool_use
            .as_ref()
            .map_or(Some(0), |tools| tools.web_search_requests),
        web_fetch_requests: line
            .message
            .usage
            .server_tool_use
            .as_ref()
            .map_or(Some(0), |tools| tools.web_fetch_requests),
        code_execution_requests: (line.message.content.code_execution_requests > 0)
            .then_some(line.message.content.code_execution_requests),
        has_unknown_paid_server_tool: line.message.usage.server_tool_use.as_ref().is_some_and(
            |tools| {
                tools.web_search_requests.is_none()
                    || tools.web_fetch_requests.is_none()
                    || !tools.unknown.is_empty()
            },
        ) || line.message.content.has_unknown_server_tool,
    };
    TranscriptLineOutcome::Usage(Box::new(NormalizedMessage {
        frame_key: frame.frame_key,
        supersedes_frame_keys: frame.supersedes_frame_keys,
        message_key: dedupe_key(&line.message.id, _dedupe_salt),
        day: frame.day,
        observed_at: frame.observed_at,
        model: line.message.model,
        usage,
        pricing,
        complete,
    }))
}

#[derive(Deserialize)]
struct RawTranscriptHeader {
    #[serde(rename = "type")]
    record_type: String,
}

#[derive(Deserialize)]
struct RawAssistantEnvelope {
    #[serde(rename = "type")]
    record_type: String,
    uuid: String,
    timestamp: String,
    version: String,
    #[serde(default)]
    supersedes: Vec<String>,
}

#[derive(Deserialize)]
struct RawAssistantLine {
    #[serde(rename = "type")]
    record_type: String,
    #[serde(default)]
    aborted: bool,
    message: RawAssistantMessage,
}

#[derive(Deserialize)]
struct RawAssistantMessage {
    id: String,
    #[serde(rename = "type")]
    message_type: String,
    role: String,
    model: String,
    content: AssistantContentMetadata,
    usage: RawClaudeTokenUsage,
}

#[derive(Deserialize)]
struct RawClaudeTokenUsage {
    input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
    output_tokens: u64,
    #[serde(default)]
    cache_creation: Option<RawCacheCreationUsage>,
    #[serde(default)]
    service_tier: Option<String>,
    #[serde(default)]
    inference_geo: Option<String>,
    #[serde(default)]
    speed: Option<String>,
    #[serde(default)]
    server_tool_use: Option<RawServerToolUsage>,
    #[allow(dead_code)]
    #[serde(default)]
    fallback_credit: Option<IgnoredAny>,
    #[serde(default)]
    iterations: Option<RawMessageIterations>,
    #[serde(default)]
    output_tokens_details: Option<RawOutputTokenDetails>,
    #[serde(flatten)]
    unknown: BoundedUnknownUsageFields,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawMessageIterations {
    Reviewed([ReviewedMessageIteration; 1]),
    Unreviewed(IgnoredAny),
}

impl RawMessageIterations {
    fn matches(&self, usage: &RawClaudeTokenUsage) -> bool {
        let Self::Reviewed([iteration]) = self else {
            return false;
        };
        iteration.matches(usage)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewedMessageIteration {
    cache_creation: ReviewedMessageIterationCacheCreation,
    cache_creation_input_tokens: u64,
    cache_read_input_tokens: u64,
    input_tokens: u64,
    output_tokens: u64,
    #[serde(rename = "type")]
    message_type: String,
}

impl ReviewedMessageIteration {
    fn matches(&self, usage: &RawClaudeTokenUsage) -> bool {
        self.message_type == "message"
            && self.input_tokens == usage.input_tokens
            && Some(self.cache_creation_input_tokens) == usage.cache_creation_input_tokens
            && Some(self.cache_read_input_tokens) == usage.cache_read_input_tokens
            && self.output_tokens == usage.output_tokens
            && usage
                .cache_creation
                .as_ref()
                .is_some_and(|cache| self.cache_creation.matches(cache))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewedMessageIterationCacheCreation {
    ephemeral_1h_input_tokens: u64,
    ephemeral_5m_input_tokens: u64,
}

impl ReviewedMessageIterationCacheCreation {
    fn matches(&self, usage: &RawCacheCreationUsage) -> bool {
        usage.unknown.is_empty()
            && Some(self.ephemeral_1h_input_tokens) == usage.ephemeral_1h_input_tokens
            && Some(self.ephemeral_5m_input_tokens) == usage.ephemeral_5m_input_tokens
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawOutputTokenDetails {
    Reviewed(ReviewedOutputTokenDetails),
    Unreviewed(IgnoredAny),
}

impl RawOutputTokenDetails {
    fn matches(&self, output_tokens: u64) -> bool {
        matches!(
            self,
            Self::Reviewed(details) if details.thinking_tokens <= output_tokens
        )
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewedOutputTokenDetails {
    thinking_tokens: u64,
}

#[derive(Default)]
struct BoundedUnknownUsageFields {
    fields: BTreeMap<String, IgnoredAny>,
    overflowed: bool,
}

impl BoundedUnknownUsageFields {
    fn is_empty(&self) -> bool {
        self.fields.is_empty() && !self.overflowed
    }
}

impl<'de> Deserialize<'de> for BoundedUnknownUsageFields {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(BoundedUnknownUsageFieldsVisitor)
    }
}

struct BoundedUnknownUsageFieldsVisitor;

impl<'de> Visitor<'de> for BoundedUnknownUsageFieldsVisitor {
    type Value = BoundedUnknownUsageFields;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bounded map of unknown usage metadata")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut unknown = BoundedUnknownUsageFields::default();
        while let Some(key) = map.next_key::<String>()? {
            let value = map.next_value::<IgnoredAny>()?;
            if key.len() > MAX_UNKNOWN_USAGE_KEY_BYTES
                || unknown.fields.len() >= MAX_UNKNOWN_USAGE_FIELDS
            {
                unknown.overflowed = true;
                continue;
            }
            if unknown.fields.insert(key, value).is_some() {
                unknown.overflowed = true;
            }
        }
        Ok(unknown)
    }
}

#[derive(Deserialize)]
struct RawCacheCreationUsage {
    #[serde(default)]
    ephemeral_5m_input_tokens: Option<u64>,
    #[serde(default)]
    ephemeral_1h_input_tokens: Option<u64>,
    #[serde(flatten)]
    unknown: BoundedUnknownUsageFields,
}

#[derive(Deserialize)]
struct RawServerToolUsage {
    #[serde(default)]
    web_search_requests: Option<u64>,
    #[serde(default)]
    web_fetch_requests: Option<u64>,
    #[serde(flatten)]
    unknown: BoundedUnknownUsageFields,
}

#[derive(Clone, Copy, Default)]
struct DiscardedString {
    non_empty: bool,
}

impl DiscardedString {
    fn is_non_empty(self) -> bool {
        self.non_empty
    }
}

impl<'de> Deserialize<'de> for DiscardedString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_string(DiscardedStringVisitor)
    }
}

struct DiscardedStringVisitor;

impl Visitor<'_> for DiscardedStringVisitor {
    type Value = DiscardedString;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a string that is discarded after validation")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(DiscardedString {
            non_empty: !value.is_empty(),
        })
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        self.visit_str(&value)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewedApiErrorEnvelope {
    cwd: DiscardedString,
    entrypoint: DiscardedString,
    error: DiscardedString,
    #[serde(rename = "gitBranch")]
    git_branch: DiscardedString,
    #[serde(rename = "isApiErrorMessage")]
    is_api_error_message: bool,
    #[serde(rename = "isSidechain")]
    is_sidechain: bool,
    message: ReviewedApiErrorMessage,
    #[serde(rename = "parentUuid")]
    parent_uuid: DiscardedString,
    #[serde(rename = "sessionId")]
    session_id_camel: DiscardedString,
    session_id: DiscardedString,
    timestamp: DiscardedString,
    #[serde(rename = "type")]
    record_type: String,
    #[serde(rename = "userType")]
    user_type: DiscardedString,
    uuid: DiscardedString,
    version: String,
}

impl ReviewedApiErrorEnvelope {
    fn is_reviewed(&self) -> bool {
        [
            self.cwd,
            self.entrypoint,
            self.error,
            self.git_branch,
            self.parent_uuid,
            self.session_id_camel,
            self.session_id,
            self.timestamp,
            self.user_type,
            self.uuid,
        ]
        .into_iter()
        .all(DiscardedString::is_non_empty)
            && self.is_api_error_message
            && !self.is_sidechain
            && self.record_type == "assistant"
            && self.version == "2.1.241"
            && self.message.is_reviewed()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewedApiErrorMessage {
    #[serde(rename = "container")]
    _container: (),
    content: [ReviewedApiErrorTextBlock; 1],
    #[serde(rename = "context_management")]
    _context_management: (),
    #[serde(rename = "diagnostics")]
    _diagnostics: (),
    id: DiscardedString,
    model: String,
    role: String,
    #[serde(rename = "stop_details")]
    _stop_details: (),
    stop_reason: String,
    stop_sequence: String,
    #[serde(rename = "type")]
    message_type: String,
    usage: ReviewedApiErrorUsage,
}

impl ReviewedApiErrorMessage {
    fn is_reviewed(&self) -> bool {
        self.id.is_non_empty()
            && self.model == "<synthetic>"
            && self.role == "assistant"
            && self.stop_reason == "stop_sequence"
            && self.stop_sequence.is_empty()
            && self.message_type == "message"
            && self.content[0].is_reviewed()
            && self.usage.is_zero()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewedApiErrorTextBlock {
    text: DiscardedString,
    #[serde(rename = "type")]
    block_type: String,
}

impl ReviewedApiErrorTextBlock {
    fn is_reviewed(&self) -> bool {
        self.text.is_non_empty() && self.block_type == "text"
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewedApiErrorUsage {
    cache_creation: ReviewedApiErrorCacheCreation,
    cache_creation_input_tokens: u64,
    cache_read_input_tokens: u64,
    #[serde(rename = "inference_geo")]
    _inference_geo: (),
    input_tokens: u64,
    #[serde(rename = "iterations")]
    _iterations: (),
    output_tokens: u64,
    #[serde(rename = "output_tokens_details")]
    _output_tokens_details: (),
    server_tool_use: ReviewedApiErrorServerToolUse,
    #[serde(rename = "service_tier")]
    _service_tier: (),
    #[serde(rename = "speed")]
    _speed: (),
}

impl ReviewedApiErrorUsage {
    fn is_zero(&self) -> bool {
        self.cache_creation_input_tokens == 0
            && self.cache_read_input_tokens == 0
            && self.input_tokens == 0
            && self.output_tokens == 0
            && self.cache_creation.is_zero()
            && self.server_tool_use.is_zero()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewedApiErrorCacheCreation {
    ephemeral_1h_input_tokens: u64,
    ephemeral_5m_input_tokens: u64,
}

impl ReviewedApiErrorCacheCreation {
    fn is_zero(&self) -> bool {
        self.ephemeral_1h_input_tokens == 0 && self.ephemeral_5m_input_tokens == 0
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewedApiErrorServerToolUse {
    web_fetch_requests: u64,
    web_search_requests: u64,
}

impl ReviewedApiErrorServerToolUse {
    fn is_zero(&self) -> bool {
        self.web_fetch_requests == 0 && self.web_search_requests == 0
    }
}

fn is_reviewed_zero_usage_api_error(line: &[u8]) -> bool {
    serde_json::from_slice::<ReviewedApiErrorEnvelope>(line)
        .is_ok_and(|envelope| envelope.is_reviewed())
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct AssistantContentMetadata {
    code_execution_requests: u64,
    has_unknown_server_tool: bool,
}

impl AssistantContentMetadata {
    fn observe(&mut self, block: RawAssistantContentBlock) {
        if block.block_type.as_deref() != Some("server_tool_use") {
            return;
        }
        let Some(name) = block
            .name
            .filter(|value| value.len() <= MAX_CONTENT_METADATA_BYTES)
        else {
            self.has_unknown_server_tool = true;
            return;
        };
        match name.as_str() {
            "code_execution" | "bash_code_execution" | "text_editor_code_execution" => {
                self.code_execution_requests = self.code_execution_requests.saturating_add(1);
            }
            "web_search" | "web_fetch" => {}
            _ => self.has_unknown_server_tool = true,
        }
    }
}

impl<'de> Deserialize<'de> for AssistantContentMetadata {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(AssistantContentVisitor)
    }
}

struct AssistantContentVisitor;

impl<'de> Visitor<'de> for AssistantContentVisitor {
    type Value = AssistantContentMetadata;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bounded list of assistant content blocks")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut metadata = AssistantContentMetadata::default();
        for _ in 0..MAX_ASSISTANT_CONTENT_BLOCKS {
            let Some(block) = sequence.next_element::<RawAssistantContentBlock>()? else {
                return Ok(metadata);
            };
            metadata.observe(block);
        }
        if sequence.next_element::<IgnoredAny>()?.is_some() {
            metadata.has_unknown_server_tool = true;
            while sequence.next_element::<IgnoredAny>()?.is_some() {}
        }
        Ok(metadata)
    }
}

#[derive(Deserialize)]
struct RawAssistantContentBlock {
    #[serde(default, rename = "type")]
    block_type: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

fn normalized_modifier(value: Option<String>) -> Option<String> {
    match value {
        None => None,
        Some(value)
            if !value.is_empty()
                && value.len() <= 32
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || matches!(byte, b'-' | b'_')) =>
        {
            Some(value)
        }
        Some(_) => Some(INVALID_PRICING_MODIFIER.to_owned()),
    }
}

fn normalized_inference_geo(value: Option<String>) -> Option<String> {
    match value.as_deref() {
        Some("") => None,
        _ => normalized_modifier(value),
    }
}

fn supported_claude_code_version(value: &str) -> bool {
    SUPPORTED_CLAUDE_CODE_VERSIONS.contains(&value)
}

fn valid_provider_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn valid_model_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn dedupe_key(provider_identifier: &str, salt: &[u8; 32]) -> String {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let hashes = [
        0x243f6a8885a308d3,
        0x13198a2e03707344,
        0xa4093822299f31d0,
        0x082efa98ec4e6c89,
    ]
    .map(|domain| {
        salt.iter()
            .copied()
            .chain(provider_identifier.bytes())
            .fold(OFFSET ^ domain, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(PRIME)
            })
    });
    format!(
        "tg-dedupe-v1:{:016x}{:016x}{:016x}{:016x}",
        hashes[0], hashes[1], hashes[2], hashes[3]
    )
}

fn byte_fingerprint(bytes: &[u8]) -> String {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let hashes = [
        0x243f6a8885a308d3,
        0x13198a2e03707344,
        0xa4093822299f31d0,
        0x082efa98ec4e6c89,
    ]
    .map(|domain| {
        bytes.iter().copied().fold(OFFSET ^ domain, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(PRIME)
        })
    });
    format!(
        "tg-anchor-v1:{:016x}{:016x}{:016x}{:016x}",
        hashes[0], hashes[1], hashes[2], hashes[3]
    )
}

fn composite_pricing_basis(pricing_bases: BTreeSet<String>) -> Option<String> {
    if pricing_bases.is_empty() {
        return None;
    }
    let basis_count = pricing_bases.len();
    let canonical = pricing_bases.into_iter().collect::<Vec<_>>().join(" + ");
    if canonical.len() <= MAX_PRICING_BASIS_BYTES {
        return Some(canonical);
    }
    let bounded = format!(
        "composite-v1:{basis_count}:{}",
        byte_fingerprint(canonical.as_bytes())
    );
    debug_assert!(bounded.len() <= MAX_PRICING_BASIS_BYTES);
    Some(bounded)
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct LocalUsageObservation {
    daily_usage: BTreeMap<Date, DailyUsageEvidence>,
    daily_cost: BTreeMap<Date, DailyCostEvidence>,
    pub(super) top_model_usage: Option<TopModelUsage>,
    pricing_basis: Option<String>,
    scan_status: UsageScanStatus,
    latest_pending_modified_at: Option<OffsetDateTime>,
    latest_error_modified_at: Option<OffsetDateTime>,
    scan_scope_known: bool,
    transcript_source_present: bool,
    aggregate_changed: bool,
    daily_corrections: BTreeMap<Date, ProviderCorrection>,
    pub(super) correction: Option<ProviderCorrection>,
}

impl LocalUsageObservation {
    fn period_scan_status(&self, today: Date, length: i64) -> UsageScanStatus {
        let period_start = (today - Duration::days(length - 1)).midnight().assume_utc();
        if !self.scan_scope_known {
            return self.scan_status;
        }
        if self
            .latest_pending_modified_at
            .is_some_and(|modified_at| modified_at >= period_start)
        {
            return UsageScanStatus::Indexing;
        }
        if self
            .latest_error_modified_at
            .is_some_and(|modified_at| modified_at >= period_start)
        {
            return UsageScanStatus::Unavailable;
        }
        UsageScanStatus::Complete
    }
}

pub(super) fn project_usage_periods(
    local: Option<&LocalUsageObservation>,
    now: OffsetDateTime,
) -> UsagePeriods {
    let evidence = provider_usage_evidence(local, now);
    calculate_usage_periods(&evidence, now)
}

fn provider_usage_evidence(
    local: Option<&LocalUsageObservation>,
    now: OffsetDateTime,
) -> ProviderUsageEvidence {
    let today = utc_ranking_day(now);
    ProviderUsageEvidence {
        provider_reported_tokens: None,
        provider_observed_at: None,
        provider_observed_at_by_day: BTreeMap::new(),
        local_usage_evidence: local.map_or_else(BTreeMap::new, |local| local.daily_usage.clone()),
        local_cost_evidence: local.map_or_else(BTreeMap::new, |local| local.daily_cost.clone()),
        local_evidence_available: local.is_some(),
        local_observed_at: local.map(|_| now),
        pricing_basis: local.and_then(|local| local.pricing_basis.clone()),
        scan_status: local.map_or(UsageScanStatus::Unavailable, |local| local.scan_status),
        today_scan_status: local.map_or(UsageScanStatus::Unavailable, |local| {
            local.period_scan_status(today, 1)
        }),
        seven_day_scan_status: local.map_or(UsageScanStatus::Unavailable, |local| {
            local.period_scan_status(today, 7)
        }),
        thirty_day_scan_status: local.map_or(UsageScanStatus::Unavailable, |local| {
            local.period_scan_status(today, 30)
        }),
    }
}

pub(crate) fn load_daily_usage_history(
    connection: &Connection,
    now: OffsetDateTime,
    anchor_day: Date,
    length: i64,
) -> Result<Vec<(Date, UsageTotal, Option<ProviderCorrection>)>, ()> {
    if !(1..=60).contains(&length) {
        return Err(());
    }
    let cutoff = anchor_day
        .checked_sub(Duration::days(length - 1))
        .ok_or(())?;
    let local = read_indexed_usage(
        connection,
        cutoff,
        anchor_day,
        UsageScanStatus::Complete,
        true,
        true,
        false,
    )?;
    let evidence = provider_usage_evidence(Some(&local), now);
    Ok(calculate_daily_usage_aggregates(
        &evidence,
        anchor_day.midnight().assume_utc(),
        anchor_day,
        length,
    )
    .into_iter()
    .map(|(day, total)| {
        let correction = local.daily_corrections.get(&day).copied();
        (day, total, correction)
    })
    .collect())
}

pub(super) fn scan_local_usage(
    database_path: Option<&Path>,
    probe_directory: Option<&Path>,
    now: OffsetDateTime,
) -> Option<LocalUsageObservation> {
    scan_local_usage_at(
        database_path?,
        &claude_config_root()?,
        probe_directory.unwrap_or_else(|| Path::new("")),
        now,
    )
}

fn scan_local_usage_at(
    database_path: &Path,
    config_root: &Path,
    probe_directory: &Path,
    now: OffsetDateTime,
) -> Option<LocalUsageObservation> {
    let observation = index_local_usage_at(database_path, config_root, probe_directory, now)?;
    (observation.transcript_source_present
        && (observation.scan_status == UsageScanStatus::Complete || observation.aggregate_changed))
        .then_some(observation)
}

fn claude_config_root() -> Option<PathBuf> {
    let home = env::var_os("HOME").map(PathBuf::from)?;
    let configured = env::var_os("CLAUDE_CONFIG_DIR").filter(|value| !value.is_empty());
    Some(match configured {
        Some(value) => {
            let path = PathBuf::from(value);
            if path.is_absolute() {
                path
            } else {
                home.join(path)
            }
        }
        None => home.join(".claude"),
    })
}

fn utc_ranking_day(timestamp: OffsetDateTime) -> Date {
    timestamp.to_offset(UtcOffset::UTC).date()
}

fn parse_ranking_day(value: &str) -> Result<Date, ()> {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return Err(());
    }
    let year = value.get(0..4).ok_or(())?.parse::<i32>().map_err(|_| ())?;
    let month = value.get(5..7).ok_or(())?.parse::<u8>().map_err(|_| ())?;
    let day = value.get(8..10).ok_or(())?.parse::<u8>().map_err(|_| ())?;
    Date::from_calendar_date(year, time::Month::try_from(month).map_err(|_| ())?, day)
        .map_err(|_| ())
}

fn to_i64(value: u64) -> Result<i64, ()> {
    i64::try_from(value).map_err(|_| ())
}

fn from_i64(value: i64) -> Result<u64, ()> {
    u64::try_from(value).map_err(|_| ())
}

pub(crate) fn usage_index_schema_version(connection: &Connection) -> Result<i64, ()> {
    let exists = connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM sqlite_master
               WHERE type = 'table' AND name = 'touchgrassbar_schema_versions'
             )",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|_| ())?;
    if !exists {
        return Ok(0);
    }
    connection
        .query_row(
            "SELECT version FROM touchgrassbar_schema_versions WHERE module = ?1",
            [USAGE_INDEX_SCHEMA_MODULE],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map(|version| version.unwrap_or(0))
        .map_err(|_| ())
}

fn daily_usage_optional_columns(connection: &Connection) -> Result<(bool, bool, bool), ()> {
    let mut statement = connection
        .prepare("PRAGMA table_info(claude_usage_daily)")
        .map_err(|_| ())?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|_| ())?;
    let mut has_provenance = false;
    let mut has_source_revision = false;
    let mut has_cost_modeled = false;
    for column in columns {
        match column.map_err(|_| ())?.as_str() {
            "correction_provenance" => has_provenance = true,
            "correction_source_revision" => has_source_revision = true,
            "cost_modeled" => has_cost_modeled = true,
            _ => {}
        }
    }
    Ok((has_provenance, has_source_revision, has_cost_modeled))
}

fn supersede_edges_have_aggregate_applied(connection: &Connection) -> Result<bool, ()> {
    let mut statement = connection
        .prepare("PRAGMA table_info(claude_usage_message_supersedes)")
        .map_err(|_| ())?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|_| ())?;
    for column in columns {
        if column.map_err(|_| ())? == "aggregate_applied" {
            return Ok(true);
        }
    }
    Ok(false)
}

fn usage_index_backup_path(path: &Path, source_version: i64) -> PathBuf {
    path.with_extension(format!("sqlite3.claude-usage-v{source_version}.backup"))
}

fn usage_index_backup_partial_path(path: &Path, source_version: i64) -> PathBuf {
    path.with_extension(format!(
        "sqlite3.claude-usage-v{source_version}.backup.partial"
    ))
}

fn usage_index_backup_is_valid(connection: &Connection, source_version: i64) -> Result<bool, ()> {
    let integrity = connection
        .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
        .map_err(|_| ())?;
    Ok(integrity == "ok" && usage_index_schema_version(connection)? == source_version)
}

fn backup_usage_index_before_migration(
    connection: &Connection,
    path: &Path,
    source_version: i64,
) -> Result<(), ()> {
    let backup_path = usage_index_backup_path(path, source_version);
    if backup_path.exists() {
        let valid =
            Connection::open_with_flags(&backup_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .ok()
                .and_then(|backup| usage_index_backup_is_valid(&backup, source_version).ok())
                .unwrap_or(false);
        if valid {
            return Ok(());
        }
        fs::remove_file(&backup_path).map_err(|_| ())?;
    }
    let partial_path = usage_index_backup_partial_path(path, source_version);
    if partial_path.exists() {
        fs::remove_file(&partial_path).map_err(|_| ())?;
    }
    connection
        .backup(rusqlite::MAIN_DB, &partial_path, None)
        .map_err(|_| ())?;
    let backup =
        Connection::open_with_flags(&partial_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| ())?;
    if !usage_index_backup_is_valid(&backup, source_version)? {
        return Err(());
    }
    drop(backup);
    fs::File::open(&partial_path)
        .and_then(|file| file.sync_all())
        .map_err(|_| ())?;
    fs::rename(&partial_path, &backup_path).map_err(|_| ())?;
    let parent = backup_path.parent().ok_or(())?;
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| ())
}

fn ensure_index_schema(connection: &mut Connection, database_path: &Path) -> Result<(), ()> {
    connection
        .busy_timeout(StdDuration::from_secs(2))
        .map_err(|_| ())?;
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|_| ())?;
    let source_version = usage_index_schema_version(connection)?;
    if source_version > USAGE_INDEX_SCHEMA_VERSION {
        return Err(());
    }
    if source_version == USAGE_INDEX_SCHEMA_VERSION {
        return Ok(());
    }
    backup_usage_index_before_migration(connection, database_path, source_version)?;
    let transaction = connection.transaction().map_err(|_| ())?;
    transaction
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS touchgrassbar_schema_versions (
               module TEXT PRIMARY KEY,
               version INTEGER NOT NULL CHECK (version >= 1)
             );
             CREATE TABLE IF NOT EXISTS claude_usage_index_meta (
               key TEXT PRIMARY KEY NOT NULL,
               value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS claude_usage_files (
               path TEXT PRIMARY KEY NOT NULL,
               file_identity TEXT NOT NULL,
               size_bytes INTEGER NOT NULL,
               modified_ns INTEGER NOT NULL,
               parsed_offset INTEGER NOT NULL,
               resume_anchor TEXT,
               parser_version INTEGER NOT NULL,
               completion_state TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS claude_usage_messages (
               frame_key TEXT PRIMARY KEY NOT NULL,
               supersedes_frame_key TEXT,
               message_key TEXT NOT NULL,
               day TEXT NOT NULL,
               observed_at TEXT NOT NULL,
               model TEXT NOT NULL,
               input_tokens INTEGER NOT NULL,
               cache_creation_input_tokens INTEGER NOT NULL,
               cache_read_input_tokens INTEGER NOT NULL,
               output_tokens INTEGER NOT NULL,
               cache_creation_5m_input_tokens INTEGER,
               cache_creation_1h_input_tokens INTEGER,
               service_tier TEXT,
               inference_geo TEXT,
               speed TEXT,
               web_search_requests INTEGER,
               web_fetch_requests INTEGER,
               code_execution_requests INTEGER,
               has_unknown_paid_server_tool INTEGER NOT NULL,
               observed_tokens INTEGER NOT NULL,
               complete INTEGER NOT NULL,
               parser_version INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS claude_usage_messages_by_day
               ON claude_usage_messages(day);
             CREATE INDEX IF NOT EXISTS claude_usage_messages_by_message
               ON claude_usage_messages(message_key);
             CREATE INDEX IF NOT EXISTS claude_usage_messages_by_superseded_frame
               ON claude_usage_messages(supersedes_frame_key);
             CREATE TABLE IF NOT EXISTS claude_usage_frames (
               frame_key TEXT PRIMARY KEY NOT NULL,
               day TEXT NOT NULL,
               observed_at TEXT NOT NULL,
               parser_version INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS claude_usage_frames_by_day
               ON claude_usage_frames(day);
             INSERT OR IGNORE INTO claude_usage_frames(
               frame_key, day, observed_at, parser_version
             )
             SELECT frame_key, day, observed_at, parser_version
             FROM claude_usage_messages;
             CREATE TABLE IF NOT EXISTS claude_usage_message_supersedes (
               replacement_frame_key TEXT NOT NULL,
               superseded_frame_key TEXT NOT NULL,
               parser_version INTEGER NOT NULL,
               PRIMARY KEY(replacement_frame_key, superseded_frame_key),
               FOREIGN KEY(replacement_frame_key)
                 REFERENCES claude_usage_messages(frame_key) ON DELETE CASCADE
             );
             CREATE INDEX IF NOT EXISTS claude_usage_supersedes_by_superseded_frame
               ON claude_usage_message_supersedes(superseded_frame_key, parser_version);
             INSERT OR IGNORE INTO claude_usage_message_supersedes(
               replacement_frame_key, superseded_frame_key, parser_version
             )
             SELECT frame_key, supersedes_frame_key, parser_version
             FROM claude_usage_messages
             WHERE supersedes_frame_key IS NOT NULL;
             DROP INDEX IF EXISTS claude_usage_supersedes_by_superseded_frame;
             DROP TABLE IF EXISTS claude_usage_message_supersedes_next;
             CREATE TABLE claude_usage_message_supersedes_next (
               replacement_frame_key TEXT NOT NULL,
               superseded_frame_key TEXT NOT NULL,
               parser_version INTEGER NOT NULL,
               PRIMARY KEY(replacement_frame_key, superseded_frame_key),
               FOREIGN KEY(replacement_frame_key)
                 REFERENCES claude_usage_frames(frame_key) ON DELETE CASCADE
             );
             INSERT OR IGNORE INTO claude_usage_message_supersedes_next(
               replacement_frame_key, superseded_frame_key, parser_version
             )
             SELECT replacement_frame_key, superseded_frame_key, parser_version
             FROM claude_usage_message_supersedes;
             DROP TABLE claude_usage_message_supersedes;
             ALTER TABLE claude_usage_message_supersedes_next
               RENAME TO claude_usage_message_supersedes;
             CREATE INDEX claude_usage_supersedes_by_superseded_frame
               ON claude_usage_message_supersedes(superseded_frame_key, parser_version);
             CREATE TABLE IF NOT EXISTS claude_usage_daily (
               day TEXT PRIMARY KEY NOT NULL,
               observed_tokens INTEGER NOT NULL,
               coverage TEXT NOT NULL CHECK (coverage IN ('complete', 'partial')),
               observed_through TEXT NOT NULL,
               revision INTEGER NOT NULL CHECK (revision >= 1),
               priced_tokens INTEGER NOT NULL DEFAULT 0,
               cost_usd REAL,
               cost_modeled INTEGER NOT NULL DEFAULT 0 CHECK (cost_modeled IN (0, 1)),
               pricing_basis TEXT,
               pricing_fingerprint TEXT,
               correction_provenance TEXT,
               correction_source_revision INTEGER,
               CHECK (
                 (
                   correction_provenance IS NULL
                   AND correction_source_revision IS NULL
                 ) OR (
                   correction_provenance = 'parser-correction'
                   AND correction_source_revision >= 1
                   AND correction_source_revision <= revision
                 )
               )
             );",
        )
        .map_err(|_| ())?;
    let (has_provenance, has_source_revision, has_cost_modeled) =
        daily_usage_optional_columns(&transaction)?;
    if !has_provenance {
        transaction
            .execute_batch(
                "ALTER TABLE claude_usage_daily
                 ADD COLUMN correction_provenance TEXT CHECK (
                   correction_provenance IS NULL
                   OR correction_provenance = 'parser-correction'
                 );",
            )
            .map_err(|_| ())?;
    }
    if !has_source_revision {
        // A legacy marker has no exact source revision. Clear it instead of
        // deriving provenance from the current aggregate revision.
        transaction
            .execute(
                "UPDATE claude_usage_daily SET correction_provenance = NULL",
                [],
            )
            .map_err(|_| ())?;
        transaction
            .execute_batch(
                "ALTER TABLE claude_usage_daily
                 ADD COLUMN correction_source_revision INTEGER CHECK (
                   (
                     correction_provenance IS NULL
                     AND correction_source_revision IS NULL
                   ) OR (
                     correction_provenance = 'parser-correction'
                     AND correction_source_revision >= 1
                     AND correction_source_revision <= revision
                   )
                 );",
            )
            .map_err(|_| ())?;
    }
    if !has_cost_modeled {
        transaction
            .execute_batch(
                "ALTER TABLE claude_usage_daily
                 ADD COLUMN cost_modeled INTEGER NOT NULL DEFAULT 0
                 CHECK (cost_modeled IN (0, 1));",
            )
            .map_err(|_| ())?;
    }
    if !supersede_edges_have_aggregate_applied(&transaction)? {
        // Existing edges predate this proof lifecycle. Mark them as consumed
        // so migration cannot create new correction authority.
        transaction
            .execute_batch(
                "ALTER TABLE claude_usage_message_supersedes
                 ADD COLUMN aggregate_applied INTEGER NOT NULL DEFAULT 1
                 CHECK (aggregate_applied IN (0, 1));",
            )
            .map_err(|_| ())?;
    }
    transaction
        .execute(
            "INSERT INTO touchgrassbar_schema_versions(module, version) VALUES(?1, ?2)
             ON CONFLICT(module) DO UPDATE SET version = excluded.version",
            params![USAGE_INDEX_SCHEMA_MODULE, USAGE_INDEX_SCHEMA_VERSION],
        )
        .map_err(|_| ())?;
    transaction.commit().map_err(|_| ())
}

pub(crate) fn prepare_database(database_path: &Path) -> Result<(), ()> {
    let mut connection = Connection::open(database_path).map_err(|_| ())?;
    ensure_index_schema(&mut connection, database_path)
}

fn salt_to_string(salt: &[u8; 32]) -> String {
    salt.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn salt_from_string(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut salt = [0_u8; 32];
    for (index, byte) in salt.iter_mut().enumerate() {
        *byte = u8::from_str_radix(value.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(salt)
}

fn load_or_create_dedupe_salt(connection: &Connection) -> Result<[u8; 32], ()> {
    let stored = connection
        .query_row(
            "SELECT value FROM claude_usage_index_meta WHERE key = 'dedupe_salt_v1'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| ())?;
    if let Some(stored) = stored {
        return salt_from_string(&stored).ok_or(());
    }
    let mut salt = [0_u8; 32];
    getrandom::fill(&mut salt).map_err(|_| ())?;
    connection
        .execute(
            "INSERT INTO claude_usage_index_meta(key, value) VALUES('dedupe_salt_v1', ?1)",
            [salt_to_string(&salt)],
        )
        .map_err(|_| ())?;
    Ok(salt)
}

#[derive(Clone, Debug)]
struct StoredFileSummary {
    identity: String,
    size: u64,
    modified_ns: i64,
    parsed_offset: u64,
    resume_anchor: Option<String>,
    parser_version: i64,
    completion_state: String,
}

impl StoredFileSummary {
    fn needs_work(&self, identity: &str, size: u64, modified_ns: i64) -> bool {
        self.identity != identity
            || self.size != size
            || self.modified_ns != modified_ns
            || self.parser_version != TRANSCRIPT_PARSER_VERSION
            || self.completion_state != "complete"
    }
}

fn load_file_summaries(connection: &Connection) -> Result<BTreeMap<String, StoredFileSummary>, ()> {
    connection
        .prepare(
            "SELECT path, file_identity, size_bytes, modified_ns, parsed_offset,
                    resume_anchor, parser_version, completion_state
             FROM claude_usage_files",
        )
        .map_err(|_| ())?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                StoredFileSummary {
                    identity: row.get(1)?,
                    size: from_i64(row.get(2)?).map_err(|_| rusqlite::Error::InvalidQuery)?,
                    modified_ns: row.get(3)?,
                    parsed_offset: from_i64(row.get(4)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    resume_anchor: row.get(5)?,
                    parser_version: row.get(6)?,
                    completion_state: row.get(7)?,
                },
            ))
        })
        .map_err(|_| ())?
        .collect::<Result<BTreeMap<_, _>, _>>()
        .map_err(|_| ())
}

fn file_modified_ns(metadata: &fs::Metadata) -> Result<i64, ()> {
    let duration = metadata
        .modified()
        .map_err(|_| ())?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| ())?;
    i64::try_from(duration.as_nanos()).map_err(|_| ())
}

#[cfg(unix)]
fn file_identity(metadata: &fs::Metadata) -> String {
    use std::os::unix::fs::MetadataExt;
    format!("{}:{}", metadata.dev(), metadata.ino())
}

#[cfg(not(unix))]
fn file_identity(metadata: &fs::Metadata) -> String {
    format!("{}", metadata.len())
}

fn file_resume_anchor(path: &Path, indexed_bytes: u64) -> Result<Option<String>, ()> {
    if indexed_bytes == 0 {
        return Ok(None);
    }
    let sample_length = RESUME_ANCHOR_BYTES.min(indexed_bytes);
    let starts = BTreeSet::from([
        0,
        indexed_bytes / 4,
        indexed_bytes / 2,
        indexed_bytes.saturating_mul(3) / 4,
        indexed_bytes.saturating_sub(sample_length),
    ])
    .into_iter()
    .map(|start| start.min(indexed_bytes.saturating_sub(sample_length)))
    .collect::<BTreeSet<_>>();
    let mut file = fs::File::open(path).map_err(|_| ())?;
    let mut samples = Vec::new();
    for start in starts {
        file.seek(SeekFrom::Start(start)).map_err(|_| ())?;
        let length = sample_length.min(indexed_bytes.saturating_sub(start));
        let mut sample = vec![0_u8; usize::try_from(length).map_err(|_| ())?];
        file.read_exact(&mut sample).map_err(|_| ())?;
        samples.extend_from_slice(&start.to_le_bytes());
        samples.extend_from_slice(&length.to_le_bytes());
        samples.extend_from_slice(&sample);
    }
    Ok(Some(format!(
        "{indexed_bytes}:{}",
        byte_fingerprint(&samples)
    )))
}

fn collect_transcript_files(
    root: &Path,
    files: &mut Vec<PathBuf>,
    started: Instant,
    max_millis: u128,
) -> Result<(), ()> {
    let mut pending = vec![(root.to_path_buf(), 0_u8)];
    while let Some((directory, depth)) = pending.pop() {
        if depth > 32 || files.len() >= 100_000 || started.elapsed().as_millis() >= max_millis {
            return Err(());
        }
        let entries = fs::read_dir(directory).map_err(|_| ())?;
        for entry in entries {
            if started.elapsed().as_millis() >= max_millis {
                return Err(());
            }
            let entry = entry.map_err(|_| ())?;
            let file_type = entry.file_type().map_err(|_| ())?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                pending.push((entry.path(), depth.saturating_add(1)));
            } else if file_type.is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "jsonl")
            {
                files.push(entry.path());
            }
        }
    }
    Ok(())
}

#[derive(Clone)]
struct StoredMessage {
    day: Date,
    observed_at: OffsetDateTime,
    model: String,
    usage: ClaudeTokenUsage,
    pricing: ClaudePricingMetadata,
    observed_tokens: u64,
    details_retained: bool,
    complete: bool,
}

fn store_frame(transaction: &rusqlite::Transaction<'_>, frame: &NormalizedFrame) -> Result<(), ()> {
    let changed = transaction
        .execute(
            "INSERT INTO claude_usage_frames(frame_key, day, observed_at, parser_version)
             VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(frame_key) DO UPDATE SET
               day=excluded.day,
               observed_at=excluded.observed_at,
               parser_version=excluded.parser_version
             WHERE claude_usage_frames.parser_version != excluded.parser_version",
            params![
                frame.frame_key,
                frame.day.to_string(),
                frame.observed_at.format(&Rfc3339).map_err(|_| ())?,
                TRANSCRIPT_PARSER_VERSION,
            ],
        )
        .map_err(|_| ())?;
    if changed == 0 {
        return Ok(());
    }
    transaction
        .execute(
            "DELETE FROM claude_usage_message_supersedes
             WHERE replacement_frame_key = ?1",
            [&frame.frame_key],
        )
        .map_err(|_| ())?;
    for superseded_frame_key in &frame.supersedes_frame_keys {
        transaction
            .execute(
                "INSERT OR IGNORE INTO claude_usage_message_supersedes(
                   replacement_frame_key, superseded_frame_key, parser_version,
                   aggregate_applied
                 ) VALUES(?1, ?2, ?3, 0)",
                params![
                    frame.frame_key,
                    superseded_frame_key,
                    TRANSCRIPT_PARSER_VERSION
                ],
            )
            .map_err(|_| ())?;
    }
    Ok(())
}

fn store_frame_only(connection: &Connection, frame: NormalizedFrame) -> Result<(), ()> {
    let transaction = connection.unchecked_transaction().map_err(|_| ())?;
    store_frame(&transaction, &frame)?;
    transaction.commit().map_err(|_| ())
}

fn store_message(connection: &Connection, message: NormalizedMessage) -> Result<(), ()> {
    let observed_tokens = message.usage.observed_tokens().ok_or(())?;
    let transaction = connection.unchecked_transaction().map_err(|_| ())?;
    store_frame(
        &transaction,
        &NormalizedFrame {
            frame_key: message.frame_key.clone(),
            supersedes_frame_keys: message.supersedes_frame_keys.clone(),
            day: message.day,
            observed_at: message.observed_at,
        },
    )?;
    transaction
        .execute(
            "INSERT INTO claude_usage_messages(
               frame_key, supersedes_frame_key, message_key, day, observed_at, model, input_tokens,
               cache_creation_input_tokens, cache_read_input_tokens, output_tokens,
               cache_creation_5m_input_tokens, cache_creation_1h_input_tokens,
               service_tier, inference_geo, speed, web_search_requests,
               web_fetch_requests, code_execution_requests, has_unknown_paid_server_tool,
               observed_tokens, complete, parser_version
             ) VALUES(
               ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
               ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22
             )
             ON CONFLICT(frame_key) DO UPDATE SET
               supersedes_frame_key=NULL,
               message_key=excluded.message_key,
               day=excluded.day,
               observed_at=excluded.observed_at,
               model=excluded.model,
               input_tokens=excluded.input_tokens,
               cache_creation_input_tokens=excluded.cache_creation_input_tokens,
               cache_read_input_tokens=excluded.cache_read_input_tokens,
               output_tokens=excluded.output_tokens,
               cache_creation_5m_input_tokens=excluded.cache_creation_5m_input_tokens,
               cache_creation_1h_input_tokens=excluded.cache_creation_1h_input_tokens,
               service_tier=excluded.service_tier,
               inference_geo=excluded.inference_geo,
               speed=excluded.speed,
               web_search_requests=excluded.web_search_requests,
               web_fetch_requests=excluded.web_fetch_requests,
               code_execution_requests=excluded.code_execution_requests,
               has_unknown_paid_server_tool=excluded.has_unknown_paid_server_tool,
               observed_tokens=excluded.observed_tokens,
               complete=excluded.complete,
               parser_version=excluded.parser_version
             WHERE claude_usage_messages.parser_version != excluded.parser_version",
            params![
                message.frame_key,
                Option::<String>::None,
                message.message_key,
                message.day.to_string(),
                message.observed_at.format(&Rfc3339).map_err(|_| ())?,
                message.model,
                to_i64(message.usage.input)?,
                to_i64(message.usage.cache_creation_input)?,
                to_i64(message.usage.cache_read_input)?,
                to_i64(message.usage.output)?,
                message
                    .usage
                    .cache_creation_5m_input
                    .map(to_i64)
                    .transpose()?,
                message
                    .usage
                    .cache_creation_1h_input
                    .map(to_i64)
                    .transpose()?,
                message.pricing.service_tier,
                message.pricing.inference_geo,
                message.pricing.speed,
                message
                    .pricing
                    .web_search_requests
                    .map(to_i64)
                    .transpose()?,
                message.pricing.web_fetch_requests.map(to_i64).transpose()?,
                message
                    .pricing
                    .code_execution_requests
                    .map(to_i64)
                    .transpose()?,
                message.pricing.has_unknown_paid_server_tool,
                to_i64(observed_tokens)?,
                message.complete,
                TRANSCRIPT_PARSER_VERSION,
            ],
        )
        .map_err(|_| ())?;
    transaction.commit().map_err(|_| ())
}

struct FileScanContext<'a> {
    connection: &'a Connection,
    dedupe_salt: &'a [u8; 32],
    cutoff: Date,
    today: Date,
    started: Instant,
    max_millis: u128,
}

fn index_file(
    context: &FileScanContext<'_>,
    path: &Path,
    stored: Option<&StoredFileSummary>,
    remaining_bytes: &mut u64,
) -> Result<bool, ()> {
    let metadata = fs::metadata(path).map_err(|_| ())?;
    if !metadata.is_file() {
        return Err(());
    }
    let identity = file_identity(&metadata);
    let modified_ns = file_modified_ns(&metadata)?;
    let size = metadata.len();
    let resume_anchor_matches = stored.is_some_and(|stored| {
        stored.resume_anchor.is_some()
            && file_resume_anchor(path, stored.parsed_offset)
                .ok()
                .flatten()
                == stored.resume_anchor
    });
    let can_resume = stored.is_some_and(|stored| {
        stored.identity == identity
            && stored.parser_version == TRANSCRIPT_PARSER_VERSION
            && size >= stored.parsed_offset
            && (size > stored.size || modified_ns == stored.modified_ns)
            && resume_anchor_matches
    });
    let mut parsed_offset = stored
        .filter(|_| can_resume)
        .map_or(0, |stored| stored.parsed_offset);
    let mut parser_complete = stored
        .filter(|_| can_resume)
        .is_none_or(|stored| stored.completion_state != "error");
    let mut file = fs::File::open(path).map_err(|_| ())?;
    file.seek(SeekFrom::Start(parsed_offset)).map_err(|_| ())?;
    let mut reader = BufReader::new(file);
    while parsed_offset < size
        && *remaining_bytes > 0
        && context.started.elapsed().as_millis() < context.max_millis
    {
        let read_limit = (*remaining_bytes).min((MAX_TRANSCRIPT_LINE_BYTES as u64) + 1);
        let mut line = Vec::new();
        let bytes = reader
            .by_ref()
            .take(read_limit)
            .read_until(b'\n', &mut line)
            .map_err(|_| ())?;
        if bytes == 0 {
            break;
        }
        let bytes = u64::try_from(bytes).map_err(|_| ())?;
        let ends_with_newline = line.ends_with(b"\n");
        if !ends_with_newline {
            if line.len() <= MAX_TRANSCRIPT_LINE_BYTES {
                break;
            }
            parser_complete = false;
            parsed_offset = parsed_offset.checked_add(bytes).ok_or(())?;
            *remaining_bytes = remaining_bytes.saturating_sub(bytes);
            loop {
                if parsed_offset >= size
                    || *remaining_bytes == 0
                    || context.started.elapsed().as_millis() >= context.max_millis
                {
                    break;
                }
                let mut discarded = Vec::new();
                let allowance = (*remaining_bytes).min(64 * 1024);
                let read = reader
                    .by_ref()
                    .take(allowance)
                    .read_until(b'\n', &mut discarded)
                    .map_err(|_| ())?;
                if read == 0 {
                    break;
                }
                let read = u64::try_from(read).map_err(|_| ())?;
                let newline = discarded.ends_with(b"\n");
                parsed_offset = parsed_offset.checked_add(read).ok_or(())?;
                *remaining_bytes = remaining_bytes.saturating_sub(read);
                if newline {
                    break;
                }
            }
            continue;
        }
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        parsed_offset = parsed_offset.checked_add(bytes).ok_or(())?;
        *remaining_bytes = remaining_bytes.saturating_sub(bytes);
        if line.is_empty() {
            continue;
        }
        match parse_transcript_line(&line, context.dedupe_salt) {
            TranscriptLineOutcome::Ignored => {}
            TranscriptLineOutcome::Invalid => parser_complete = false,
            TranscriptLineOutcome::FrameOnly(frame) => {
                if frame.day >= context.cutoff && frame.day <= context.today + Duration::days(1) {
                    store_frame_only(context.connection, frame)?;
                }
                parser_complete = false;
            }
            TranscriptLineOutcome::Usage(message) => {
                if message.day >= context.cutoff && message.day <= context.today + Duration::days(1)
                {
                    store_message(context.connection, *message)?;
                } else if message.day > context.today + Duration::days(1) {
                    parser_complete = false;
                }
            }
        }
    }
    let completed = parsed_offset == size;
    let completion_state = if completed {
        if parser_complete { "complete" } else { "error" }
    } else {
        "indexing"
    };
    let resume_anchor = file_resume_anchor(path, parsed_offset)?;
    context
        .connection
        .execute(
            "INSERT INTO claude_usage_files(
               path, file_identity, size_bytes, modified_ns, parsed_offset, resume_anchor,
               parser_version, completion_state
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(path) DO UPDATE SET
               file_identity=excluded.file_identity,
               size_bytes=excluded.size_bytes,
               modified_ns=excluded.modified_ns,
               parsed_offset=excluded.parsed_offset,
               resume_anchor=excluded.resume_anchor,
               parser_version=excluded.parser_version,
               completion_state=excluded.completion_state",
            params![
                path.to_string_lossy(),
                identity,
                to_i64(size)?,
                modified_ns,
                to_i64(parsed_offset)?,
                resume_anchor,
                TRANSCRIPT_PARSER_VERSION,
                completion_state,
            ],
        )
        .map_err(|_| ())?;
    Ok(completed)
}

#[derive(Default)]
struct DailyAccumulator {
    observed_tokens: u64,
    complete: bool,
    observed_through: Option<OffsetDateTime>,
    priced_tokens: u64,
    cost_usd: f64,
    modeled: bool,
    pricing_rule_fingerprints: BTreeSet<String>,
}

fn merge_provider_message(mut current: StoredMessage, candidate: StoredMessage) -> StoredMessage {
    if current.day == candidate.day && current.observed_tokens == candidate.observed_tokens {
        let details_match = current.model == candidate.model
            && current.usage == candidate.usage
            && current.pricing == candidate.pricing;
        if details_match || !current.details_retained || !candidate.details_retained {
            let complete = current.complete | candidate.complete;
            let observed_at = current.observed_at.max(candidate.observed_at);
            let mut selected = if !current.details_retained && candidate.details_retained {
                candidate
            } else {
                current
            };
            selected.complete = complete;
            selected.observed_at = observed_at;
            return selected;
        }
    }
    match candidate.observed_tokens > current.observed_tokens {
        true => StoredMessage {
            complete: false,
            ..candidate
        },
        false => {
            current.complete = false;
            current
        }
    }
}

fn load_active_provider_messages(
    connection: &Connection,
    cutoff: Date,
    today: Date,
) -> Result<Vec<StoredMessage>, ()> {
    let mut statement = connection
        .prepare(
            "SELECT message_key, day, observed_at, model, input_tokens,
                    cache_creation_input_tokens, cache_read_input_tokens, output_tokens,
                    cache_creation_5m_input_tokens, cache_creation_1h_input_tokens,
                    service_tier, inference_geo, speed, web_search_requests,
                    web_fetch_requests, code_execution_requests, has_unknown_paid_server_tool,
                    observed_tokens, complete
             FROM claude_usage_messages AS message
             WHERE parser_version = ?1 AND day >= ?2 AND day <= ?3
               AND NOT EXISTS (
                 SELECT 1 FROM claude_usage_message_supersedes AS supersedes
                 WHERE supersedes.superseded_frame_key = message.frame_key
                   AND supersedes.parser_version = ?1
               )
             ORDER BY message_key, frame_key",
        )
        .map_err(|_| ())?;
    let messages = statement
        .query_map(
            params![
                TRANSCRIPT_PARSER_VERSION,
                cutoff.to_string(),
                today.to_string()
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    StoredMessage {
                        day: parse_ranking_day(&row.get::<_, String>(1)?)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        observed_at: OffsetDateTime::parse(&row.get::<_, String>(2)?, &Rfc3339)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        model: row.get(3)?,
                        usage: ClaudeTokenUsage {
                            input: from_i64(row.get(4)?)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            cache_creation_input: from_i64(row.get(5)?)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            cache_read_input: from_i64(row.get(6)?)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            output: from_i64(row.get(7)?)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            cache_creation_5m_input: row
                                .get::<_, Option<i64>>(8)?
                                .map(from_i64)
                                .transpose()
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            cache_creation_1h_input: row
                                .get::<_, Option<i64>>(9)?
                                .map(from_i64)
                                .transpose()
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        },
                        pricing: ClaudePricingMetadata {
                            service_tier: row.get(10)?,
                            inference_geo: row.get(11)?,
                            speed: row.get(12)?,
                            web_search_requests: row
                                .get::<_, Option<i64>>(13)?
                                .map(from_i64)
                                .transpose()
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            web_fetch_requests: row
                                .get::<_, Option<i64>>(14)?
                                .map(from_i64)
                                .transpose()
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            code_execution_requests: row
                                .get::<_, Option<i64>>(15)?
                                .map(from_i64)
                                .transpose()
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            has_unknown_paid_server_tool: row.get(16)?,
                        },
                        observed_tokens: from_i64(row.get(17)?)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        details_retained: !row.get::<_, String>(3)?.is_empty(),
                        complete: row.get(18)?,
                    },
                ))
            },
        )
        .map_err(|_| ())?;
    let mut provider_messages = BTreeMap::<String, StoredMessage>::new();
    for message in messages {
        let (message_key, candidate) = message.map_err(|_| ())?;
        if candidate.details_retained
            && candidate.usage.observed_tokens() != Some(candidate.observed_tokens)
        {
            return Err(());
        }
        match provider_messages.entry(message_key) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(candidate);
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                let current = entry.get().clone();
                entry.insert(merge_provider_message(current, candidate));
            }
        }
    }
    Ok(provider_messages.into_values().collect())
}

fn price_stored_message(
    catalog: &super::pricing::PricingCatalog,
    message: &StoredMessage,
) -> super::pricing::PriceDecision {
    catalog.price_message(
        &message.model,
        message.day,
        super::pricing::BillableUsage {
            input_tokens: message.usage.input,
            cache_creation_input_tokens: message.usage.cache_creation_input,
            cache_creation_5m_input_tokens: message.usage.cache_creation_5m_input,
            cache_creation_1h_input_tokens: message.usage.cache_creation_1h_input,
            cache_read_input_tokens: message.usage.cache_read_input,
            output_tokens: message.usage.output,
            service_tier: message.pricing.service_tier.as_deref(),
            inference_geo: message.pricing.inference_geo.as_deref(),
            speed: message.pricing.speed.as_deref(),
            web_search_requests: message.pricing.web_search_requests,
            web_fetch_requests: message.pricing.web_fetch_requests.unwrap_or(0),
            code_execution_requests: message.pricing.code_execution_requests,
            has_unknown_paid_server_tool: message.pricing.has_unknown_paid_server_tool,
        },
    )
}

#[derive(Clone)]
struct StoredDailyAggregate {
    observed_tokens: u64,
    coverage: String,
    observed_through: OffsetDateTime,
    revision: u64,
    priced_tokens: u64,
    cost_usd: Option<f64>,
    modeled: bool,
    pricing_basis: Option<String>,
    pricing_fingerprint: Option<String>,
    correction: Option<ProviderCorrection>,
}

fn decode_stored_correction(
    provenance: Option<String>,
    source_revision: Option<i64>,
    current_revision: u64,
) -> Result<Option<ProviderCorrection>, ()> {
    match (provenance.as_deref(), source_revision) {
        (None, None) => Ok(None),
        (Some(PARSER_CORRECTION_PROVENANCE), Some(source_revision)) => {
            let source_revision = from_i64(source_revision)?;
            if source_revision == 0 || source_revision > current_revision {
                return Err(());
            }
            Ok(Some(ProviderCorrection::ParserCorrection {
                source_revision,
            }))
        }
        _ => Err(()),
    }
}

fn load_stored_daily_aggregates(
    connection: &Connection,
    cutoff: Date,
    today: Date,
) -> Result<BTreeMap<Date, StoredDailyAggregate>, ()> {
    let mut statement = connection
        .prepare(
            "SELECT day, observed_tokens, coverage, observed_through, revision,
                    priced_tokens, cost_usd, cost_modeled, pricing_basis, pricing_fingerprint,
                    correction_provenance, correction_source_revision
             FROM claude_usage_daily
             WHERE day >= ?1 AND day <= ?2
             ORDER BY day",
        )
        .map_err(|_| ())?;
    let rows = statement
        .query_map(params![cutoff.to_string(), today.to_string()], |row| {
            let revision = from_i64(row.get(4)?).map_err(|_| rusqlite::Error::InvalidQuery)?;
            Ok((
                parse_ranking_day(&row.get::<_, String>(0)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                StoredDailyAggregate {
                    observed_tokens: from_i64(row.get(1)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    coverage: row.get(2)?,
                    observed_through: OffsetDateTime::parse(&row.get::<_, String>(3)?, &Rfc3339)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    revision,
                    priced_tokens: from_i64(row.get(5)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    cost_usd: row.get(6)?,
                    modeled: row.get(7)?,
                    pricing_basis: row.get(8)?,
                    pricing_fingerprint: row.get(9)?,
                    correction: decode_stored_correction(row.get(10)?, row.get(11)?, revision)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                },
            ))
        })
        .map_err(|_| ())?;
    let mut daily = BTreeMap::new();
    for row in rows {
        let (day, aggregate) = row.map_err(|_| ())?;
        daily.insert(day, aggregate);
    }
    Ok(daily)
}

fn stored_usage_aggregate_parser_version(connection: &Connection) -> Result<Option<i64>, ()> {
    let stored = connection
        .query_row(
            "SELECT value FROM claude_usage_index_meta WHERE key = ?1",
            [USAGE_AGGREGATE_PARSER_VERSION_KEY],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| ())?;
    stored
        .map(|value| value.parse::<i64>().map_err(|_| ()))
        .transpose()
}

fn load_pending_explicit_supersede_days(
    connection: &Connection,
    cutoff: Date,
    today: Date,
) -> Result<BTreeMap<Date, OffsetDateTime>, ()> {
    let mut statement = connection
        .prepare(
            "SELECT superseded.day, replacement.observed_at
             FROM claude_usage_message_supersedes AS edge
             JOIN claude_usage_frames AS replacement
               ON replacement.frame_key = edge.replacement_frame_key
             JOIN claude_usage_messages AS superseded
               ON superseded.frame_key = edge.superseded_frame_key
             WHERE edge.parser_version = ?1
               AND edge.aggregate_applied = 0
               AND replacement.parser_version = ?1
               AND superseded.parser_version = ?1
               AND superseded.day >= ?2 AND superseded.day <= ?3",
        )
        .map_err(|_| ())?;
    let rows = statement
        .query_map(
            params![
                TRANSCRIPT_PARSER_VERSION,
                cutoff.to_string(),
                today.to_string()
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(|_| ())?;
    let mut corrections = BTreeMap::new();
    for row in rows {
        let (day, observed_at) = row.map_err(|_| ())?;
        let day = parse_ranking_day(&day)?;
        let observed_at = OffsetDateTime::parse(&observed_at, &Rfc3339).map_err(|_| ())?;
        corrections
            .entry(day)
            .and_modify(|current: &mut OffsetDateTime| *current = (*current).max(observed_at))
            .or_insert(observed_at);
    }
    Ok(corrections)
}

fn mark_explicit_supersede_edges_applied(
    connection: &Connection,
    cutoff: Date,
    today: Date,
) -> Result<(), ()> {
    connection
        .execute(
            "UPDATE claude_usage_message_supersedes AS edge
             SET aggregate_applied = 1
             WHERE edge.parser_version = ?1
               AND edge.aggregate_applied = 0
               AND EXISTS (
                 SELECT 1
                 FROM claude_usage_frames AS replacement
                 JOIN claude_usage_messages AS superseded
                   ON superseded.frame_key = edge.superseded_frame_key
                 WHERE replacement.frame_key = edge.replacement_frame_key
                   AND replacement.parser_version = ?1
                   AND superseded.parser_version = ?1
                   AND superseded.day >= ?2 AND superseded.day <= ?3
               )",
            params![
                TRANSCRIPT_PARSER_VERSION,
                cutoff.to_string(),
                today.to_string()
            ],
        )
        .map(|_| ())
        .map_err(|_| ())
}

fn refresh_daily_aggregates(
    connection: &Connection,
    cutoff: Date,
    today: Date,
    scan_can_prove_complete: bool,
) -> Result<bool, ()> {
    refresh_daily_aggregates_with_catalog(
        connection,
        cutoff,
        today,
        scan_can_prove_complete,
        super::pricing::catalog(),
    )
}

fn refresh_daily_aggregates_with_catalog(
    connection: &Connection,
    cutoff: Date,
    today: Date,
    scan_can_prove_complete: bool,
    pricing_catalog: Option<&super::pricing::PricingCatalog>,
) -> Result<bool, ()> {
    let transaction = connection.unchecked_transaction().map_err(|_| ())?;
    let provider_messages = load_active_provider_messages(&transaction, cutoff, today)?;
    let existing_daily = load_stored_daily_aggregates(&transaction, cutoff, today)?;
    let stored_parser_version = stored_usage_aggregate_parser_version(&transaction)?;
    if stored_parser_version.is_some_and(|version| version > TRANSCRIPT_PARSER_VERSION) {
        return Err(());
    }
    let parser_correction_pending =
        !existing_daily.is_empty() && stored_parser_version != Some(TRANSCRIPT_PARSER_VERSION);
    let explicit_corrections = if scan_can_prove_complete {
        load_pending_explicit_supersede_days(&transaction, cutoff, today)?
    } else {
        BTreeMap::new()
    };
    let mut daily = BTreeMap::<Date, DailyAccumulator>::new();
    let mut aggregate_changed = false;
    let cost_cutoff = today - Duration::days(COST_DETAIL_RETENTION_DAYS - 1);
    for message in provider_messages {
        let observed_tokens = message.observed_tokens;
        let day = message.day;
        let observed_at = message.observed_at;
        let complete = message.complete;
        let entry = daily.entry(day).or_insert_with(|| DailyAccumulator {
            complete: true,
            ..DailyAccumulator::default()
        });
        entry.observed_tokens = entry
            .observed_tokens
            .checked_add(observed_tokens)
            .ok_or(())?;
        entry.complete &= complete;
        if complete
            && message.details_retained
            && day >= cost_cutoff
            && let Some(catalog) = pricing_catalog
        {
            let decision = price_stored_message(catalog, &message);
            entry
                .pricing_rule_fingerprints
                .insert(decision.rule_fingerprint);
            if let Some(cost_usd) = decision.cost_usd {
                entry.priced_tokens = entry
                    .priced_tokens
                    .checked_add(decision.priced_tokens)
                    .ok_or(())?;
                entry.cost_usd += cost_usd;
                entry.modeled |= decision.modeled;
            }
        }
        entry.observed_through = Some(
            entry
                .observed_through
                .map_or(observed_at, |current| current.max(observed_at)),
        );
    }
    if scan_can_prove_complete {
        for (day, observed_at) in &explicit_corrections {
            let entry = daily.entry(*day).or_insert_with(|| DailyAccumulator {
                complete: true,
                ..DailyAccumulator::default()
            });
            entry.observed_through = Some(
                entry
                    .observed_through
                    .map_or(*observed_at, |current| current.max(*observed_at)),
            );
        }
        if parser_correction_pending {
            for (day, existing) in &existing_daily {
                let entry = daily.entry(*day).or_insert_with(|| DailyAccumulator {
                    complete: true,
                    ..DailyAccumulator::default()
                });
                entry.observed_through = Some(
                    entry
                        .observed_through
                        .map_or(existing.observed_through, |current| {
                            current.max(existing.observed_through)
                        }),
                );
            }
        }
    } else {
        for (day, existing) in &existing_daily {
            daily.entry(*day).or_insert_with(|| DailyAccumulator {
                complete: false,
                observed_through: Some(existing.observed_through),
                ..DailyAccumulator::default()
            });
        }
    }
    for (day, candidate) in daily {
        let observed_through = candidate.observed_through.ok_or(())?;
        let candidate_pricing_fingerprint = pricing_catalog.and_then(|_| {
            (!candidate.pricing_rule_fingerprints.is_empty()).then(|| {
                candidate
                    .pricing_rule_fingerprints
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(",")
            })
        });
        let candidate_pricing_basis = pricing_catalog
            .filter(|_| candidate.priced_tokens > 0)
            .map(|catalog| catalog.basis().to_owned());
        let candidate_cost_usd = (candidate.priced_tokens > 0).then_some(candidate.cost_usd);
        let coverage = if scan_can_prove_complete && candidate.complete {
            "complete"
        } else {
            "partial"
        };
        let (
            observed_tokens,
            observed_through,
            revision,
            priced_tokens,
            cost_usd,
            modeled,
            pricing_basis,
            pricing_fingerprint,
            correction,
            row_changed,
        ) = match existing_daily.get(&day) {
            None => (
                candidate.observed_tokens,
                observed_through,
                1,
                candidate.priced_tokens,
                candidate_cost_usd,
                candidate.modeled,
                candidate_pricing_basis,
                candidate_pricing_fingerprint,
                None,
                true,
            ),
            Some(previous) => {
                let lower_correction_allowed = scan_can_prove_complete
                    && (parser_correction_pending || explicit_corrections.contains_key(&day));
                let proven_parser_correction_applied = lower_correction_allowed
                    && candidate.observed_tokens < previous.observed_tokens;
                let accept_candidate = scan_can_prove_complete
                    && (lower_correction_allowed
                        || candidate.observed_tokens >= previous.observed_tokens);
                let observed_tokens = if lower_correction_allowed {
                    candidate.observed_tokens
                } else {
                    previous.observed_tokens.max(candidate.observed_tokens)
                };
                let observed_through = previous.observed_through.max(observed_through);
                let (priced_tokens, cost_usd, modeled, accepted_pricing_basis, pricing_fingerprint) =
                    if accept_candidate {
                        (
                            candidate.priced_tokens,
                            candidate_cost_usd,
                            candidate.modeled,
                            candidate_pricing_basis,
                            candidate_pricing_fingerprint,
                        )
                    } else {
                        (
                            previous.priced_tokens,
                            previous.cost_usd,
                            previous.modeled,
                            previous.pricing_basis.clone(),
                            previous.pricing_fingerprint.clone(),
                        )
                    };
                let material_changed = observed_tokens != previous.observed_tokens
                    || observed_through != previous.observed_through
                    || previous.coverage != coverage
                    || previous.priced_tokens != priced_tokens
                    || previous.cost_usd.map(f64::to_bits) != cost_usd.map(f64::to_bits)
                    || previous.modeled != modeled
                    || previous.pricing_fingerprint != pricing_fingerprint;
                let pricing_basis =
                    if accept_candidate && !material_changed && previous.pricing_basis.is_some() {
                        previous.pricing_basis.clone()
                    } else {
                        accepted_pricing_basis
                    };
                let changed = material_changed || previous.pricing_basis != pricing_basis;
                let revision = previous
                    .revision
                    .checked_add(u64::from(changed))
                    .ok_or(())?;
                let correction = if proven_parser_correction_applied {
                    Some(ProviderCorrection::ParserCorrection {
                        source_revision: revision,
                    })
                } else {
                    previous.correction
                };
                (
                    observed_tokens,
                    observed_through,
                    revision,
                    priced_tokens,
                    cost_usd,
                    modeled,
                    pricing_basis,
                    pricing_fingerprint,
                    correction,
                    changed,
                )
            }
        };
        aggregate_changed |= row_changed;
        let (correction_provenance, correction_source_revision) = match correction {
            Some(ProviderCorrection::ParserCorrection { source_revision }) => (
                Some(PARSER_CORRECTION_PROVENANCE),
                Some(to_i64(source_revision)?),
            ),
            None => (None, None),
        };
        transaction
            .execute(
                "INSERT INTO claude_usage_daily(
                   day, observed_tokens, coverage, observed_through, revision,
                   priced_tokens, cost_usd, cost_modeled, pricing_basis, pricing_fingerprint,
                   correction_provenance, correction_source_revision
                 ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT(day) DO UPDATE SET
                   observed_tokens=excluded.observed_tokens,
                   coverage=excluded.coverage,
                   observed_through=excluded.observed_through,
                   revision=excluded.revision,
                   priced_tokens=excluded.priced_tokens,
                   cost_usd=excluded.cost_usd,
                   cost_modeled=excluded.cost_modeled,
                   pricing_basis=excluded.pricing_basis,
                   pricing_fingerprint=excluded.pricing_fingerprint,
                   correction_provenance=excluded.correction_provenance,
                   correction_source_revision=excluded.correction_source_revision",
                params![
                    day.to_string(),
                    to_i64(observed_tokens)?,
                    coverage,
                    observed_through.format(&Rfc3339).map_err(|_| ())?,
                    to_i64(revision)?,
                    to_i64(priced_tokens)?,
                    cost_usd,
                    modeled,
                    pricing_basis,
                    pricing_fingerprint,
                    correction_provenance,
                    correction_source_revision,
                ],
            )
            .map_err(|_| ())?;
    }
    if scan_can_prove_complete {
        mark_explicit_supersede_edges_applied(&transaction, cutoff, today)?;
    }
    match pricing_catalog {
        Some(catalog) => transaction
            .execute(
                "INSERT INTO claude_usage_index_meta(key, value)
                 VALUES('pricing_manifest_fingerprint', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [catalog.semantic_fingerprint()],
            )
            .map_err(|_| ())?,
        None => transaction
            .execute(
                "DELETE FROM claude_usage_index_meta
                 WHERE key = 'pricing_manifest_fingerprint'",
                [],
            )
            .map_err(|_| ())?,
    };
    if scan_can_prove_complete {
        transaction
            .execute(
                "INSERT INTO claude_usage_index_meta(key, value)
                 VALUES(?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![
                    USAGE_AGGREGATE_PARSER_VERSION_KEY,
                    TRANSCRIPT_PARSER_VERSION.to_string()
                ],
            )
            .map_err(|_| ())?;
    }
    transaction.commit().map_err(|_| ())?;
    Ok(aggregate_changed)
}

fn prune_expired_index(connection: &Connection, cutoff: Date, today: Date) -> Result<(), ()> {
    let transaction = connection.unchecked_transaction().map_err(|_| ())?;
    transaction
        .execute(
            "DELETE FROM claude_usage_messages WHERE day < ?1 OR day > ?2",
            params![cutoff.to_string(), (today + Duration::days(1)).to_string()],
        )
        .map_err(|_| ())?;
    transaction
        .execute(
            "DELETE FROM claude_usage_frames WHERE day < ?1 OR day > ?2",
            params![cutoff.to_string(), (today + Duration::days(1)).to_string()],
        )
        .map_err(|_| ())?;
    transaction
        .execute(
            "DELETE FROM claude_usage_daily WHERE day < ?1 OR day > ?2",
            params![cutoff.to_string(), today.to_string()],
        )
        .map_err(|_| ())?;
    let cutoff_modified_ns =
        i64::try_from(cutoff.midnight().assume_utc().unix_timestamp_nanos()).map_err(|_| ())?;
    transaction
        .execute(
            "DELETE FROM claude_usage_files WHERE modified_ns < ?1",
            [cutoff_modified_ns],
        )
        .map_err(|_| ())?;
    transaction.commit().map_err(|_| ())
}

fn prune_private_message_details(connection: &Connection, today: Date) -> Result<(), ()> {
    let detail_cutoff = today - Duration::days(COST_DETAIL_RETENTION_DAYS - 1);
    let transaction = connection.unchecked_transaction().map_err(|_| ())?;
    transaction
        .execute(
            "UPDATE claude_usage_messages
             SET model = '',
                 input_tokens = 0,
                 cache_creation_input_tokens = 0,
                 cache_read_input_tokens = 0,
                 output_tokens = 0,
                 cache_creation_5m_input_tokens = NULL,
                 cache_creation_1h_input_tokens = NULL,
                 service_tier = NULL,
                 inference_geo = NULL,
                 speed = NULL,
                 web_search_requests = NULL,
                 web_fetch_requests = NULL,
                 code_execution_requests = NULL,
                 has_unknown_paid_server_tool = 0
             WHERE day < ?1 AND (
               model != '' OR input_tokens != 0 OR cache_creation_input_tokens != 0
               OR cache_read_input_tokens != 0 OR output_tokens != 0
               OR cache_creation_5m_input_tokens IS NOT NULL
               OR cache_creation_1h_input_tokens IS NOT NULL
               OR service_tier IS NOT NULL OR inference_geo IS NOT NULL
               OR speed IS NOT NULL OR web_search_requests IS NOT NULL
               OR web_fetch_requests IS NOT NULL OR code_execution_requests IS NOT NULL
               OR has_unknown_paid_server_tool != 0
             )",
            [detail_cutoff.to_string()],
        )
        .map_err(|_| ())?;
    transaction
        .execute(
            "UPDATE claude_usage_daily
             SET priced_tokens = 0,
                 cost_usd = NULL,
                 cost_modeled = 0,
                 pricing_basis = NULL,
                 pricing_fingerprint = NULL
             WHERE day < ?1 AND (
               priced_tokens != 0 OR cost_usd IS NOT NULL
               OR cost_modeled != 0
               OR pricing_basis IS NOT NULL OR pricing_fingerprint IS NOT NULL
             )",
            [detail_cutoff.to_string()],
        )
        .map_err(|_| ())?;
    transaction.commit().map_err(|_| ())
}

fn read_indexed_usage(
    connection: &Connection,
    cutoff: Date,
    today: Date,
    scan_status: UsageScanStatus,
    scan_scope_known: bool,
    transcript_source_present: bool,
    aggregate_changed: bool,
) -> Result<LocalUsageObservation, ()> {
    let cost_cutoff = today - Duration::days(COST_DETAIL_RETENTION_DAYS - 1);
    let mut statement = connection
        .prepare(
            "SELECT day, observed_tokens, coverage, observed_through,
                    priced_tokens, cost_usd, cost_modeled, pricing_basis, revision,
                    correction_provenance, correction_source_revision
             FROM claude_usage_daily
             WHERE day >= ?1 AND day <= ?2 ORDER BY day",
        )
        .map_err(|_| ())?;
    let rows = statement
        .query_map(params![cutoff.to_string(), today.to_string()], |row| {
            let day = parse_ranking_day(&row.get::<_, String>(0)?)
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
            let observed_tokens =
                from_i64(row.get(1)?).map_err(|_| rusqlite::Error::InvalidQuery)?;
            let stored_coverage = match row.get::<_, String>(2)?.as_str() {
                "complete" => UsageCoverage::Complete,
                "partial" => UsageCoverage::Partial,
                _ => return Err(rusqlite::Error::InvalidQuery),
            };
            let coverage = if scan_scope_known && scan_status == UsageScanStatus::Complete {
                stored_coverage
            } else {
                UsageCoverage::Partial
            };
            let observed_through = OffsetDateTime::parse(&row.get::<_, String>(3)?, &Rfc3339)
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
            let priced_tokens = from_i64(row.get(4)?).map_err(|_| rusqlite::Error::InvalidQuery)?;
            let cost_usd = row.get::<_, Option<f64>>(5)?;
            let modeled = row.get::<_, bool>(6)?;
            let pricing_basis = row.get::<_, Option<String>>(7)?;
            let revision = from_i64(row.get(8)?).map_err(|_| rusqlite::Error::InvalidQuery)?;
            let correction = decode_stored_correction(row.get(9)?, row.get(10)?, revision)
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
            Ok((
                day,
                observed_tokens,
                coverage,
                observed_through,
                priced_tokens,
                cost_usd,
                modeled,
                pricing_basis,
                correction,
            ))
        })
        .map_err(|_| ())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ())?;
    let mut daily_usage = BTreeMap::new();
    let mut daily_cost = BTreeMap::new();
    let mut pricing_bases = BTreeSet::new();
    let mut daily_corrections = BTreeMap::new();
    let mut correction = None;
    for (
        day,
        observed_tokens,
        coverage,
        observed_through,
        priced_tokens,
        cost_usd,
        modeled,
        pricing_basis,
        stored_correction,
    ) in rows
    {
        if let Some(stored_correction) = stored_correction {
            daily_corrections.insert(day, stored_correction);
            if day == today {
                correction = Some(stored_correction);
            }
        }
        daily_usage.insert(
            day,
            DailyUsageEvidence {
                observed_tokens,
                coverage,
                observed_through: Some(observed_through),
            },
        );
        if day >= cost_cutoff && priced_tokens > 0 {
            if let (Some(cost_usd), Some(pricing_basis)) = (cost_usd, pricing_basis) {
                pricing_bases.insert(pricing_basis.clone());
                daily_cost.insert(
                    day,
                    DailyCostEvidence {
                        observed_tokens,
                        priced_tokens,
                        api_equivalent_cost_usd: Some(cost_usd),
                        modeled,
                        complete: priced_tokens == observed_tokens,
                        observed_through: Some(observed_through),
                        priced_observed_through: Some(observed_through),
                        pricing_basis: Some(pricing_basis),
                    },
                );
            }
        }
    }
    let (latest_pending_ns, latest_error_ns) = connection
        .query_row(
            "SELECT
               MAX(CASE WHEN completion_state = 'indexing' THEN modified_ns END),
               MAX(CASE WHEN completion_state IN ('error', 'missing') THEN modified_ns END)
             FROM claude_usage_files",
            [],
            |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
        )
        .map_err(|_| ())?;
    let parse_modified = |value: Option<i64>| {
        value
            .map(|value| OffsetDateTime::from_unix_timestamp_nanos(i128::from(value)))
            .transpose()
            .map_err(|_| ())
    };
    let pricing_basis = composite_pricing_basis(pricing_bases);
    let top_model_usage = read_top_model_usage(connection, cost_cutoff, today)?;
    Ok(LocalUsageObservation {
        daily_usage,
        daily_cost,
        top_model_usage,
        pricing_basis,
        scan_status,
        latest_pending_modified_at: parse_modified(latest_pending_ns)?,
        latest_error_modified_at: parse_modified(latest_error_ns)?,
        scan_scope_known,
        transcript_source_present,
        aggregate_changed,
        daily_corrections,
        correction,
    })
}

fn read_top_model_usage(
    connection: &Connection,
    cutoff: Date,
    today: Date,
) -> Result<Option<TopModelUsage>, ()> {
    let catalog = super::pricing::catalog();
    let entries = load_active_provider_messages(connection, cutoff, today)?
        .into_iter()
        .map(|message| {
            let display_name = message
                .details_retained
                .then_some(message.model.as_str())
                .and_then(|model| catalog.and_then(|catalog| catalog.canonical_model_name(model)))
                .and_then(crate::providers::normalized_model_display_name);
            let grouping_key = display_name
                .clone()
                .unwrap_or_else(|| message.model.clone());
            (grouping_key, display_name, message.observed_tokens)
        });
    Ok(crate::providers::select_top_model_usage(entries))
}

fn debug_cost_quality(quality: Option<ApiEquivalentCostQuality>) -> &'static str {
    match quality {
        Some(ApiEquivalentCostQuality::Reconciled) => "reconciled",
        Some(ApiEquivalentCostQuality::Modeled) => "modeled",
        Some(ApiEquivalentCostQuality::LocalOnly) => "local-only",
        None => "unavailable",
    }
}

fn debug_period_line(
    label: &str,
    length: i64,
    today: Date,
    local: &LocalUsageObservation,
    projected: &UsageTotal,
) -> String {
    let days = period_days(today, length, 0).collect::<Vec<_>>();
    let local_tokens = checked_sum(days.iter().filter_map(|day| {
        local
            .daily_usage
            .get(day)
            .map(|detail| &detail.observed_tokens)
    }));
    let priced_tokens = checked_sum(days.iter().filter_map(|day| {
        local
            .daily_cost
            .get(day)
            .map(|detail| &detail.priced_tokens)
    }));
    let (
        availability,
        evidence_basis,
        usage_coverage,
        authoritative_tokens,
        cost,
        quality,
        cost_coverage,
        trend,
    ) = match projected {
        UsageTotal::Current {
            evidence_basis,
            coverage,
            observed_tokens,
            api_equivalent_cost_usd,
            trend_percent,
            api_equivalent_cost_quality,
            api_equivalent_cost_coverage_percent,
            ..
        } => (
            "current",
            format!("{evidence_basis:?}"),
            format!("{coverage:?}"),
            Some(*observed_tokens),
            *api_equivalent_cost_usd,
            debug_cost_quality(*api_equivalent_cost_quality),
            *api_equivalent_cost_coverage_percent,
            *trend_percent,
        ),
        UsageTotal::Stale {
            evidence_basis,
            coverage,
            observed_tokens,
            api_equivalent_cost_usd,
            trend_percent,
            api_equivalent_cost_quality,
            api_equivalent_cost_coverage_percent,
            ..
        } => (
            "stale",
            format!("{evidence_basis:?}"),
            format!("{coverage:?}"),
            Some(*observed_tokens),
            *api_equivalent_cost_usd,
            debug_cost_quality(*api_equivalent_cost_quality),
            *api_equivalent_cost_coverage_percent,
            *trend_percent,
        ),
        UsageTotal::Unavailable => (
            "unavailable",
            "unavailable".to_owned(),
            "unavailable".to_owned(),
            None,
            None,
            "unavailable",
            None,
            None,
        ),
    };
    format!(
        "[TouchGrassBar][claude-usage-report] period={label} availability={availability} evidence_basis={evidence_basis} usage_coverage={usage_coverage} local_detail_tokens={} priced_local_tokens={} authoritative_tokens={} projected_cost_usd={} quality={quality} coverage_percent={} trend_percent={}",
        local_tokens.map_or_else(|| "unavailable".to_owned(), |value| value.to_string()),
        priced_tokens.map_or_else(|| "unavailable".to_owned(), |value| value.to_string()),
        authoritative_tokens.map_or_else(|| "unavailable".to_owned(), |value| value.to_string()),
        cost.map_or_else(|| "unavailable".to_owned(), |value| format!("{value:.6}")),
        cost_coverage.map_or_else(|| "unavailable".to_owned(), |value| format!("{value:.2}")),
        trend.map_or_else(|| "unavailable".to_owned(), |value| format!("{value:.2}")),
    )
}

#[derive(Default)]
struct DebugModelDay {
    input_tokens: u64,
    cache_creation_input_tokens: u64,
    cache_read_input_tokens: u64,
    output_tokens: u64,
    observed_tokens: u64,
    priced_tokens: u64,
    cost_usd: f64,
    detail_complete: bool,
    pricing_rule_fingerprints: BTreeSet<String>,
}

pub(super) fn debug_usage_report(
    database_path: &Path,
    config_root: &Path,
    probe_directory: &Path,
    now: OffsetDateTime,
) -> Result<String, ()> {
    let local = index_local_usage_at(database_path, config_root, probe_directory, now).ok_or(())?;
    let periods = project_usage_periods(Some(&local), now);
    let connection = Connection::open(database_path).map_err(|_| ())?;
    let (complete_files, pending_files, error_files, missing_files) = connection
        .query_row(
            "SELECT
               COALESCE(SUM(CASE WHEN completion_state = 'complete' THEN 1 ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN completion_state = 'indexing' THEN 1 ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN completion_state = 'error' THEN 1 ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN completion_state = 'missing' THEN 1 ELSE 0 END), 0)
             FROM claude_usage_files WHERE parser_version = ?1",
            [TRANSCRIPT_PARSER_VERSION],
            |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, u64>(3)?,
                ))
            },
        )
        .map_err(|_| ())?;
    let pricing_catalog = super::pricing::catalog();
    let pricing_basis = pricing_catalog.map_or("unavailable", |catalog| catalog.basis());
    let pricing_fingerprint =
        pricing_catalog.map_or("unavailable", |catalog| catalog.semantic_fingerprint());
    let mut lines = vec![format!(
        "[TouchGrassBar][claude-usage-report] token_retention_days={TOKEN_HISTORY_RETENTION_DAYS} private_detail_retention_days={COST_DETAIL_RETENTION_DAYS} pricing_basis={pricing_basis} pricing_fingerprint={pricing_fingerprint} scan={:?} today_scan={:?} seven_day_scan={:?} thirty_day_scan={:?} complete_files={complete_files} pending_files={pending_files} error_files={error_files} missing_files={missing_files}",
        periods.scan_status,
        periods.today_scan_status,
        periods.seven_day_scan_status,
        periods.thirty_day_scan_status,
    )];
    let today = utc_ranking_day(now);
    lines.push(debug_period_line("today", 1, today, &local, &periods.today));
    lines.push(debug_period_line(
        "7-day",
        7,
        today,
        &local,
        &periods.seven_days,
    ));
    lines.push(debug_period_line(
        "30-day",
        30,
        today,
        &local,
        &periods.thirty_days,
    ));

    let detail_cutoff = today - Duration::days(COST_DETAIL_RETENTION_DAYS - 1);
    let mut model_days = BTreeMap::<(Date, String), DebugModelDay>::new();
    for message in load_active_provider_messages(&connection, detail_cutoff, today)? {
        if !message.details_retained {
            continue;
        }
        let observed_tokens = message.observed_tokens;
        let decision = message
            .complete
            .then(|| pricing_catalog.map(|catalog| price_stored_message(catalog, &message)))
            .flatten();
        let entry = model_days
            .entry((message.day, message.model.clone()))
            .or_insert_with(|| DebugModelDay {
                detail_complete: true,
                ..DebugModelDay::default()
            });
        entry.input_tokens = entry
            .input_tokens
            .checked_add(message.usage.input)
            .ok_or(())?;
        entry.cache_creation_input_tokens = entry
            .cache_creation_input_tokens
            .checked_add(message.usage.cache_creation_input)
            .ok_or(())?;
        entry.cache_read_input_tokens = entry
            .cache_read_input_tokens
            .checked_add(message.usage.cache_read_input)
            .ok_or(())?;
        entry.output_tokens = entry
            .output_tokens
            .checked_add(message.usage.output)
            .ok_or(())?;
        entry.observed_tokens = entry
            .observed_tokens
            .checked_add(observed_tokens)
            .ok_or(())?;
        entry.detail_complete &= message.complete;
        if let Some(decision) = decision {
            entry
                .pricing_rule_fingerprints
                .insert(decision.rule_fingerprint);
            if let Some(cost_usd) = decision.cost_usd {
                entry.priced_tokens = entry
                    .priced_tokens
                    .checked_add(decision.priced_tokens)
                    .ok_or(())?;
                entry.cost_usd += cost_usd;
            }
        }
    }
    for ((day, model), detail) in model_days.into_iter().rev() {
        let pricing_status =
            if detail.priced_tokens == detail.observed_tokens && detail.detail_complete {
                "complete"
            } else if detail.priced_tokens > 0 {
                "partial"
            } else {
                "unavailable"
            };
        lines.push(format!(
            "[TouchGrassBar][claude-usage-report] day={day} model={model} observed_tokens={} input_tokens={} cache_creation_input_tokens={} cache_read_input_tokens={} output_tokens={} priced_tokens={} local_cost_usd={:.6} detail_complete={} pricing_status={pricing_status} pricing_rule_count={}",
            detail.observed_tokens,
            detail.input_tokens,
            detail.cache_creation_input_tokens,
            detail.cache_read_input_tokens,
            detail.output_tokens,
            detail.priced_tokens,
            detail.cost_usd,
            detail.detail_complete,
            detail.pricing_rule_fingerprints.len(),
        ));
    }
    Ok(lines.join("\n"))
}

fn index_local_usage_at(
    database_path: &Path,
    config_root: &Path,
    probe_directory: &Path,
    now: OffsetDateTime,
) -> Option<LocalUsageObservation> {
    index_local_usage_with_budget(
        database_path,
        config_root,
        probe_directory,
        now,
        DEFAULT_SCAN_BUDGET,
    )
}

fn index_local_usage_with_budget(
    database_path: &Path,
    config_root: &Path,
    probe_directory: &Path,
    now: OffsetDateTime,
    budget: ScanBudget,
) -> Option<LocalUsageObservation> {
    let started = Instant::now();
    let max_bytes = budget.max_bytes.min(MAX_TRANSCRIPT_SCAN_BYTES);
    let max_file_bytes = budget.max_file_bytes.min(MAX_TRANSCRIPT_FILE_SCAN_BYTES);
    let max_millis = budget.max_millis.min(MAX_TRANSCRIPT_SCAN_MILLIS);
    let mut connection = Connection::open(database_path).ok()?;
    ensure_index_schema(&mut connection, database_path).ok()?;
    let dedupe_salt = load_or_create_dedupe_salt(&connection).ok()?;
    let today = utc_ranking_day(now);
    let cutoff = today - Duration::days(TOKEN_HISTORY_RETENTION_DAYS - 1);
    prune_expired_index(&connection, cutoff, today).ok()?;

    let transcripts_root = config_root.join("projects");
    let stored_files = load_file_summaries(&connection).ok()?;
    let transcript_source_missing = match fs::metadata(&transcripts_root) {
        Ok(metadata) => !metadata.is_dir(),
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    };
    if transcript_source_missing {
        connection
            .execute(
                "UPDATE claude_usage_files SET completion_state = 'missing'",
                [],
            )
            .ok()?;
        let aggregate_changed = refresh_daily_aggregates(&connection, cutoff, today, false).ok()?;
        prune_private_message_details(&connection, today).ok()?;
        return read_indexed_usage(
            &connection,
            cutoff,
            today,
            UsageScanStatus::Unavailable,
            false,
            false,
            aggregate_changed,
        )
        .ok();
    }
    let probe_exclusion =
        super::cli_probe::probe_transcript_exclusion(probe_directory, config_root);
    let probe_exclusion_safe = !matches!(
        &probe_exclusion,
        super::cli_probe::ProbeTranscriptExclusion::UnsafeProject(_)
    );
    let mut files = Vec::new();
    let traversal_complete =
        collect_transcript_files(&transcripts_root, &mut files, started, max_millis).is_ok();
    files.retain(|path| match &probe_exclusion {
        super::cli_probe::ProbeTranscriptExclusion::None => true,
        super::cli_probe::ProbeTranscriptExclusion::Exact(excluded) => path != excluded,
        super::cli_probe::ProbeTranscriptExclusion::UnsafeProject(project) => {
            !path.starts_with(project)
        }
    });
    let cutoff_modified_ns =
        i64::try_from(cutoff.midnight().assume_utc().unix_timestamp_nanos()).ok()?;
    let mut ordered_files = Vec::with_capacity(files.len());
    for path in files {
        let metadata = fs::metadata(&path).ok()?;
        let identity = file_identity(&metadata);
        let modified_ns = file_modified_ns(&metadata).ok()?;
        let path_value = path.to_string_lossy();
        let needs_work = modified_ns >= cutoff_modified_ns
            && stored_files
                .get(path_value.as_ref())
                .is_none_or(|stored| stored.needs_work(&identity, metadata.len(), modified_ns));
        ordered_files.push((needs_work, modified_ns, path));
    }
    ordered_files.sort_by_key(|entry| (!entry.0, std::cmp::Reverse(entry.1)));
    if traversal_complete {
        let present = ordered_files
            .iter()
            .map(|(_, _, path)| path.to_string_lossy().into_owned())
            .collect::<BTreeSet<_>>();
        for missing in stored_files.keys().filter(|path| !present.contains(*path)) {
            connection
                .execute(
                    "UPDATE claude_usage_files SET completion_state = 'missing' WHERE path = ?1",
                    [missing],
                )
                .ok()?;
        }
    }
    let mut remaining_bytes = max_bytes;
    let mut all_complete = traversal_complete;
    let mut failed = !probe_exclusion_safe;
    let file_scan_context = FileScanContext {
        connection: &connection,
        dedupe_salt: &dedupe_salt,
        cutoff,
        today,
        started,
        max_millis,
    };
    for (needs_work, _, path) in ordered_files.iter().filter(|entry| entry.0) {
        let _ = needs_work;
        if started.elapsed().as_millis() >= max_millis || remaining_bytes == 0 {
            all_complete = false;
            break;
        }
        let allowance = remaining_bytes.min(max_file_bytes);
        let mut file_remaining = allowance;
        let path_value = path.to_string_lossy();
        match index_file(
            &file_scan_context,
            path,
            stored_files.get(path_value.as_ref()),
            &mut file_remaining,
        ) {
            Ok(complete) => all_complete &= complete,
            Err(()) => {
                failed = true;
                all_complete = false;
            }
        }
        remaining_bytes = remaining_bytes.saturating_sub(allowance - file_remaining);
    }
    let (pending_files, error_files) = connection
        .query_row(
            "SELECT
               COALESCE(SUM(CASE WHEN completion_state = 'indexing' THEN 1 ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN completion_state IN ('error', 'missing') THEN 1 ELSE 0 END), 0)
             FROM claude_usage_files",
            [],
            |row| Ok((row.get::<_, u64>(0)?, row.get::<_, u64>(1)?)),
        )
        .unwrap_or((1, 1));
    let scan_status = if failed || error_files > 0 {
        UsageScanStatus::Unavailable
    } else if !all_complete || pending_files > 0 {
        UsageScanStatus::Indexing
    } else {
        UsageScanStatus::Complete
    };
    let aggregate_changed = refresh_daily_aggregates(
        &connection,
        cutoff,
        today,
        scan_status == UsageScanStatus::Complete,
    )
    .ok()?;
    prune_private_message_details(&connection, today).ok()?;
    debug_usage_event(&format!(
        "scan_completed status={scan_status:?} files={} bytes_read={} elapsed_ms={}",
        ordered_files.len(),
        max_bytes.saturating_sub(remaining_bytes),
        started.elapsed().as_millis()
    ));
    read_indexed_usage(
        &connection,
        cutoff,
        today,
        scan_status,
        traversal_complete && !failed && probe_exclusion_safe,
        true,
        aggregate_changed,
    )
    .ok()
}

#[cfg(test)]
fn stored_message_count(database_path: &Path) -> u64 {
    Connection::open(database_path)
        .unwrap()
        .query_row(
            "SELECT COUNT(DISTINCT message_key) FROM claude_usage_messages",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

#[cfg(test)]
fn stored_frame_count(database_path: &Path) -> u64 {
    Connection::open(database_path)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM claude_usage_frames", [], |row| {
            row.get(0)
        })
        .unwrap()
}

#[cfg(test)]
fn stored_supersede_edge_count(database_path: &Path) -> u64 {
    Connection::open(database_path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM claude_usage_message_supersedes",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

#[cfg(test)]
fn stored_file_count(database_path: &Path) -> u64 {
    Connection::open(database_path)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM claude_usage_files", [], |row| {
            row.get(0)
        })
        .unwrap()
}

#[cfg(test)]
fn stored_daily_revision(database_path: &Path, day: Date) -> u64 {
    Connection::open(database_path)
        .unwrap()
        .query_row(
            "SELECT revision FROM claude_usage_daily WHERE day = ?1",
            [day.to_string()],
            |row| row.get(0),
        )
        .unwrap()
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use time::{Duration, format_description::well_known::Rfc3339};

    use super::*;
    use crate::sanitized::{UsageCoverage, UsageEvidenceBasis, UsageTotal};

    const SALT: [u8; 32] = [0x5a; 32];
    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct FixtureRoot(PathBuf);

    impl FixtureRoot {
        fn new() -> Self {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "touchgrassbar-claude-usage-test-{}-{timestamp}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn database(&self) -> PathBuf {
            self.0.join("native.sqlite3")
        }

        fn config(&self) -> PathBuf {
            let path = self.0.join("config");
            fs::create_dir_all(path.join("projects")).unwrap();
            path
        }

        fn probe(&self) -> PathBuf {
            self.0.join("probe")
        }
    }

    impl Drop for FixtureRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn now() -> OffsetDateTime {
        OffsetDateTime::parse("2026-08-07T12:00:00Z", &Rfc3339).unwrap()
    }

    #[test]
    fn thirty_day_sync_fixture_keeps_each_day_revision_and_parser_correction() {
        let now = now();
        let first_day = now.date() - Duration::days(1);
        let second_day = now.date() - Duration::days(2);
        let usage = |tokens| DailyUsageEvidence {
            observed_tokens: tokens,
            coverage: UsageCoverage::Complete,
            observed_through: Some(now - Duration::minutes(1)),
        };
        let local = LocalUsageObservation {
            daily_usage: BTreeMap::from([(first_day, usage(100)), (second_day, usage(200))]),
            daily_cost: BTreeMap::new(),
            top_model_usage: None,
            pricing_basis: None,
            scan_status: UsageScanStatus::Complete,
            latest_pending_modified_at: None,
            latest_error_modified_at: None,
            scan_scope_known: true,
            transcript_source_present: true,
            aggregate_changed: true,
            daily_corrections: BTreeMap::from([
                (
                    first_day,
                    ProviderCorrection::ParserCorrection { source_revision: 2 },
                ),
                (
                    second_day,
                    ProviderCorrection::ParserCorrection { source_revision: 5 },
                ),
            ]),
            correction: None,
        };

        let evidence = provider_usage_evidence(Some(&local), now);
        let daily = calculate_daily_usage_aggregates(&evidence, now, now.date(), 30);

        assert_eq!(daily.len(), 2);
        assert_eq!(
            local.daily_corrections[&first_day],
            ProviderCorrection::ParserCorrection { source_revision: 2 }
        );
        assert_eq!(
            local.daily_corrections[&second_day],
            ProviderCorrection::ParserCorrection { source_revision: 5 }
        );
    }

    #[test]
    fn sqlite_history_loader_keeps_sparse_day_cost_and_correction_evidence() {
        let fixture = FixtureRoot::new();
        prepare_database(&fixture.database()).expect("the Claude index must prepare");
        let connection = Connection::open(fixture.database()).unwrap();
        let priced_day = now().date() - Duration::days(1);
        let unpriced_day = now().date() - Duration::days(3);
        connection
            .execute(
                "INSERT INTO claude_usage_daily(
                   day, observed_tokens, coverage, observed_through, revision,
                   priced_tokens, cost_usd, cost_modeled, pricing_basis,
                   pricing_fingerprint, correction_provenance,
                   correction_source_revision
                 ) VALUES(?1, 100, 'complete', ?2, 2, 100, 1.5, 0,
                          'anthropic-standard-2026-08-07-v1', 'fixture-price',
                          'parser-correction', 2)",
                params![
                    priced_day.to_string(),
                    (now() - Duration::minutes(1)).format(&Rfc3339).unwrap()
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO claude_usage_daily(
                   day, observed_tokens, coverage, observed_through, revision,
                   priced_tokens, cost_usd, cost_modeled, pricing_basis,
                   pricing_fingerprint, correction_provenance,
                   correction_source_revision
                 ) VALUES(?1, 40, 'partial', ?2, 1, 0, NULL, 0,
                          NULL, NULL, NULL, NULL)",
                params![
                    unpriced_day.to_string(),
                    (now() - Duration::minutes(2)).format(&Rfc3339).unwrap()
                ],
            )
            .unwrap();

        let history = load_daily_usage_history(&connection, now(), now().date(), 30)
            .expect("the stored sparse history must load");

        assert_eq!(history.len(), 2);
        let (_, priced, correction) = history
            .iter()
            .find(|(day, _, _)| *day == priced_day)
            .expect("the priced day must remain present");
        assert_eq!(
            *correction,
            Some(ProviderCorrection::ParserCorrection { source_revision: 2 })
        );
        let UsageTotal::Current {
            observed_tokens,
            api_equivalent_cost_usd,
            api_equivalent_cost_basis,
            ..
        } = priced
        else {
            panic!("the priced day must be current");
        };
        assert_eq!(*observed_tokens, 100);
        assert_eq!(*api_equivalent_cost_usd, Some(1.5));
        assert_eq!(
            api_equivalent_cost_basis.as_deref(),
            Some("anthropic-standard-2026-08-07-v1")
        );

        let (_, unpriced, correction) = history
            .iter()
            .find(|(day, _, _)| *day == unpriced_day)
            .expect("the unpriced sparse day must remain present");
        assert_eq!(*correction, None);
        let UsageTotal::Current {
            coverage,
            observed_tokens,
            api_equivalent_cost_usd,
            ..
        } = unpriced
        else {
            panic!("the sparse day must be current");
        };
        assert_eq!(*coverage, UsageCoverage::Partial);
        assert_eq!(*observed_tokens, 40);
        assert_eq!(*api_equivalent_cost_usd, None);
    }

    fn transcript_line(
        id: &str,
        timestamp: OffsetDateTime,
        model: &str,
        usage: ClaudeTokenUsage,
    ) -> String {
        let cache_creation = usage
            .cache_creation_5m_input
            .zip(usage.cache_creation_1h_input)
            .map(|(five_minutes, one_hour)| {
                format!(
                    r#", "cache_creation":{{"ephemeral_5m_input_tokens":{five_minutes},"ephemeral_1h_input_tokens":{one_hour}}}"#
                )
            })
            .unwrap_or_default();
        format!(
            r#"{{"type":"assistant","uuid":"frame-{id}","timestamp":"{}","version":"2.1.224","sessionId":"PRIVATE-SESSION","cwd":"/PRIVATE/PATH","message":{{"id":"{id}","type":"message","role":"assistant","model":"{model}","content":[{{"type":"text","text":"PRIVATE-CONTENT"}}],"usage":{{"input_tokens":{},"cache_creation_input_tokens":{},"cache_read_input_tokens":{},"output_tokens":{}{cache_creation},"server_tool_use":{{"web_search_requests":0,"web_fetch_requests":0}}}}}}}}"#,
            timestamp.format(&Rfc3339).unwrap(),
            usage.input,
            usage.cache_creation_input,
            usage.cache_read_input,
            usage.output,
        )
    }

    fn api_error_transcript_line() -> String {
        format!(
            r#"{{"cwd":"/PRIVATE/PATH","entrypoint":"cli","error":"PRIVATE-API-ERROR","gitBranch":"PRIVATE-BRANCH","isApiErrorMessage":true,"isSidechain":false,"message":{{"container":null,"content":[{{"text":"PRIVATE-CONTENT","type":"text"}}],"context_management":null,"diagnostics":null,"id":"PRIVATE-API-MESSAGE-ID","model":"<synthetic>","role":"assistant","stop_details":null,"stop_reason":"stop_sequence","stop_sequence":"","type":"message","usage":{{"cache_creation":{{"ephemeral_1h_input_tokens":0,"ephemeral_5m_input_tokens":0}},"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"inference_geo":null,"input_tokens":0,"iterations":null,"output_tokens":0,"output_tokens_details":null,"server_tool_use":{{"web_fetch_requests":0,"web_search_requests":0}},"service_tier":null,"speed":null}}}},"parentUuid":"PRIVATE-PARENT","sessionId":"PRIVATE-SESSION","session_id":"PRIVATE-SESSION","timestamp":"{}","type":"assistant","userType":"external","uuid":"PRIVATE-API-FRAME-ID","version":"2.1.241"}}"#,
            (now() - Duration::minutes(4)).format(&Rfc3339).unwrap()
        )
    }

    fn claude_code_2_1_241_transcript_line() -> String {
        format!(
            r#"{{"cwd":"/PRIVATE/PATH","effort":"medium","entrypoint":"cli","gitBranch":"PRIVATE-BRANCH","isSidechain":false,"message":{{"content":[{{"text":"PRIVATE-CONTENT","type":"text"}}],"diagnostics":null,"id":"PRIVATE-VALID-MESSAGE-ID","model":"claude-sonnet-4-5-20250929","role":"assistant","stop_details":null,"stop_reason":"end_turn","stop_sequence":null,"type":"message","usage":{{"cache_creation":{{"ephemeral_1h_input_tokens":0,"ephemeral_5m_input_tokens":20}},"cache_creation_input_tokens":20,"cache_read_input_tokens":30,"inference_geo":"not_available","input_tokens":10,"iterations":[{{"cache_creation":{{"ephemeral_1h_input_tokens":0,"ephemeral_5m_input_tokens":20}},"cache_creation_input_tokens":20,"cache_read_input_tokens":30,"input_tokens":10,"output_tokens":40,"type":"message"}}],"output_tokens":40,"output_tokens_details":{{"thinking_tokens":15}},"server_tool_use":{{"web_fetch_requests":0,"web_search_requests":0}},"service_tier":"standard","speed":"standard"}}}},"parentUuid":"PRIVATE-PARENT","requestId":"PRIVATE-REQUEST","sessionId":"PRIVATE-SESSION","session_id":"PRIVATE-SESSION","timestamp":"{}","type":"assistant","userType":"external","uuid":"PRIVATE-VALID-FRAME-ID","version":"2.1.241"}}"#,
            (now() - Duration::minutes(5)).format(&Rfc3339).unwrap()
        )
    }

    fn write_transcript(path: &Path, lines: &[String]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("{}\n", lines.join("\n"))).unwrap();
    }

    fn assert_sqlite_artifacts_exclude(database_path: &Path, forbidden: &str) {
        for suffix in ["", "-wal", "-shm"] {
            let mut artifact = database_path.as_os_str().to_os_string();
            artifact.push(suffix);
            let artifact = PathBuf::from(artifact);
            let Ok(bytes) = fs::read(artifact) else {
                continue;
            };
            assert!(
                !bytes
                    .windows(forbidden.len())
                    .any(|window| window == forbidden.as_bytes()),
                "private content reached a SQLite artifact"
            );
        }
    }

    fn set_modified_at(path: &Path, timestamp: OffsetDateTime) {
        let modified = std::time::SystemTime::UNIX_EPOCH
            + StdDuration::from_secs(u64::try_from(timestamp.unix_timestamp()).unwrap());
        let times = fs::FileTimes::new().set_modified(modified);
        fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(times)
            .unwrap();
    }

    fn usage(input: u64, cache_creation: u64, cache_read: u64, output: u64) -> ClaudeTokenUsage {
        ClaudeTokenUsage {
            input,
            cache_creation_input: cache_creation,
            cache_read_input: cache_read,
            output,
            cache_creation_5m_input: Some(cache_creation),
            cache_creation_1h_input: Some(0),
        }
    }

    fn pricing_catalog_with_standard_output_rate(
        model_name: &str,
        output_rate: f64,
    ) -> super::super::pricing::PricingCatalog {
        let mut manifest: serde_json::Value =
            serde_json::from_str(super::super::pricing::bundled_manifest_for_test()).unwrap();
        let model = manifest["models"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|model| model["name"].as_str() == Some(model_name))
            .unwrap();
        for period in model["standardPeriods"].as_array_mut().unwrap() {
            period["outputUsdPerMillion"] = serde_json::Value::from(output_rate);
        }
        super::super::pricing::catalog_from_manifest_for_test(
            &serde_json::to_string(&manifest).unwrap(),
        )
        .unwrap()
    }

    fn pricing_catalog_with_basis(basis: &str) -> super::super::pricing::PricingCatalog {
        let mut manifest: serde_json::Value =
            serde_json::from_str(super::super::pricing::bundled_manifest_for_test()).unwrap();
        manifest["basis"] = serde_json::Value::from(basis);
        super::super::pricing::catalog_from_manifest_for_test(
            &serde_json::to_string(&manifest).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn schema_migration_preserves_legacy_supersede_edges_and_forces_parser_rescan() {
        let fixture = FixtureRoot::new();
        let database = fixture.database();
        let mut connection = Connection::open(&database).unwrap();
        ensure_index_schema(&mut connection, &database).unwrap();
        connection
            .execute(
                "INSERT INTO claude_usage_messages(
                   frame_key, supersedes_frame_key, message_key, day, observed_at, model,
                   input_tokens, cache_creation_input_tokens, cache_read_input_tokens,
                   output_tokens, has_unknown_paid_server_tool, observed_tokens, complete,
                   parser_version
                 ) VALUES(
                   'replacement-key', 'superseded-key', 'message-key', '2026-08-07',
                   '2026-08-07T10:00:00Z', 'claude-sonnet-4-5-20250929',
                   1, 0, 0, 1, 0, 2, 1, 1
                 )",
                [],
            )
            .unwrap();
        connection
            .execute_batch(
                "DROP TABLE claude_usage_message_supersedes;
                 DROP TABLE claude_usage_frames;
                 UPDATE touchgrassbar_schema_versions
                 SET version = 1 WHERE module = 'claude-usage-index';",
            )
            .unwrap();

        ensure_index_schema(&mut connection, &database).unwrap();

        assert_eq!(
            usage_index_schema_version(&connection).unwrap(),
            USAGE_INDEX_SCHEMA_VERSION
        );
        assert_eq!(stored_supersede_edge_count(&database), 1);
        assert_eq!(stored_frame_count(&database), 1);
        assert_eq!(
            connection
                .query_row(
                    "SELECT aggregate_applied
                     FROM claude_usage_message_supersedes",
                    [],
                    |row| row.get::<_, u64>(0),
                )
                .unwrap(),
            1
        );
        assert!(usage_index_backup_path(&database, 1).is_file());
        let old_checkpoint = StoredFileSummary {
            identity: "device:inode".to_owned(),
            size: 10,
            modified_ns: 20,
            parsed_offset: 10,
            resume_anchor: Some("anchor".to_owned()),
            parser_version: TRANSCRIPT_PARSER_VERSION - 1,
            completion_state: "complete".to_owned(),
        };
        assert!(old_checkpoint.needs_work("device:inode", 10, 20));
    }

    #[test]
    fn schema_migration_adds_correction_fields_without_inventing_them() {
        let fixture = FixtureRoot::new();
        let database = fixture.database();
        let mut connection = Connection::open(&database).unwrap();
        ensure_index_schema(&mut connection, &database).unwrap();
        connection
            .execute_batch(
                "DROP TABLE claude_usage_daily;
                 CREATE TABLE claude_usage_daily (
                   day TEXT PRIMARY KEY NOT NULL,
                   observed_tokens INTEGER NOT NULL,
                   coverage TEXT NOT NULL CHECK (coverage IN ('complete', 'partial')),
                   observed_through TEXT NOT NULL,
                   revision INTEGER NOT NULL CHECK (revision >= 1),
                   priced_tokens INTEGER NOT NULL DEFAULT 0,
                   cost_usd REAL,
                   pricing_basis TEXT,
                   pricing_fingerprint TEXT
                 );
                 INSERT INTO claude_usage_daily(
                   day, observed_tokens, coverage, observed_through, revision
                 ) VALUES(
                   '2026-08-07', 40, 'complete', '2026-08-07T12:00:00Z', 3
                 );
                 UPDATE touchgrassbar_schema_versions
                 SET version = 3 WHERE module = 'claude-usage-index';",
            )
            .unwrap();

        ensure_index_schema(&mut connection, &database).unwrap();

        assert_eq!(
            usage_index_schema_version(&connection).unwrap(),
            USAGE_INDEX_SCHEMA_VERSION
        );
        let stored = connection
            .query_row(
                "SELECT observed_tokens, revision, cost_modeled,
                        correction_provenance, correction_source_revision
                 FROM claude_usage_daily WHERE day = '2026-08-07'",
                [],
                |row| {
                    Ok((
                        row.get::<_, u64>(0)?,
                        row.get::<_, u64>(1)?,
                        row.get::<_, bool>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<u64>>(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(stored, (40, 3, false, None, None));
        assert!(usage_index_backup_path(&database, 3).is_file());
    }

    #[test]
    fn schema_migration_discards_a_marker_without_its_source_revision() {
        let fixture = FixtureRoot::new();
        let database = fixture.database();
        let mut connection = Connection::open(&database).unwrap();
        ensure_index_schema(&mut connection, &database).unwrap();
        connection
            .execute_batch(
                "DROP TABLE claude_usage_daily;
                 CREATE TABLE claude_usage_daily (
                   day TEXT PRIMARY KEY NOT NULL,
                   observed_tokens INTEGER NOT NULL,
                   coverage TEXT NOT NULL CHECK (coverage IN ('complete', 'partial')),
                   observed_through TEXT NOT NULL,
                   revision INTEGER NOT NULL CHECK (revision >= 1),
                   priced_tokens INTEGER NOT NULL DEFAULT 0,
                   cost_usd REAL,
                   pricing_basis TEXT,
                   pricing_fingerprint TEXT,
                   correction_provenance TEXT CHECK (
                     correction_provenance IS NULL
                     OR correction_provenance = 'parser-correction'
                   )
                 );
                 INSERT INTO claude_usage_daily(
                   day, observed_tokens, coverage, observed_through, revision,
                   correction_provenance
                 ) VALUES(
                   '2026-08-07', 40, 'complete', '2026-08-07T12:00:00Z', 3,
                   'parser-correction'
                 );
                 UPDATE touchgrassbar_schema_versions
                 SET version = 4 WHERE module = 'claude-usage-index';",
            )
            .unwrap();

        ensure_index_schema(&mut connection, &database).unwrap();

        let stored = connection
            .query_row(
                "SELECT correction_provenance, correction_source_revision
                 FROM claude_usage_daily WHERE day = '2026-08-07'",
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<u64>>(1)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(stored, (None, None));
        assert!(usage_index_backup_path(&database, 4).is_file());
    }

    #[test]
    fn parser_counts_claude_categories_once_and_discards_content() {
        let line = br#"{
          "type":"assistant",
          "uuid":"PRIVATE-FRAME-ID",
          "timestamp":"2026-08-07T10:15:00Z",
          "version":"2.1.224",
          "sessionId":"PRIVATE-SESSION",
          "cwd":"/PRIVATE/PATH",
          "message":{
            "id":"PRIVATE-MESSAGE-ID",
            "type":"message",
            "role":"assistant",
            "model":"claude-sonnet-4-20250514",
            "content":[{"type":"thinking","thinking":"PRIVATE-CONTENT"}],
            "usage":{
              "input_tokens":10,
              "cache_creation_input_tokens":20,
              "cache_read_input_tokens":30,
              "output_tokens":40,
              "cache_creation":{
                "ephemeral_5m_input_tokens":20,
                "ephemeral_1h_input_tokens":0
              }
            }
          }
        }"#;

        let TranscriptLineOutcome::Usage(message) = parse_transcript_line(line, &SALT) else {
            panic!("supported assistant metadata must parse");
        };

        assert_eq!(message.usage.observed_tokens(), Some(100));
        assert_eq!(message.usage.output, 40);
        assert_eq!(message.day.to_string(), "2026-08-07");
        assert_eq!(message.model, "claude-sonnet-4-20250514");
        assert_eq!(message.pricing.web_search_requests, Some(0));
        assert_eq!(message.pricing.web_fetch_requests, Some(0));
        assert_eq!(message.pricing.code_execution_requests, None);
        assert!(!message.pricing.has_unknown_paid_server_tool);
        let reduced = format!("{message:?}");
        for forbidden in [
            "PRIVATE-CONTENT",
            "PRIVATE-MESSAGE-ID",
            "PRIVATE-SESSION",
            "/PRIVATE/PATH",
        ] {
            assert!(!reduced.contains(forbidden));
        }
    }

    #[test]
    fn parser_marks_unmodeled_nested_beta_usage_metadata_partial() {
        for (case, field) in [
            ("fallback credit", r#""fallback_credit":{"amount":1}"#),
            ("iteration details", r#""iterations":[{"index":1}]"#),
            (
                "output token details",
                r#""output_tokens_details":{"reasoning_tokens":2}"#,
            ),
        ] {
            let line = transcript_line(
                "fixture-unmodeled-beta-usage",
                now() - Duration::minutes(1),
                "claude-sonnet-4-5-20250929",
                usage(10, 20, 30, 40),
            )
            .replacen(
                "\"server_tool_use\"",
                &format!("{field},\"server_tool_use\""),
                1,
            );

            let TranscriptLineOutcome::Usage(message) =
                parse_transcript_line(line.as_bytes(), &SALT)
            else {
                panic!("{case} must retain known token evidence");
            };

            assert_eq!(message.usage.observed_tokens(), Some(100), "{case}");
            assert!(!message.complete, "{case}");
        }
    }

    #[test]
    fn unknown_usage_counter_is_partial_unpriced_and_absent_from_debug_output() {
        const UNKNOWN_KEY: &str = "future_private_output_tokens";
        const UNKNOWN_VALUE: &str = "918273645";

        let fixture = FixtureRoot::new();
        let config = fixture.config();
        let line = transcript_line(
            "fixture-unknown-beta-usage",
            now() - Duration::minutes(1),
            "claude-sonnet-4-5-20250929",
            usage(1_000_000, 0, 0, 1_000_000),
        )
        .replacen(
            r#""server_tool_use":{"web_search_requests":0,"web_fetch_requests":0}"#,
            &format!(
                r#""{UNKNOWN_KEY}":{UNKNOWN_VALUE},"server_tool_use":{{"web_search_requests":0,"web_fetch_requests":0}}"#
            ),
            1,
        );

        let TranscriptLineOutcome::Usage(message) = parse_transcript_line(line.as_bytes(), &SALT)
        else {
            panic!("an unknown usage counter must retain known token evidence");
        };
        assert_eq!(message.usage.observed_tokens(), Some(2_000_000));
        assert!(!message.complete);
        let normalized_debug = format!("{message:?}");
        assert!(!normalized_debug.contains(UNKNOWN_KEY));
        assert!(!normalized_debug.contains(UNKNOWN_VALUE));

        write_transcript(&config.join("projects/project-a/session.jsonl"), &[line]);
        let local = index_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
            .expect("an unknown usage counter must retain known token evidence");
        assert_eq!(local.daily_usage[&now().date()].observed_tokens, 2_000_000);
        assert_eq!(
            local.daily_usage[&now().date()].coverage,
            UsageCoverage::Partial
        );
        assert!(!local.daily_cost.contains_key(&now().date()));

        let report = debug_usage_report(&fixture.database(), &config, &fixture.probe(), now())
            .expect("partial usage must produce a sanitized debug report");
        assert!(report.contains("pricing_status=unavailable"));
        assert!(!report.contains(UNKNOWN_KEY));
        assert!(!report.contains(UNKNOWN_VALUE));
        assert_sqlite_artifacts_exclude(&fixture.database(), UNKNOWN_KEY);
        assert_sqlite_artifacts_exclude(&fixture.database(), UNKNOWN_VALUE);
    }

    #[test]
    fn unknown_nested_usage_fields_are_partial_and_unpriced() {
        const UNKNOWN_KEY: &str = "future_private_billing_counter";
        const UNKNOWN_VALUE: &str = "918273645";
        let cases = [
            (
                "cache-creation",
                r#""ephemeral_5m_input_tokens":20"#,
                format!(r#""{UNKNOWN_KEY}":{UNKNOWN_VALUE},"ephemeral_5m_input_tokens":20"#,),
            ),
            (
                "server-tool-use",
                r#""web_search_requests":0"#,
                format!(r#""{UNKNOWN_KEY}":{UNKNOWN_VALUE},"web_search_requests":0"#),
            ),
        ];

        for (case, marker, replacement) in cases {
            let fixture = FixtureRoot::new();
            let config = fixture.config();
            let line = transcript_line(
                &format!("fixture-unknown-nested-{case}"),
                now() - Duration::minutes(1),
                "claude-sonnet-4-5-20250929",
                usage(10, 20, 30, 40),
            )
            .replacen(marker, &replacement, 1);
            let TranscriptLineOutcome::Usage(message) =
                parse_transcript_line(line.as_bytes(), &SALT)
            else {
                panic!("unknown nested metadata must retain known token evidence");
            };
            assert_eq!(message.usage.observed_tokens(), Some(100));
            assert!(!message.complete);
            assert_eq!(
                message.pricing.has_unknown_paid_server_tool,
                case == "server-tool-use"
            );
            let normalized_debug = format!("{message:?}");
            assert!(!normalized_debug.contains(UNKNOWN_KEY));
            assert!(!normalized_debug.contains(UNKNOWN_VALUE));

            write_transcript(&config.join("projects/project-a/session.jsonl"), &[line]);
            let local = index_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
                .expect("unknown nested metadata must retain known token evidence");
            assert_eq!(local.daily_usage[&now().date()].observed_tokens, 100);
            assert_eq!(
                local.daily_usage[&now().date()].coverage,
                UsageCoverage::Partial
            );
            assert!(!local.daily_cost.contains_key(&now().date()));

            let report = debug_usage_report(&fixture.database(), &config, &fixture.probe(), now())
                .expect("partial usage must produce a sanitized debug report");
            assert!(report.contains("pricing_status=unavailable"));
            assert!(!report.contains(UNKNOWN_KEY));
            assert!(!report.contains(UNKNOWN_VALUE));
            assert_sqlite_artifacts_exclude(&fixture.database(), UNKNOWN_KEY);
            assert_sqlite_artifacts_exclude(&fixture.database(), UNKNOWN_VALUE);
        }
    }

    #[test]
    fn unknown_usage_metadata_storage_is_bounded() {
        let fields = (0..=MAX_UNKNOWN_USAGE_FIELDS)
            .map(|index| format!(r#""unknown_{index}":{index}"#))
            .collect::<Vec<_>>()
            .join(",");
        let unknown: BoundedUnknownUsageFields =
            serde_json::from_str(&format!("{{{fields}}}")).unwrap();

        assert_eq!(unknown.fields.len(), MAX_UNKNOWN_USAGE_FIELDS);
        assert!(unknown.overflowed);
        assert!(!unknown.is_empty());

        let long_key = "x".repeat(MAX_UNKNOWN_USAGE_KEY_BYTES + 1);
        let unknown: BoundedUnknownUsageFields =
            serde_json::from_str(&format!(r#"{{"{long_key}":1}}"#)).unwrap();

        assert!(unknown.fields.is_empty());
        assert!(unknown.overflowed);
        assert!(!unknown.is_empty());
    }

    #[test]
    fn parser_reduces_server_tool_blocks_to_bounded_pricing_signals() {
        for name in [
            "code_execution",
            "bash_code_execution",
            "text_editor_code_execution",
        ] {
            let line = transcript_line(
                "fixture-code-execution",
                now() - Duration::minutes(1),
                "claude-sonnet-4-5-20250929",
                usage(10, 0, 0, 40),
            )
            .replacen(
                r#"{"type":"text","text":"PRIVATE-CONTENT"}"#,
                &format!(
                    r#"{{"type":"server_tool_use","name":"{name}","input":{{"code":"PRIVATE-CONTENT"}}}}"#
                ),
                1,
            );
            let TranscriptLineOutcome::Usage(message) =
                parse_transcript_line(line.as_bytes(), &SALT)
            else {
                panic!("supported server-tool metadata must parse");
            };
            assert_eq!(message.pricing.code_execution_requests, Some(1));
            assert!(!message.pricing.has_unknown_paid_server_tool);
            assert!(!format!("{message:?}").contains("PRIVATE-CONTENT"));
        }

        let unknown = transcript_line(
            "fixture-unknown-server-tool",
            now() - Duration::minutes(1),
            "claude-sonnet-4-5-20250929",
            usage(10, 0, 0, 40),
        )
        .replacen(
            r#"{"type":"text","text":"PRIVATE-CONTENT"}"#,
            r#"{"type":"server_tool_use","name":"advisor","input":{"secret":"PRIVATE-CONTENT"}}"#,
            1,
        );
        let TranscriptLineOutcome::Usage(message) =
            parse_transcript_line(unknown.as_bytes(), &SALT)
        else {
            panic!("unknown server-tool metadata must retain token evidence");
        };
        assert_eq!(message.pricing.code_execution_requests, None);
        assert!(message.pricing.has_unknown_paid_server_tool);
        let reduced = format!("{message:?}");
        assert!(!reduced.contains("advisor"));
        assert!(!reduced.contains("PRIVATE-CONTENT"));
    }

    #[test]
    fn parser_bounds_supersedes_and_content_metadata_lists() {
        let superseded = (0..=MAX_SUPERSEDED_FRAMES)
            .map(|index| format!(r#""frame-{index}""#))
            .collect::<Vec<_>>()
            .join(",");
        let too_many_superseded = transcript_line(
            "fixture-too-many-superseded",
            now() - Duration::minutes(1),
            "claude-sonnet-4-5-20250929",
            usage(10, 0, 0, 40),
        )
        .replacen(
            r#""timestamp""#,
            &format!(r#""supersedes":[{superseded}],"timestamp""#),
            1,
        );
        assert_eq!(
            parse_transcript_line(too_many_superseded.as_bytes(), &SALT),
            TranscriptLineOutcome::Invalid
        );

        let content = std::iter::repeat_n(
            r#"{"type":"text","text":"PRIVATE-CONTENT"}"#,
            MAX_ASSISTANT_CONTENT_BLOCKS + 1,
        )
        .collect::<Vec<_>>()
        .join(",");
        let too_many_blocks = transcript_line(
            "fixture-too-many-blocks",
            now() - Duration::minutes(1),
            "claude-sonnet-4-5-20250929",
            usage(10, 0, 0, 40),
        )
        .replacen(r#"{"type":"text","text":"PRIVATE-CONTENT"}"#, &content, 1);
        let TranscriptLineOutcome::Usage(message) =
            parse_transcript_line(too_many_blocks.as_bytes(), &SALT)
        else {
            panic!("extra content blocks must retain token evidence");
        };
        assert!(message.pricing.has_unknown_paid_server_tool);
        assert!(!format!("{message:?}").contains("PRIVATE-CONTENT"));
    }

    #[test]
    fn parser_ignores_non_assistant_records_and_rejects_unknown_assistant_schemas() {
        let user = br#"{"type":"user","message":{"content":"PRIVATE-CONTENT"}}"#;
        assert_eq!(
            parse_transcript_line(user, &SALT),
            TranscriptLineOutcome::Ignored
        );

        let future = br#"{
          "type":"assistant",
          "uuid":"PRIVATE-FRAME-ID",
          "timestamp":"2026-08-07T10:15:00Z",
          "version":"3.0.0",
          "message":{
            "id":"PRIVATE-MESSAGE-ID",
            "type":"message",
            "role":"assistant",
            "model":"claude-future",
            "usage":{
              "input_tokens":1,
              "cache_creation_input_tokens":0,
              "cache_read_input_tokens":0,
              "output_tokens":1
            }
          }
        }"#;
        assert_eq!(
            parse_transcript_line(future, &SALT),
            TranscriptLineOutcome::Invalid
        );
    }

    #[test]
    fn parser_keeps_partial_tokens_when_cache_breakdown_is_invalid() {
        let line = br#"{
          "type":"assistant",
          "uuid":"PRIVATE-FRAME-ID",
          "timestamp":"2026-08-07T10:15:00Z",
          "version":"2.1.224",
          "message":{
            "id":"PRIVATE-MESSAGE-ID",
            "type":"message",
            "role":"assistant",
            "model":"claude-sonnet-4-20250514",
            "content":[],
            "usage":{
              "input_tokens":1,
              "cache_creation_input_tokens":2,
              "cache_read_input_tokens":3,
              "output_tokens":4,
              "cache_creation":{
                "ephemeral_5m_input_tokens":2,
                "ephemeral_1h_input_tokens":1
              }
            }
          }
        }"#;

        let TranscriptLineOutcome::Usage(message) = parse_transcript_line(line, &SALT) else {
            panic!("valid top-level counters must remain partial evidence");
        };
        assert_eq!(message.usage.observed_tokens(), Some(10));
        assert_eq!(message.usage.cache_creation_5m_input, None);
        assert_eq!(message.usage.cache_creation_1h_input, None);
        assert!(!message.complete);
    }

    #[test]
    fn parser_accepts_verified_claude_code_2_1_223_schema() {
        let line = transcript_line(
            "verified-previous-patch",
            now(),
            "claude-sonnet-4-20250514",
            ClaudeTokenUsage {
                input: 10,
                cache_creation_input: 20,
                cache_read_input: 30,
                output: 40,
                cache_creation_5m_input: Some(20),
                cache_creation_1h_input: Some(0),
            },
        )
        .replace("\"version\":\"2.1.224\"", "\"version\":\"2.1.223\"");

        let TranscriptLineOutcome::Usage(message) = parse_transcript_line(line.as_bytes(), &SALT)
        else {
            panic!("the verified Claude Code 2.1.223 schema must be accepted");
        };
        assert_eq!(message.usage.observed_tokens(), Some(100));
        assert!(message.complete);
    }

    #[test]
    fn parser_rejects_unverified_versions_and_marks_nullable_cache_counters_partial() {
        let future_patch = br#"{
          "type":"assistant",
          "uuid":"PRIVATE-FRAME-ID",
          "timestamp":"2026-08-07T10:15:00Z",
          "version":"2.1.225",
          "message":{
            "id":"PRIVATE-MESSAGE-ID",
            "type":"message",
            "role":"assistant",
            "model":"claude-sonnet-4-20250514",
            "content":[],
            "usage":{
              "input_tokens":10,
              "cache_creation_input_tokens":0,
              "cache_read_input_tokens":0,
              "output_tokens":40
            }
          }
        }"#;
        assert_eq!(
            parse_transcript_line(future_patch, &SALT),
            TranscriptLineOutcome::Invalid
        );

        let nullable = br#"{
          "type":"assistant",
          "uuid":"PRIVATE-FRAME-ID",
          "timestamp":"2026-08-07T10:15:00Z",
          "version":"2.1.224",
          "message":{
            "id":"PRIVATE-MESSAGE-ID",
            "type":"message",
            "role":"assistant",
            "model":"claude-sonnet-4-20250514",
            "content":[],
            "usage":{
              "input_tokens":10,
              "cache_creation_input_tokens":null,
              "cache_read_input_tokens":null,
              "output_tokens":40
            }
          }
        }"#;
        let TranscriptLineOutcome::Usage(message) = parse_transcript_line(nullable, &SALT) else {
            panic!("known nullable metadata must retain partial token evidence");
        };
        assert_eq!(message.usage.observed_tokens(), Some(50));
        assert!(!message.complete);
    }

    #[test]
    fn parser_ignores_only_the_reviewed_zero_usage_api_error_shape() {
        let reviewed = api_error_transcript_line();
        assert_eq!(
            parse_transcript_line(reviewed.as_bytes(), &SALT),
            TranscriptLineOutcome::Ignored
        );

        for unreviewed in [
            reviewed.replacen("\"input_tokens\":0", "\"input_tokens\":1", 1),
            reviewed.replacen("\"error\":\"PRIVATE-API-ERROR\",", "", 1),
            reviewed.replacen("<synthetic>", "<different>", 1),
            reviewed.replacen(
                "\"input_tokens\":0",
                "\"future_paid_tokens\":0,\"input_tokens\":0",
                1,
            ),
            reviewed.replacen("\"iterations\":null", "\"iterations\":[]", 1),
            reviewed.replacen(
                "\"text\":\"PRIVATE-CONTENT\",\"type\":\"text\"",
                "\"future_cost\":0,\"text\":\"PRIVATE-CONTENT\",\"type\":\"text\"",
                1,
            ),
            reviewed.replacen(
                "\"timestamp\"",
                "\"supersedes\":[\"frame-other\"],\"timestamp\"",
                1,
            ),
        ] {
            assert!(matches!(
                parse_transcript_line(unreviewed.as_bytes(), &SALT),
                TranscriptLineOutcome::FrameOnly(_)
            ));
        }
    }

    #[test]
    fn parser_marks_aborted_wrappers_partial() {
        let aborted = br#"{
          "type":"assistant",
          "uuid":"PRIVATE-FRAME-ID",
          "timestamp":"2026-08-07T10:15:00Z",
          "version":"2.1.224",
          "aborted":true,
          "message":{
            "id":"PRIVATE-MESSAGE-ID",
            "type":"message",
            "role":"assistant",
            "model":"claude-sonnet-4-20250514",
            "content":[],
            "usage":{
              "input_tokens":10,
              "cache_creation_input_tokens":0,
              "cache_read_input_tokens":0,
              "output_tokens":40
            }
          }
        }"#;
        let TranscriptLineOutcome::Usage(message) = parse_transcript_line(aborted, &SALT) else {
            panic!("aborted wrappers must retain partial token evidence");
        };
        assert_eq!(message.usage.observed_tokens(), Some(50));
        assert!(!message.complete);
    }

    #[test]
    fn conflicting_message_copies_keep_the_larger_whole_record_as_partial() {
        let smaller = StoredMessage {
            day: now().date() - Duration::days(1),
            observed_at: now() - Duration::days(1),
            model: "claude-sonnet-4-5-20250929".to_owned(),
            usage: usage(40, 0, 0, 0),
            pricing: ClaudePricingMetadata::default(),
            observed_tokens: 40,
            details_retained: true,
            complete: true,
        };
        let larger = StoredMessage {
            day: now().date(),
            observed_at: now(),
            model: "claude-sonnet-4-5-20250929".to_owned(),
            usage: usage(100, 0, 0, 0),
            pricing: ClaudePricingMetadata::default(),
            observed_tokens: 100,
            details_retained: true,
            complete: true,
        };

        let merged = merge_provider_message(smaller, larger);

        assert_eq!(merged.day, now().date());
        assert_eq!(merged.usage.observed_tokens(), Some(100));
        assert!(!merged.complete);
    }

    #[test]
    fn index_deduplicates_provider_messages_globally() {
        let fixture = FixtureRoot::new();
        let config = fixture.config();
        let message = transcript_line(
            "fixture-message-a",
            now() - Duration::minutes(5),
            "claude-sonnet-4-20250514",
            usage(10, 20, 30, 40),
        );
        write_transcript(
            &config.join("projects/project-a/session.jsonl"),
            std::slice::from_ref(&message),
        );
        write_transcript(
            &config.join("projects/project-a/subagents/agent.jsonl"),
            &[message],
        );

        let local = index_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
            .expect("synthetic transcript usage must index");
        let periods = project_usage_periods(Some(&local), now());
        let UsageTotal::Current {
            evidence_basis,
            coverage,
            observed_tokens,
            ..
        } = periods.today
        else {
            panic!("today must be available");
        };
        assert_eq!(evidence_basis, UsageEvidenceBasis::LocallyDerived);
        assert_eq!(coverage, UsageCoverage::Complete);
        assert_eq!(observed_tokens, 100);
        assert_eq!(stored_message_count(&fixture.database()), 1);
    }

    #[test]
    fn scan_counts_reviewed_claude_code_2_1_241_usage_once_and_prices_it() {
        let fixture = FixtureRoot::new();
        let config = fixture.config();
        let message = claude_code_2_1_241_transcript_line();
        let TranscriptLineOutcome::Usage(parsed) = parse_transcript_line(message.as_bytes(), &SALT)
        else {
            panic!("the reviewed Claude Code 2.1.241 schema must parse");
        };
        assert_eq!(parsed.usage, usage(10, 20, 30, 40));
        assert_eq!(parsed.usage.observed_tokens(), Some(100));
        assert!(parsed.complete);
        write_transcript(&config.join("projects/project-a/session.jsonl"), &[message]);

        let local = scan_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
            .expect("known Claude Code 2.1.241 tokens must publish");
        let UsageTotal::Current {
            api_equivalent_cost_usd,
            evidence_basis,
            coverage,
            observed_tokens,
            ..
        } = project_usage_periods(Some(&local), now()).today
        else {
            panic!("today must be available");
        };

        assert_eq!(local.scan_status, UsageScanStatus::Complete);
        assert_eq!(evidence_basis, UsageEvidenceBasis::LocallyDerived);
        assert_eq!(coverage, UsageCoverage::Complete);
        assert_eq!(observed_tokens, 100);
        assert_eq!(api_equivalent_cost_usd, Some(0.000_714));
    }

    #[test]
    fn scan_keeps_unreviewed_2_1_241_extended_usage_partial_and_unpriced() {
        let reviewed = claude_code_2_1_241_transcript_line();
        let second_iteration = r#",{"cache_creation":{"ephemeral_1h_input_tokens":0,"ephemeral_5m_input_tokens":20},"cache_creation_input_tokens":20,"cache_read_input_tokens":30,"input_tokens":10,"output_tokens":40,"type":"message"}"#;
        for (case, message) in [
            (
                "fallback credit",
                reviewed.replacen(
                    r#""inference_geo""#,
                    r#""fallback_credit":{"amount":1},"inference_geo""#,
                    1,
                ),
            ),
            (
                "mismatched iteration counter",
                reviewed.replacen(
                    r#""output_tokens":40,"type":"message""#,
                    r#""output_tokens":41,"type":"message""#,
                    1,
                ),
            ),
            (
                "unknown iteration field",
                reviewed.replacen(
                    r#""output_tokens":40,"type":"message""#,
                    r#""future_tokens":1,"output_tokens":40,"type":"message""#,
                    1,
                ),
            ),
            (
                "more than one iteration",
                reviewed.replacen(
                    r#"}],"output_tokens":40"#,
                    &format!(r#"}}{second_iteration}],"output_tokens":40"#),
                    1,
                ),
            ),
            (
                "thinking exceeds output",
                reviewed.replacen(r#""thinking_tokens":15"#, r#""thinking_tokens":41"#, 1),
            ),
            (
                "unknown output-token detail",
                reviewed.replacen(
                    r#""thinking_tokens":15"#,
                    r#""future_tokens":1,"thinking_tokens":15"#,
                    1,
                ),
            ),
            (
                "unreviewed Claude Code version",
                reviewed.replacen(r#""version":"2.1.241""#, r#""version":"2.1.224""#, 1),
            ),
        ] {
            let TranscriptLineOutcome::Usage(parsed) =
                parse_transcript_line(message.as_bytes(), &SALT)
            else {
                panic!("{case} must retain known top-level token evidence");
            };
            assert_eq!(parsed.usage.observed_tokens(), Some(100), "{case}");
            assert!(!parsed.complete, "{case}");

            let fixture = FixtureRoot::new();
            let config = fixture.config();
            write_transcript(&config.join("projects/project-a/session.jsonl"), &[message]);
            let local = index_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
                .expect("known top-level token evidence must index");
            assert_eq!(
                local.daily_usage[&now().date()].coverage,
                UsageCoverage::Partial,
                "{case}"
            );
            assert!(!local.daily_cost.contains_key(&now().date()), "{case}");
        }
    }

    #[test]
    fn scan_ignores_a_reviewed_api_error_without_losing_valid_usage() {
        let fixture = FixtureRoot::new();
        let config = fixture.config();
        let valid = transcript_line(
            "fixture-valid-before-api-error",
            now() - Duration::minutes(5),
            "claude-sonnet-4-20250514",
            usage(10, 20, 30, 40),
        )
        .replacen("\"version\":\"2.1.224\"", "\"version\":\"2.1.241\"", 1);
        write_transcript(
            &config.join("projects/project-a/session.jsonl"),
            &[valid, api_error_transcript_line()],
        );

        let local = scan_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
            .expect("the reviewed API error must not withhold valid usage");
        let UsageTotal::Current {
            coverage,
            observed_tokens,
            ..
        } = project_usage_periods(Some(&local), now()).today
        else {
            panic!("today must be available");
        };

        assert_eq!(local.scan_status, UsageScanStatus::Complete);
        assert_eq!(coverage, UsageCoverage::Complete);
        assert_eq!(observed_tokens, 100);
        assert_eq!(stored_message_count(&fixture.database()), 1);
        assert_eq!(stored_frame_count(&fixture.database()), 1);
        let report = debug_usage_report(&fixture.database(), &config, &fixture.probe(), now())
            .expect("the sanitized Claude report must render");
        for forbidden in [
            "/PRIVATE/PATH",
            "PRIVATE-API-ERROR",
            "PRIVATE-API-FRAME-ID",
            "PRIVATE-API-MESSAGE-ID",
            "PRIVATE-BRANCH",
            "PRIVATE-CONTENT",
            "PRIVATE-PARENT",
            "PRIVATE-SESSION",
        ] {
            assert_sqlite_artifacts_exclude(&fixture.database(), forbidden);
            assert!(!report.contains(forbidden));
        }
    }

    #[test]
    fn parser_upgrade_reindexes_the_previous_2_1_241_checkpoint_once() {
        let fixture = FixtureRoot::new();
        let config = fixture.config();
        let database = fixture.database();
        let transcript = config.join("projects/project-a/session.jsonl");
        let message = claude_code_2_1_241_transcript_line();
        write_transcript(&transcript, std::slice::from_ref(&message));
        prepare_database(&database).expect("the Claude index must prepare");
        let metadata = fs::metadata(&transcript).unwrap();
        let resume_anchor = file_resume_anchor(&transcript, metadata.len()).unwrap();
        let connection = Connection::open(&database).unwrap();
        let dedupe_salt = load_or_create_dedupe_salt(&connection).unwrap();
        let TranscriptLineOutcome::Usage(previous_message) =
            parse_transcript_line(message.as_bytes(), &dedupe_salt)
        else {
            panic!("the reviewed Claude Code 2.1.241 fixture must parse");
        };
        let day = previous_message.day.to_string();
        let observed_at = previous_message.observed_at.format(&Rfc3339).unwrap();
        let transaction = connection.unchecked_transaction().unwrap();
        transaction
            .execute(
                "INSERT INTO claude_usage_files(
                   path, file_identity, size_bytes, modified_ns, parsed_offset,
                   resume_anchor, parser_version, completion_state
                 ) VALUES(?1, ?2, ?3, ?4, ?3, ?5, 6, 'complete')",
                params![
                    transcript.to_string_lossy().as_ref(),
                    file_identity(&metadata),
                    i64::try_from(metadata.len()).unwrap(),
                    file_modified_ns(&metadata).unwrap(),
                    resume_anchor,
                ],
            )
            .unwrap();
        transaction
            .execute(
                "INSERT INTO claude_usage_frames(
                   frame_key, day, observed_at, parser_version
                 ) VALUES(?1, ?2, ?3, 6)",
                params![previous_message.frame_key, day, observed_at],
            )
            .unwrap();
        transaction
            .execute(
                "INSERT INTO claude_usage_messages(
                   frame_key, supersedes_frame_key, message_key, day, observed_at, model,
                   input_tokens, cache_creation_input_tokens, cache_read_input_tokens,
                   output_tokens, cache_creation_5m_input_tokens,
                   cache_creation_1h_input_tokens, service_tier, inference_geo, speed,
                   web_search_requests, web_fetch_requests, code_execution_requests,
                   has_unknown_paid_server_tool, observed_tokens, complete, parser_version
                 ) VALUES(
                   ?1, NULL, ?2, ?3, ?4, 'claude-sonnet-4-5-20250929',
                   10, 20, 30, 40, 20, 0, 'standard', 'not_available', 'standard',
                   0, 0, NULL, 0, 100, 0, 6
                 )",
                params![
                    previous_message.frame_key,
                    previous_message.message_key,
                    day,
                    observed_at,
                ],
            )
            .unwrap();
        transaction
            .execute(
                "INSERT INTO claude_usage_daily(
                   day, observed_tokens, coverage, observed_through, revision,
                   priced_tokens, cost_usd, cost_modeled, pricing_basis,
                   pricing_fingerprint, correction_provenance,
                   correction_source_revision
                 ) VALUES(
                   ?1, 100, 'partial', ?2, 4, 0, NULL, 0, NULL, NULL, NULL, NULL
                 )",
                params![day, observed_at],
            )
            .unwrap();
        transaction
            .execute(
                "INSERT INTO claude_usage_index_meta(key, value) VALUES(?1, '6')",
                [USAGE_AGGREGATE_PARSER_VERSION_KEY],
            )
            .unwrap();
        transaction.commit().unwrap();

        let previous = connection
            .query_row(
                "SELECT complete, parser_version FROM claude_usage_messages",
                [],
                |row| Ok((row.get::<_, bool>(0)?, row.get::<_, i64>(1)?)),
            )
            .unwrap();
        assert_eq!(previous, (false, 6));
        let previous_daily = connection
            .query_row(
                "SELECT coverage, priced_tokens, cost_usd, revision
                 FROM claude_usage_daily WHERE day = ?1",
                [&day],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, u64>(1)?,
                        row.get::<_, Option<f64>>(2)?,
                        row.get::<_, u64>(3)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(previous_daily, ("partial".to_owned(), 0, None, 4));
        drop(connection);

        let local = scan_local_usage_at(&database, &config, &fixture.probe(), now())
            .expect("the parser update must restart the previous checkpoint");
        let UsageTotal::Current {
            api_equivalent_cost_usd,
            coverage,
            observed_tokens,
            ..
        } = project_usage_periods(Some(&local), now()).today
        else {
            panic!("today must be available");
        };

        assert_eq!(local.scan_status, UsageScanStatus::Complete);
        assert_eq!(coverage, UsageCoverage::Complete);
        assert_eq!(observed_tokens, 100);
        assert_eq!(api_equivalent_cost_usd, Some(0.000_714));
        assert!(local.aggregate_changed);
        assert_eq!(local.correction, None);
        let connection = Connection::open(database).unwrap();
        assert_eq!(
            stored_usage_aggregate_parser_version(&connection).unwrap(),
            Some(TRANSCRIPT_PARSER_VERSION)
        );
        assert_eq!(stored_message_count(&fixture.database()), 1);
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM claude_usage_messages", [], |row| {
                    row.get::<_, u64>(0)
                })
                .unwrap(),
            1
        );
        let current_message = connection
            .query_row(
                "SELECT complete, parser_version FROM claude_usage_messages",
                [],
                |row| Ok((row.get::<_, bool>(0)?, row.get::<_, i64>(1)?)),
            )
            .unwrap();
        assert_eq!(current_message, (true, TRANSCRIPT_PARSER_VERSION));
        let current_daily = connection
            .query_row(
                "SELECT coverage, priced_tokens, cost_usd, revision
                 FROM claude_usage_daily WHERE day = ?1",
                [&day],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, u64>(1)?,
                        row.get::<_, Option<f64>>(2)?,
                        row.get::<_, u64>(3)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            current_daily,
            ("complete".to_owned(), 100, Some(0.000_714), 5)
        );
        let first_revision = stored_daily_revision(&fixture.database(), now().date());
        drop(connection);

        let unchanged = scan_local_usage_at(
            &fixture.database(),
            &config,
            &fixture.probe(),
            now() + Duration::minutes(1),
        )
        .expect("a complete unchanged scan must retain the indexed usage");
        assert!(
            !unchanged.aggregate_changed,
            "an unchanged checkpoint must not apply the same parser correction twice"
        );
        assert_eq!(stored_message_count(&fixture.database()), 1);
        assert_eq!(
            stored_daily_revision(&fixture.database(), now().date()),
            first_revision
        );
        let connection = Connection::open(fixture.database()).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM claude_usage_messages", [], |row| {
                    row.get::<_, u64>(0)
                })
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT coverage, priced_tokens, cost_usd, revision
                     FROM claude_usage_daily WHERE day = ?1",
                    [&day],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, u64>(1)?,
                            row.get::<_, Option<f64>>(2)?,
                            row.get::<_, u64>(3)?,
                        ))
                    },
                )
                .unwrap(),
            current_daily
        );
    }

    #[test]
    fn explicit_root_scan_withholds_only_a_missing_transcript_source() {
        let fixture = FixtureRoot::new();
        let config = fixture.config();
        write_transcript(
            &config.join("projects/project-a/session.jsonl"),
            &[
                transcript_line(
                    "fixture-valid-message",
                    now() - Duration::minutes(5),
                    "claude-sonnet-4-5-20250929",
                    usage(10, 20, 30, 40),
                ),
                "{invalid-json".to_owned(),
            ],
        );

        let partial = scan_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
            .expect("a parser failure must retain valid lower-bound evidence");
        assert_eq!(partial.scan_status, UsageScanStatus::Unavailable);
        assert_eq!(partial.daily_usage[&now().date()].observed_tokens, 100);
        assert_eq!(
            partial.daily_usage[&now().date()].coverage,
            UsageCoverage::Partial
        );

        fs::remove_dir_all(config.join("projects")).unwrap();
        assert!(
            scan_local_usage_at(
                &fixture.database(),
                &config,
                &fixture.probe(),
                now() + Duration::minutes(1),
            )
            .is_none(),
            "a missing transcript source must preserve the adapter cache"
        );
    }

    #[test]
    fn unchanged_partial_scan_publishes_only_after_daily_aggregate_changes() {
        let fixture = FixtureRoot::new();
        let config = fixture.config();
        let transcript = config.join("projects/project-a/session.jsonl");
        let first_message = transcript_line(
            "fixture-partial-first",
            now() - Duration::minutes(5),
            "claude-sonnet-4-5-20250929",
            usage(100, 0, 0, 0),
        );
        let invalid = "{invalid-json".to_owned();
        write_transcript(&transcript, &[first_message.clone(), invalid.clone()]);

        let first = scan_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
            .expect("new partial evidence must publish once");
        assert_eq!(first.scan_status, UsageScanStatus::Unavailable);
        assert!(first.aggregate_changed);
        assert_eq!(first.daily_usage[&now().date()].observed_tokens, 100);

        assert!(
            scan_local_usage_at(
                &fixture.database(),
                &config,
                &fixture.probe(),
                now() + Duration::minutes(1),
            )
            .is_none(),
            "an unchanged partial retry must retain the older cache observation time"
        );
        let internal = index_local_usage_at(
            &fixture.database(),
            &config,
            &fixture.probe(),
            now() + Duration::minutes(2),
        )
        .expect("the debug index path must keep the internal partial observation");
        assert_eq!(internal.daily_usage[&now().date()].observed_tokens, 100);
        assert!(!internal.aggregate_changed);

        let appended_message = transcript_line(
            "fixture-partial-appended",
            now() - Duration::minutes(1),
            "claude-sonnet-4-5-20250929",
            usage(50, 0, 0, 0),
        );
        write_transcript(&transcript, &[first_message, invalid, appended_message]);
        let extended = scan_local_usage_at(
            &fixture.database(),
            &config,
            &fixture.probe(),
            now() + Duration::minutes(3),
        )
        .expect("larger partial evidence must publish once");
        assert_eq!(extended.scan_status, UsageScanStatus::Unavailable);
        assert!(extended.aggregate_changed);
        assert_eq!(extended.daily_usage[&now().date()].observed_tokens, 150);

        assert!(
            scan_local_usage_at(
                &fixture.database(),
                &config,
                &fixture.probe(),
                now() + Duration::minutes(4),
            )
            .is_none(),
            "the larger partial total must not refresh again without a change"
        );
    }

    #[test]
    fn index_deduplicates_different_frames_for_one_provider_message() {
        let fixture = FixtureRoot::new();
        let config = fixture.config();
        let larger = transcript_line(
            "fixture-shared-message",
            now() - Duration::minutes(5),
            "claude-sonnet-4-20250514",
            usage(100, 0, 0, 0),
        )
        .replacen(
            "\"uuid\":\"frame-fixture-shared-message\"",
            "\"uuid\":\"frame-copy-a\"",
            1,
        );
        let smaller = transcript_line(
            "fixture-shared-message",
            now() - Duration::minutes(4),
            "claude-sonnet-4-20250514",
            usage(40, 0, 0, 0),
        )
        .replacen(
            "\"uuid\":\"frame-fixture-shared-message\"",
            "\"uuid\":\"frame-copy-b\"",
            1,
        );
        write_transcript(&config.join("projects/project-a/session.jsonl"), &[larger]);
        write_transcript(
            &config.join("projects/project-b/subagents/agent.jsonl"),
            &[smaller],
        );

        let local = index_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
            .expect("different frames for one provider message must index");

        assert_eq!(local.daily_usage[&now().date()].observed_tokens, 100);
        assert_eq!(
            local.daily_usage[&now().date()].coverage,
            UsageCoverage::Partial
        );
        assert_eq!(stored_message_count(&fixture.database()), 1);
        assert_eq!(stored_frame_count(&fixture.database()), 2);
    }

    #[test]
    fn index_excludes_superseded_frames_before_message_deduplication() {
        let fixture = FixtureRoot::new();
        let config = fixture.config();
        let old = transcript_line(
            "fixture-old-message",
            now() - Duration::minutes(10),
            "claude-sonnet-4-20250514",
            usage(100, 0, 0, 0),
        );
        let new = transcript_line(
            "fixture-new-message",
            now() - Duration::minutes(5),
            "claude-sonnet-4-20250514",
            usage(40, 0, 0, 0),
        )
        .replacen(
            "\"timestamp\"",
            "\"supersedes\":[\"frame-fixture-old-message\"],\"timestamp\"",
            1,
        );
        write_transcript(
            &config.join("projects/project-a/session.jsonl"),
            &[old, new],
        );

        let local = index_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
            .expect("superseding metadata must index");
        let UsageTotal::Current {
            observed_tokens, ..
        } = project_usage_periods(Some(&local), now()).today
        else {
            panic!("today must be available");
        };
        assert_eq!(observed_tokens, 40);
        assert_eq!(stored_supersede_edge_count(&fixture.database()), 1);
    }

    #[test]
    fn correction_source_survives_increase_and_accepts_a_late_older_superseder() {
        let fixture = FixtureRoot::new();
        let config = fixture.config();
        let transcript = config.join("projects/project-a/session.jsonl");
        let old = transcript_line(
            "fixture-staged-old",
            now() - Duration::minutes(10),
            "claude-sonnet-4-20250514",
            usage(100, 0, 0, 0),
        );
        write_transcript(&transcript, std::slice::from_ref(&old));
        let first = index_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
            .expect("the original frame must index");
        assert_eq!(first.daily_usage[&now().date()].observed_tokens, 100);
        assert_eq!(first.correction, None);
        let first_revision = stored_daily_revision(&fixture.database(), now().date());

        let replacement = transcript_line(
            "fixture-staged-new",
            now() - Duration::minutes(5),
            "claude-sonnet-4-20250514",
            usage(40, 0, 0, 0),
        )
        .replacen(
            "\"timestamp\"",
            "\"supersedes\":[\"frame-fixture-staged-old\"],\"timestamp\"",
            1,
        );
        write_transcript(&transcript, &[old.clone(), replacement.clone()]);
        let corrected = index_local_usage_at(
            &fixture.database(),
            &config,
            &fixture.probe(),
            now() + Duration::minutes(1),
        )
        .expect("a complete supersede scan must replace the old aggregate");

        assert_eq!(corrected.scan_status, UsageScanStatus::Complete);
        assert_eq!(corrected.daily_usage[&now().date()].observed_tokens, 40);
        let correction_revision = stored_daily_revision(&fixture.database(), now().date());
        assert_eq!(
            corrected.correction,
            Some(ProviderCorrection::ParserCorrection {
                source_revision: correction_revision
            })
        );
        assert!(correction_revision > first_revision);
        assert_eq!(stored_supersede_edge_count(&fixture.database()), 1);

        let retried = index_local_usage_at(
            &fixture.database(),
            &config,
            &fixture.probe(),
            now() + Duration::minutes(2),
        )
        .expect("the correction marker must survive an unchanged retry");
        assert_eq!(
            retried.correction,
            Some(ProviderCorrection::ParserCorrection {
                source_revision: correction_revision
            })
        );

        let ordinary = transcript_line(
            "fixture-staged-ordinary",
            now() - Duration::minutes(3),
            "claude-sonnet-4-20250514",
            usage(10, 0, 0, 0),
        );
        write_transcript(&transcript, &[old.clone(), replacement.clone(), ordinary]);
        let increased = index_local_usage_at(
            &fixture.database(),
            &config,
            &fixture.probe(),
            now() + Duration::minutes(3),
        )
        .expect("an ordinary increase must keep the correction source revision");
        let increased_revision = stored_daily_revision(&fixture.database(), now().date());
        assert_eq!(increased.daily_usage[&now().date()].observed_tokens, 50);
        assert!(increased_revision > correction_revision);
        assert_eq!(
            increased.correction,
            Some(ProviderCorrection::ParserCorrection {
                source_revision: correction_revision
            })
        );

        let connection = Connection::open(fixture.database()).unwrap();
        connection
            .execute(
                "DELETE FROM claude_usage_frames
                 WHERE frame_key IN (
                   SELECT frame_key FROM claude_usage_messages
                   WHERE day = ?1 AND observed_tokens = 10
                 )",
                [now().date().to_string()],
            )
            .unwrap();
        connection
            .execute(
                "DELETE FROM claude_usage_messages
                 WHERE day = ?1 AND observed_tokens = 10",
                [now().date().to_string()],
            )
            .unwrap();
        let cutoff = now().date() - Duration::days(TOKEN_HISTORY_RETENTION_DAYS - 1);
        assert_eq!(
            load_active_provider_messages(&connection, cutoff, now().date())
                .unwrap()
                .into_iter()
                .map(|message| message.observed_tokens)
                .sum::<u64>(),
            40
        );
        refresh_daily_aggregates(&connection, cutoff, now().date(), true).unwrap();
        let unproved_decrease = read_indexed_usage(
            &connection,
            cutoff,
            now().date(),
            UsageScanStatus::Complete,
            true,
            true,
            false,
        )
        .expect("an unproved decrease must keep the accepted lower bound");
        assert_eq!(
            unproved_decrease.daily_usage[&now().date()].observed_tokens,
            50
        );
        assert_eq!(
            stored_daily_revision(&fixture.database(), now().date()),
            increased_revision
        );
        assert_eq!(
            unproved_decrease.correction,
            Some(ProviderCorrection::ParserCorrection {
                source_revision: correction_revision
            })
        );
        drop(connection);

        let newer_replacement = transcript_line(
            "fixture-staged-newer",
            now() - Duration::minutes(20),
            "claude-sonnet-4-20250514",
            usage(20, 0, 0, 0),
        )
        .replacen(
            "\"timestamp\"",
            "\"supersedes\":[\"frame-fixture-staged-new\"],\"timestamp\"",
            1,
        );
        write_transcript(&transcript, &[old, replacement, newer_replacement]);
        let newer_correction = index_local_usage_at(
            &fixture.database(),
            &config,
            &fixture.probe(),
            now() + Duration::minutes(5),
        )
        .expect("a late-scanned correction must replace the source revision");
        let newer_revision = stored_daily_revision(&fixture.database(), now().date());
        assert_eq!(
            newer_correction.daily_usage[&now().date()].observed_tokens,
            20
        );
        assert!(newer_revision > increased_revision);
        assert_eq!(
            newer_correction.correction,
            Some(ProviderCorrection::ParserCorrection {
                source_revision: newer_revision
            })
        );
    }

    #[test]
    fn edge_only_invalid_replacement_is_retained_without_lowering_an_unavailable_scan() {
        const PRIVATE_INVALID_USAGE: &str = "PRIVATE-INVALID-USAGE";

        let fixture = FixtureRoot::new();
        let config = fixture.config();
        let transcript = config.join("projects/project-a/session.jsonl");
        let old = transcript_line(
            "fixture-edge-only-old",
            now() - Duration::minutes(10),
            "claude-sonnet-4-5-20250929",
            usage(100, 0, 0, 0),
        );
        write_transcript(&transcript, std::slice::from_ref(&old));
        index_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
            .expect("the original frame must index");
        let first_revision = stored_daily_revision(&fixture.database(), now().date());

        let invalid_replacement = transcript_line(
            "fixture-edge-only-new",
            now() - Duration::minutes(5),
            "claude-sonnet-4-5-20250929",
            usage(40, 0, 0, 0),
        )
        .replacen(
            "\"timestamp\"",
            "\"supersedes\":[\"frame-fixture-edge-only-old\"],\"timestamp\"",
            1,
        )
        .replacen(
            "\"input_tokens\":40",
            &format!("\"input_tokens\":\"{PRIVATE_INVALID_USAGE}\""),
            1,
        );
        let equal_unpriced_usage = transcript_line(
            "fixture-edge-only-unpriced",
            now() - Duration::minutes(4),
            "claude-unknown-model",
            usage(100, 0, 0, 0),
        );
        write_transcript(
            &transcript,
            &[old, invalid_replacement, equal_unpriced_usage],
        );
        let unavailable = index_local_usage_at(
            &fixture.database(),
            &config,
            &fixture.probe(),
            now() + Duration::minutes(1),
        )
        .expect("trusted outer metadata must retain the previous lower bound");

        assert_eq!(unavailable.scan_status, UsageScanStatus::Unavailable);
        assert_eq!(unavailable.daily_usage[&now().date()].observed_tokens, 100);
        assert_eq!(
            unavailable.daily_usage[&now().date()].coverage,
            UsageCoverage::Partial
        );
        assert_eq!(unavailable.daily_cost[&now().date()].priced_tokens, 100);
        assert!(stored_daily_revision(&fixture.database(), now().date()) > first_revision);
        assert_eq!(stored_message_count(&fixture.database()), 2);
        assert_eq!(stored_frame_count(&fixture.database()), 3);
        assert_eq!(stored_supersede_edge_count(&fixture.database()), 1);
        assert_sqlite_artifacts_exclude(&fixture.database(), PRIVATE_INVALID_USAGE);
    }

    #[test]
    fn completed_parser_correction_can_lower_but_an_incomplete_scan_cannot() {
        let fixture = FixtureRoot::new();
        let config = fixture.config();
        write_transcript(
            &config.join("projects/project-a/session.jsonl"),
            &[transcript_line(
                "fixture-parser-correction",
                now() - Duration::minutes(5),
                "claude-sonnet-4-20250514",
                usage(40, 0, 0, 0),
            )],
        );
        index_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
            .expect("the current parser record must index");

        let connection = Connection::open(fixture.database()).unwrap();
        connection
            .execute(
                "UPDATE claude_usage_daily SET observed_tokens = 100 WHERE day = ?1",
                [now().date().to_string()],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE claude_usage_index_meta SET value = ?2 WHERE key = ?1",
                params![
                    USAGE_AGGREGATE_PARSER_VERSION_KEY,
                    (TRANSCRIPT_PARSER_VERSION - 1).to_string()
                ],
            )
            .unwrap();
        let cutoff = now().date() - Duration::days(TOKEN_HISTORY_RETENTION_DAYS - 1);

        refresh_daily_aggregates(&connection, cutoff, now().date(), false).unwrap();
        let incomplete = connection
            .query_row(
                "SELECT observed_tokens, coverage, revision, correction_provenance,
                        correction_source_revision
                 FROM claude_usage_daily
                 WHERE day = ?1",
                [now().date().to_string()],
                |row| {
                    Ok((
                        row.get::<_, u64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, u64>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<u64>>(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(incomplete.0, 100);
        assert_eq!(incomplete.1, "partial");
        assert_eq!(incomplete.3, None);
        assert_eq!(incomplete.4, None);

        refresh_daily_aggregates(&connection, cutoff, now().date(), true).unwrap();
        let completed = connection
            .query_row(
                "SELECT observed_tokens, coverage, revision, correction_provenance,
                        correction_source_revision
                 FROM claude_usage_daily
                 WHERE day = ?1",
                [now().date().to_string()],
                |row| {
                    Ok((
                        row.get::<_, u64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, u64>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<u64>>(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(completed.0, 40);
        assert_eq!(completed.1, "complete");
        assert!(completed.2 > incomplete.2);
        assert_eq!(completed.3.as_deref(), Some(PARSER_CORRECTION_PROVENANCE));
        assert_eq!(completed.4, Some(completed.2));
        assert_eq!(
            stored_usage_aggregate_parser_version(&connection).unwrap(),
            Some(TRANSCRIPT_PARSER_VERSION)
        );

        let observation = read_indexed_usage(
            &connection,
            cutoff,
            now().date(),
            UsageScanStatus::Complete,
            true,
            true,
            false,
        )
        .unwrap();
        assert_eq!(
            observation.correction,
            Some(ProviderCorrection::ParserCorrection {
                source_revision: completed.2
            })
        );

        refresh_daily_aggregates(&connection, cutoff, now().date(), true).unwrap();
        let unchanged = connection
            .query_row(
                "SELECT revision, correction_provenance, correction_source_revision
                 FROM claude_usage_daily
                 WHERE day = ?1",
                [now().date().to_string()],
                |row| {
                    Ok((
                        row.get::<_, u64>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<u64>>(2)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(unchanged.0, completed.2);
        assert_eq!(unchanged.1.as_deref(), Some(PARSER_CORRECTION_PROVENANCE));
        assert_eq!(unchanged.2, Some(completed.2));

        refresh_daily_aggregates(&connection, cutoff, now().date(), false).unwrap();
        let later_revision = connection
            .query_row(
                "SELECT revision, correction_provenance, correction_source_revision
                 FROM claude_usage_daily
                 WHERE day = ?1",
                [now().date().to_string()],
                |row| {
                    Ok((
                        row.get::<_, u64>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<u64>>(2)?,
                    ))
                },
            )
            .unwrap();
        assert!(later_revision.0 > unchanged.0);
        assert_eq!(
            later_revision.1.as_deref(),
            Some(PARSER_CORRECTION_PROVENANCE)
        );
        assert_eq!(later_revision.2, Some(completed.2));
    }

    #[test]
    fn index_excludes_all_superseded_frames_before_nested_message_deduplication() {
        let fixture = FixtureRoot::new();
        let config = fixture.config();
        let shared_smaller = transcript_line(
            "fixture-shared-message",
            now() - Duration::minutes(12),
            "claude-sonnet-4-20250514",
            usage(100, 0, 0, 0),
        )
        .replacen(
            "\"uuid\":\"frame-fixture-shared-message\"",
            "\"uuid\":\"frame-copy-a\"",
            1,
        );
        let shared_larger = transcript_line(
            "fixture-shared-message",
            now() - Duration::minutes(11),
            "claude-sonnet-4-20250514",
            usage(200, 0, 0, 0),
        )
        .replacen(
            "\"uuid\":\"frame-fixture-shared-message\"",
            "\"uuid\":\"frame-copy-b\"",
            1,
        );
        let other = transcript_line(
            "fixture-other-message",
            now() - Duration::minutes(10),
            "claude-sonnet-4-20250514",
            usage(300, 0, 0, 0),
        );
        let replacement = transcript_line(
            "fixture-replacement-message",
            now() - Duration::minutes(5),
            "claude-sonnet-4-20250514",
            usage(40, 0, 0, 0),
        )
        .replacen(
            "\"timestamp\"",
            "\"supersedes\":[\"frame-copy-b\",\"frame-fixture-other-message\"],\"timestamp\"",
            1,
        );
        write_transcript(
            &config.join("projects/project-a/session.jsonl"),
            &[shared_smaller, shared_larger, other, replacement],
        );

        let local = index_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
            .expect("all supersede edges must apply before provider-message deduplication");

        assert_eq!(local.daily_usage[&now().date()].observed_tokens, 140);
        assert_eq!(
            local.daily_usage[&now().date()].coverage,
            UsageCoverage::Complete
        );
        assert_eq!(stored_supersede_edge_count(&fixture.database()), 2);
    }

    #[test]
    fn disappearing_transcripts_do_not_reduce_daily_usage_or_revision() {
        let fixture = FixtureRoot::new();
        let config = fixture.config();
        let transcript = config.join("projects/project-a/session.jsonl");
        write_transcript(
            &transcript,
            &[transcript_line(
                "fixture-message-a",
                now() - Duration::minutes(5),
                "claude-sonnet-4-20250514",
                usage(10, 20, 30, 40),
            )],
        );
        let first = index_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
            .expect("first scan must work");
        let first_revision = stored_daily_revision(&fixture.database(), now().date());
        assert_eq!(first.daily_usage[&now().date()].observed_tokens, 100);

        fs::remove_file(transcript).unwrap();
        let second = index_local_usage_at(
            &fixture.database(),
            &config,
            &fixture.probe(),
            now() + Duration::minutes(1),
        )
        .expect("missing files must preserve indexed evidence");

        assert_eq!(second.daily_usage[&now().date()].observed_tokens, 100);
        assert_eq!(
            second.daily_usage[&now().date()].coverage,
            UsageCoverage::Partial
        );
        assert_eq!(second.scan_status, UsageScanStatus::Unavailable);
        assert!(stored_daily_revision(&fixture.database(), now().date()) > first_revision);
    }

    #[test]
    fn same_inode_rewrites_restart_from_the_beginning() {
        let fixture = FixtureRoot::new();
        let config = fixture.config();
        let transcript = config.join("projects/project-a/session.jsonl");
        let first = transcript_line(
            "fixture-message-a",
            now() - Duration::minutes(5),
            "claude-sonnet-4-20250514",
            usage(100, 0, 0, 0),
        );
        let second = transcript_line(
            "fixture-message-b",
            now() - Duration::minutes(5),
            "claude-sonnet-4-20250514",
            usage(200, 0, 0, 0),
        );
        assert_eq!(first.len(), second.len());
        write_transcript(&transcript, &[first]);
        let initial = index_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
            .expect("first scan must work");
        assert_eq!(initial.daily_usage[&now().date()].observed_tokens, 100);

        std::thread::sleep(std::time::Duration::from_millis(2));
        write_transcript(&transcript, &[second]);
        let rescanned = index_local_usage_at(
            &fixture.database(),
            &config,
            &fixture.probe(),
            now() + Duration::minutes(1),
        )
        .expect("same-inode rewrites must rescan");

        assert_eq!(rescanned.daily_usage[&now().date()].observed_tokens, 300);
    }

    #[test]
    fn truncate_and_regrow_restarts_when_the_index_anchor_changes() {
        let fixture = FixtureRoot::new();
        let config = fixture.config();
        let transcript = config.join("projects/project-a/session.jsonl");
        write_transcript(
            &transcript,
            &[transcript_line(
                "fixture-message-a",
                now() - Duration::minutes(5),
                "claude-sonnet-4-5-20250929",
                usage(100, 0, 0, 0),
            )],
        );
        let initial = index_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
            .expect("first scan must work");
        assert_eq!(initial.daily_usage[&now().date()].observed_tokens, 100);

        std::thread::sleep(std::time::Duration::from_millis(2));
        write_transcript(
            &transcript,
            &[
                transcript_line(
                    "fixture-message-b",
                    now() - Duration::minutes(4),
                    "claude-sonnet-4-5-20250929",
                    usage(200, 0, 0, 0),
                ),
                transcript_line(
                    "fixture-message-c",
                    now() - Duration::minutes(3),
                    "claude-sonnet-4-5-20250929",
                    usage(300, 0, 0, 0),
                ),
            ],
        );
        let rescanned = index_local_usage_at(
            &fixture.database(),
            &config,
            &fixture.probe(),
            now() + Duration::minutes(1),
        )
        .expect("a changed append anchor must force a full rescan");

        assert_eq!(rescanned.daily_usage[&now().date()].observed_tokens, 600);
    }

    #[test]
    fn index_retains_sixty_days_and_reports_a_best_effort_partial_trend() {
        let fixture = FixtureRoot::new();
        let config = fixture.config();
        write_transcript(
            &config.join("projects/project-a/session.jsonl"),
            &[
                transcript_line(
                    "fixture-current",
                    now() - Duration::minutes(5),
                    "claude-sonnet-4-20250514",
                    usage(200, 0, 0, 0),
                ),
                transcript_line(
                    "fixture-previous",
                    now() - Duration::days(30),
                    "claude-sonnet-4-20250514",
                    usage(100, 0, 0, 0),
                ),
            ],
        );

        let local = index_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
            .expect("60-day token history must index");
        assert_eq!(
            local.daily_usage[&(now().date() - Duration::days(30))].observed_tokens,
            100
        );
        let periods = project_usage_periods(Some(&local), now());
        let UsageTotal::Current {
            observed_tokens,
            trend_percent,
            trend_previous_tokens,
            ..
        } = periods.thirty_days
        else {
            panic!("30-day usage must be available");
        };
        assert_eq!(observed_tokens, 200);
        assert_eq!(trend_previous_tokens, Some(100));
        assert_eq!(trend_percent, Some(100.0));
    }

    #[test]
    fn index_retains_checkpoints_for_the_sixty_day_trend_window() {
        let fixture = FixtureRoot::new();
        let config = fixture.config();
        let transcript = config.join("projects/project-a/session.jsonl");
        let timestamp = now() - Duration::days(40);
        write_transcript(
            &transcript,
            &[transcript_line(
                "fixture-previous-period",
                timestamp,
                "claude-sonnet-4-20250514",
                usage(100, 0, 0, 0),
            )],
        );
        set_modified_at(&transcript, timestamp);
        let first = index_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
            .expect("a previous-period transcript must index");
        assert_eq!(first.daily_usage[&timestamp.date()].observed_tokens, 100);

        let second = index_local_usage_with_budget(
            &fixture.database(),
            &config,
            &fixture.probe(),
            now() + Duration::minutes(1),
            ScanBudget {
                max_bytes: 0,
                max_file_bytes: MAX_TRANSCRIPT_FILE_SCAN_BYTES,
                max_millis: MAX_TRANSCRIPT_SCAN_MILLIS,
            },
        )
        .expect("an unchanged retained checkpoint must need no scan bytes");

        assert_eq!(second.scan_status, UsageScanStatus::Complete);
        assert_eq!(second.daily_usage[&timestamp.date()].observed_tokens, 100);
        assert_eq!(stored_file_count(&fixture.database()), 1);
    }

    #[test]
    fn new_previous_period_messages_extend_the_retained_deduplicated_union() {
        let fixture = FixtureRoot::new();
        let config = fixture.config();
        let day = now().date() - Duration::days(40);
        let timestamp = now() - Duration::days(40);
        let first_transcript = config.join("projects/project-a/session.jsonl");
        write_transcript(
            &first_transcript,
            &[transcript_line(
                "fixture-retained-message",
                timestamp,
                "claude-sonnet-4-5-20250929",
                usage(100, 0, 0, 0),
            )],
        );
        set_modified_at(&first_transcript, timestamp);
        let first = index_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
            .expect("the first previous-period message must index");
        assert_eq!(first.daily_usage[&day].observed_tokens, 100);

        let retained = index_local_usage_at(
            &fixture.database(),
            &config,
            &fixture.probe(),
            now() + Duration::minutes(1),
        )
        .expect("a normal refresh must retain previous-period evidence");
        assert_eq!(retained.daily_usage[&day].observed_tokens, 100);

        let second_transcript = config.join("projects/project-b/session.jsonl");
        let retained_copy = transcript_line(
            "fixture-retained-message",
            timestamp,
            "claude-sonnet-4-5-20250929",
            usage(100, 0, 0, 0),
        )
        .replacen(
            "\"uuid\":\"frame-fixture-retained-message\"",
            "\"uuid\":\"frame-retained-copy\"",
            1,
        );
        write_transcript(
            &second_transcript,
            &[
                retained_copy,
                transcript_line(
                    "fixture-new-message",
                    timestamp + Duration::minutes(1),
                    "claude-sonnet-4-5-20250929",
                    usage(50, 0, 0, 0),
                ),
            ],
        );
        set_modified_at(&second_transcript, now() + Duration::minutes(2));
        let extended = index_local_usage_at(
            &fixture.database(),
            &config,
            &fixture.probe(),
            now() + Duration::minutes(2),
        )
        .expect("new previous-period evidence must extend the retained union");

        assert_eq!(extended.daily_usage[&day].observed_tokens, 150);
        assert_eq!(extended.daily_usage[&day].coverage, UsageCoverage::Complete);
        assert!(!extended.daily_cost.contains_key(&day));
        assert_eq!(stored_message_count(&fixture.database()), 2);
        assert_eq!(stored_frame_count(&fixture.database()), 3);
        let connection = Connection::open(fixture.database()).unwrap();
        let private_cost = connection
            .query_row(
                "SELECT priced_tokens, cost_usd, pricing_basis, pricing_fingerprint
                 FROM claude_usage_daily WHERE day = ?1",
                [day.to_string()],
                |row| {
                    Ok((
                        row.get::<_, u64>(0)?,
                        row.get::<_, Option<f64>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(private_cost, (0, None, None, None));
        let retained_private_details = connection
            .query_row(
                "SELECT COUNT(*) FROM claude_usage_messages
                 WHERE day = ?1 AND (
                   model != '' OR input_tokens != 0 OR cache_creation_input_tokens != 0
                   OR cache_read_input_tokens != 0 OR output_tokens != 0
                   OR cache_creation_5m_input_tokens IS NOT NULL
                   OR cache_creation_1h_input_tokens IS NOT NULL
                   OR service_tier IS NOT NULL OR inference_geo IS NOT NULL
                   OR speed IS NOT NULL OR web_search_requests IS NOT NULL
                   OR web_fetch_requests IS NOT NULL OR code_execution_requests IS NOT NULL
                   OR has_unknown_paid_server_tool != 0
                 )",
                [day.to_string()],
                |row| row.get::<_, u64>(0),
            )
            .unwrap();
        assert_eq!(retained_private_details, 0);
    }

    #[test]
    fn index_skips_files_older_than_the_sixty_day_history_window() {
        let fixture = FixtureRoot::new();
        let config = fixture.config();
        let transcript = config.join("projects/project-a/session.jsonl");
        let timestamp = now() - Duration::days(61);
        write_transcript(
            &transcript,
            &[transcript_line(
                "fixture-expired",
                timestamp,
                "claude-sonnet-4-20250514",
                usage(100, 0, 0, 0),
            )],
        );
        set_modified_at(&transcript, timestamp);

        let local = index_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
            .expect("expired transcripts must not make the scan fail");

        assert_eq!(local.scan_status, UsageScanStatus::Complete);
        assert!(local.daily_usage.is_empty());
        assert_eq!(stored_file_count(&fixture.database()), 0);
    }

    #[test]
    fn index_prices_null_server_tool_usage_but_not_incomplete_counters() {
        let fixture = FixtureRoot::new();
        let config = fixture.config();
        let no_tool = transcript_line(
            "fixture-no-server-tool",
            now() - Duration::minutes(5),
            "claude-sonnet-4-5-20250929",
            usage(1_000_000, 0, 0, 1_000_000),
        )
        .replacen(
            r#""server_tool_use":{"web_search_requests":0,"web_fetch_requests":0}"#,
            r#""server_tool_use":null"#,
            1,
        );
        let incomplete_counters = transcript_line(
            "fixture-incomplete-server-tool",
            now() - Duration::minutes(4),
            "claude-sonnet-4-5-20250929",
            usage(1_000_000, 0, 0, 1_000_000),
        )
        .replacen(
            r#""server_tool_use":{"web_search_requests":0,"web_fetch_requests":0}"#,
            r#""server_tool_use":{"web_search_requests":0}"#,
            1,
        );
        write_transcript(
            &config.join("projects/project-a/session.jsonl"),
            &[no_tool, incomplete_counters],
        );

        let local = index_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
            .expect("nullable no-tool usage and incomplete counters must retain token evidence");
        let detail = &local.daily_cost[&now().date()];

        assert_eq!(detail.observed_tokens, 4_000_000);
        assert_eq!(detail.priced_tokens, 2_000_000);
        assert_eq!(detail.api_equivalent_cost_usd, Some(18.0));
        assert!(!detail.complete);
    }

    #[test]
    fn index_omits_code_execution_cost_without_persisting_or_reporting_content() {
        const PRIVATE_CODE: &str = "PRIVATE-SERVER-CODE-CONTENT";

        let fixture = FixtureRoot::new();
        let config = fixture.config();
        let code_execution = transcript_line(
            "fixture-code-execution",
            now() - Duration::minutes(5),
            "claude-sonnet-4-5-20250929",
            usage(1_000_000, 0, 0, 1_000_000),
        )
        .replacen(
            r#"{"type":"text","text":"PRIVATE-CONTENT"}"#,
            &format!(
                r#"{{"type":"server_tool_use","name":"code_execution","input":{{"code":"{PRIVATE_CODE}"}}}}"#
            ),
            1,
        );
        write_transcript(
            &config.join("projects/project-a/session.jsonl"),
            &[code_execution],
        );

        let local = index_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
            .expect("code-execution metadata must retain token evidence");
        assert_eq!(local.daily_usage[&now().date()].observed_tokens, 2_000_000);
        assert!(!local.daily_cost.contains_key(&now().date()));
        let connection = Connection::open(fixture.database()).unwrap();
        let pricing_signal = connection
            .query_row(
                "SELECT code_execution_requests, has_unknown_paid_server_tool
                 FROM claude_usage_messages",
                [],
                |row| Ok((row.get::<_, Option<u64>>(0)?, row.get::<_, bool>(1)?)),
            )
            .unwrap();
        assert_eq!(pricing_signal, (Some(1), false));
        drop(connection);

        let report = debug_usage_report(&fixture.database(), &config, &fixture.probe(), now())
            .expect("code-execution usage must produce a sanitized report");
        assert!(report.contains("pricing_status=unavailable"));
        assert!(!report.contains(PRIVATE_CODE));
        assert_sqlite_artifacts_exclude(&fixture.database(), PRIVATE_CODE);
    }

    #[test]
    fn index_prices_known_messages_and_keeps_unknown_models_unpriced() {
        let fixture = FixtureRoot::new();
        let config = fixture.config();
        write_transcript(
            &config.join("projects/project-a/session.jsonl"),
            &[
                transcript_line(
                    "fixture-priced",
                    now() - Duration::minutes(5),
                    "claude-sonnet-4-5-20250929",
                    usage(1_000_000, 0, 0, 1_000_000),
                ),
                transcript_line(
                    "fixture-unpriced",
                    now() - Duration::minutes(4),
                    "claude-unknown-model",
                    usage(500_000, 0, 0, 500_000),
                ),
            ],
        );

        let local = index_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
            .expect("known and unknown synthetic messages must index");
        let detail = &local.daily_cost[&now().date()];
        assert_eq!(detail.observed_tokens, 3_000_000);
        assert_eq!(detail.priced_tokens, 2_000_000);
        assert!(!detail.complete);
        assert_eq!(detail.api_equivalent_cost_usd, Some(18.0));
        assert_eq!(
            local.pricing_basis.as_deref(),
            Some("anthropic-standard-2026-08-26-v1")
        );

        let UsageTotal::Current {
            observed_tokens,
            api_equivalent_cost_usd,
            api_equivalent_cost_quality,
            api_equivalent_cost_coverage_percent,
            ..
        } = project_usage_periods(Some(&local), now()).today
        else {
            panic!("today must be available");
        };
        assert_eq!(observed_tokens, 3_000_000);
        assert_eq!(api_equivalent_cost_usd, Some(27.0));
        assert_eq!(
            api_equivalent_cost_quality,
            Some(ApiEquivalentCostQuality::Modeled)
        );
        let cost_coverage = api_equivalent_cost_coverage_percent.expect("partial price coverage");
        assert!((cost_coverage - (200.0 / 3.0)).abs() < 0.001);
    }

    #[test]
    fn unavailable_geo_uses_persisted_modeled_global_cost() {
        let fixture = FixtureRoot::new();
        let config = fixture.config();
        let transcript = transcript_line(
            "fixture-unavailable-geo",
            now() - Duration::minutes(5),
            "claude-opus-4-8",
            usage(1_000_000, 0, 0, 1_000_000),
        )
        .replacen(
            r#""server_tool_use""#,
            r#""service_tier":"standard","inference_geo":"not_available","speed":"standard","server_tool_use""#,
            1,
        );
        write_transcript(
            &config.join("projects/project-a/session.jsonl"),
            &[transcript],
        );

        let indexed = index_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
            .expect("unavailable geo must retain a modeled standard cost");
        let detail = &indexed.daily_cost[&now().date()];
        assert_eq!(detail.observed_tokens, 2_000_000);
        assert_eq!(detail.priced_tokens, 2_000_000);
        assert_eq!(detail.api_equivalent_cost_usd, Some(30.0));
        assert!(detail.modeled);

        let restored = index_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
            .expect("the modeled marker must survive a cached scan");
        assert!(restored.daily_cost[&now().date()].modeled);
        let UsageTotal::Current {
            api_equivalent_cost_usd,
            api_equivalent_cost_quality,
            api_equivalent_cost_coverage_percent,
            ..
        } = project_usage_periods(Some(&restored), now()).today
        else {
            panic!("today must be available");
        };
        assert_eq!(api_equivalent_cost_usd, Some(30.0));
        assert_eq!(
            api_equivalent_cost_quality,
            Some(ApiEquivalentCostQuality::Modeled)
        );
        assert_eq!(api_equivalent_cost_coverage_percent, Some(100.0));
        assert!(
            Connection::open(fixture.database())
                .unwrap()
                .query_row(
                    "SELECT cost_modeled FROM claude_usage_daily WHERE day = ?1",
                    [now().date().to_string()],
                    |row| row.get::<_, bool>(0),
                )
                .unwrap()
        );
    }

    #[test]
    fn malformed_present_pricing_modifier_omits_cost_without_persisting_value() {
        const PRIVATE_MODIFIER: &str = "PRIVATE-MODIFIER";

        let fixture = FixtureRoot::new();
        let config = fixture.config();
        let malformed = transcript_line(
            "fixture-malformed-modifier",
            now() - Duration::minutes(5),
            "claude-sonnet-4-5-20250929",
            usage(1_000_000, 0, 0, 1_000_000),
        )
        .replacen(
            r#""server_tool_use""#,
            &format!(r#""service_tier":"{PRIVATE_MODIFIER}","server_tool_use""#),
            1,
        );
        write_transcript(
            &config.join("projects/project-a/session.jsonl"),
            &[malformed],
        );

        let local = index_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
            .expect("malformed pricing metadata must retain token evidence");

        assert_eq!(local.daily_usage[&now().date()].observed_tokens, 2_000_000);
        assert!(!local.daily_cost.contains_key(&now().date()));
        assert_sqlite_artifacts_exclude(&fixture.database(), PRIVATE_MODIFIER);
    }

    #[test]
    fn catalog_refresh_revises_only_days_with_changed_applicable_rules() {
        let fixture = FixtureRoot::new();
        let config = fixture.config();
        write_transcript(
            &config.join("projects/project-a/session.jsonl"),
            &[transcript_line(
                "fixture-priced",
                now() - Duration::minutes(5),
                "claude-sonnet-4-5-20250929",
                usage(1_000_000, 0, 0, 1_000_000),
            )],
        );
        index_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
            .expect("the priced message must index");

        let connection = Connection::open(fixture.database()).unwrap();
        let read_daily = || {
            connection
                .query_row(
                    "SELECT observed_tokens, priced_tokens, cost_usd, revision,
                            pricing_fingerprint
                     FROM claude_usage_daily WHERE day = ?1",
                    [now().date().to_string()],
                    |row| {
                        Ok((
                            row.get::<_, u64>(0)?,
                            row.get::<_, u64>(1)?,
                            row.get::<_, Option<f64>>(2)?,
                            row.get::<_, u64>(3)?,
                            row.get::<_, Option<String>>(4)?,
                        ))
                    },
                )
                .unwrap()
        };
        let read_catalog_fingerprint = || {
            connection
                .query_row(
                    "SELECT value FROM claude_usage_index_meta
                     WHERE key = 'pricing_manifest_fingerprint'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap()
        };
        let cutoff = now().date() - Duration::days(TOKEN_HISTORY_RETENTION_DAYS - 1);
        let initial = read_daily();
        assert_eq!(initial.0, 2_000_000);
        assert_eq!(initial.1, 2_000_000);
        assert_eq!(initial.2, Some(18.0));

        let base_catalog = super::super::pricing::catalog().unwrap();
        let unrelated_catalog =
            pricing_catalog_with_standard_output_rate("claude-haiku-4-5-20251001", 6.0);
        assert_eq!(unrelated_catalog.basis(), base_catalog.basis());
        assert_ne!(
            unrelated_catalog.semantic_fingerprint(),
            base_catalog.semantic_fingerprint()
        );
        refresh_daily_aggregates_with_catalog(
            &connection,
            cutoff,
            now().date(),
            true,
            Some(&unrelated_catalog),
        )
        .unwrap();
        assert_eq!(read_daily(), initial);
        assert_eq!(
            read_catalog_fingerprint(),
            unrelated_catalog.semantic_fingerprint()
        );

        let applicable_catalog =
            pricing_catalog_with_standard_output_rate("claude-sonnet-4-5-20250929", 16.0);
        refresh_daily_aggregates_with_catalog(
            &connection,
            cutoff,
            now().date(),
            true,
            Some(&applicable_catalog),
        )
        .unwrap();
        let repriced = read_daily();
        assert_eq!(repriced.0, initial.0);
        assert_eq!(repriced.1, initial.1);
        assert_eq!(repriced.2, Some(19.0));
        assert_eq!(repriced.3, initial.3 + 1);
        assert_ne!(repriced.4, initial.4);
        assert_eq!(
            read_catalog_fingerprint(),
            applicable_catalog.semantic_fingerprint()
        );

        refresh_daily_aggregates_with_catalog(
            &connection,
            cutoff,
            now().date(),
            true,
            Some(&applicable_catalog),
        )
        .unwrap();
        assert_eq!(read_daily(), repriced);
    }

    #[test]
    fn catalog_basis_only_change_keeps_unchanged_daily_basis_and_revision() {
        let fixture = FixtureRoot::new();
        let config = fixture.config();
        write_transcript(
            &config.join("projects/project-a/session.jsonl"),
            &[transcript_line(
                "fixture-basis-only",
                now() - Duration::minutes(5),
                "claude-sonnet-4-5-20250929",
                usage(1_000_000, 0, 0, 1_000_000),
            )],
        );
        index_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
            .expect("the priced message must index");

        let connection = Connection::open(fixture.database()).unwrap();
        let read_daily = || {
            connection
                .query_row(
                    "SELECT observed_tokens, priced_tokens, cost_usd, revision,
                            pricing_basis, pricing_fingerprint
                     FROM claude_usage_daily WHERE day = ?1",
                    [now().date().to_string()],
                    |row| {
                        Ok((
                            row.get::<_, u64>(0)?,
                            row.get::<_, u64>(1)?,
                            row.get::<_, Option<f64>>(2)?,
                            row.get::<_, u64>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, Option<String>>(5)?,
                        ))
                    },
                )
                .unwrap()
        };
        let initial = read_daily();
        let changed_basis = pricing_catalog_with_basis("anthropic-standard-2026-08-26-v2");
        let cutoff = now().date() - Duration::days(TOKEN_HISTORY_RETENTION_DAYS - 1);

        refresh_daily_aggregates_with_catalog(
            &connection,
            cutoff,
            now().date(),
            true,
            Some(&changed_basis),
        )
        .unwrap();

        assert_eq!(read_daily(), initial);
    }

    #[test]
    fn mixed_retained_price_books_keep_bounded_provenance_during_incomplete_scan() {
        const NEW_BASIS: &str = "anthropic-standard-2026-08-26-v2";

        let fixture = FixtureRoot::new();
        let config = fixture.config();
        let old_day = now().date() - Duration::days(1);
        write_transcript(
            &config.join("projects/project-a/session.jsonl"),
            &[transcript_line(
                "fixture-old-basis",
                now() - Duration::days(1),
                "claude-sonnet-4-5-20250929",
                usage(1_000_000, 0, 0, 1_000_000),
            )],
        );
        let indexed = index_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
            .expect("the old price-book day must index");
        let old_basis = indexed.pricing_basis.expect("old price-book basis");

        let connection = Connection::open(fixture.database()).unwrap();
        let salt = load_or_create_dedupe_salt(&connection).unwrap();
        let new_line = transcript_line(
            "fixture-new-basis",
            now() - Duration::minutes(5),
            "claude-sonnet-4-5-20250929",
            usage(1_000_000, 0, 0, 1_000_000),
        );
        let TranscriptLineOutcome::Usage(message) =
            parse_transcript_line(new_line.as_bytes(), &salt)
        else {
            panic!("the new price-book message must parse");
        };
        store_message(&connection, *message).unwrap();
        let new_catalog = pricing_catalog_with_basis(NEW_BASIS);
        let cutoff = now().date() - Duration::days(TOKEN_HISTORY_RETENTION_DAYS - 1);
        refresh_daily_aggregates_with_catalog(
            &connection,
            cutoff,
            now().date(),
            true,
            Some(&new_catalog),
        )
        .unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT pricing_basis FROM claude_usage_daily WHERE day = ?1",
                    [old_day.to_string()],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            old_basis
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT pricing_basis FROM claude_usage_daily WHERE day = ?1",
                    [now().date().to_string()],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            NEW_BASIS
        );

        let aggregate_changed = refresh_daily_aggregates_with_catalog(
            &connection,
            cutoff,
            now().date(),
            false,
            Some(&new_catalog),
        )
        .unwrap();
        let local = read_indexed_usage(
            &connection,
            cutoff,
            now().date(),
            UsageScanStatus::Unavailable,
            false,
            true,
            aggregate_changed,
        )
        .unwrap();
        let expected_basis = format!("{old_basis} + {NEW_BASIS}");
        assert_eq!(
            local.pricing_basis.as_deref(),
            Some(expected_basis.as_str())
        );
        assert!(expected_basis.len() <= 256);
        let UsageTotal::Current {
            api_equivalent_cost_usd,
            api_equivalent_cost_basis,
            ..
        } = project_usage_periods(Some(&local), now()).seven_days
        else {
            panic!("mixed retained price books must keep seven-day cost");
        };
        assert!(api_equivalent_cost_usd.is_some());
        assert_eq!(
            api_equivalent_cost_basis.as_deref(),
            Some(expected_basis.as_str())
        );
    }

    #[test]
    fn quota_probe_transcript_is_excluded_even_when_cleanup_is_pending() {
        const PROBE_SESSION: &str = "11111111-1111-4111-8111-111111111111";

        let fixture = FixtureRoot::new();
        let config = fixture.config();
        let probe = fixture.probe();
        write_transcript(
            &config.join("projects/project-a/session.jsonl"),
            &[transcript_line(
                "fixture-user-message",
                now() - Duration::minutes(5),
                "claude-sonnet-4-20250514",
                usage(10, 20, 30, 40),
            )],
        );
        let first = index_local_usage_at(&fixture.database(), &config, &probe, now())
            .expect("the user transcript must index before the quota probe");
        assert_eq!(first.daily_usage[&now().date()].observed_tokens, 100);
        let first_revision = stored_daily_revision(&fixture.database(), now().date());

        fs::create_dir_all(&probe).unwrap();
        fs::write(
            probe.join(super::super::cli_probe::PROBE_SESSION_MARKER),
            format!("{PROBE_SESSION}\n"),
        )
        .unwrap();
        let transcript = super::super::cli_probe::pending_probe_transcript(&probe, &config)
            .expect("the marker must resolve the exact probe transcript");
        write_transcript(
            &transcript,
            &[transcript_line(
                "fixture-probe-message",
                now() - Duration::minutes(1),
                "claude-sonnet-4-20250514",
                usage(500, 0, 0, 500),
            )],
        );

        let local = index_local_usage_at(
            &fixture.database(),
            &config,
            &probe,
            now() + Duration::minutes(1),
        )
        .expect("an existing transcript root must return an observation");
        assert_eq!(local.daily_usage[&now().date()].observed_tokens, 100);
        assert_eq!(
            stored_daily_revision(&fixture.database(), now().date()),
            first_revision
        );
        assert_eq!(stored_message_count(&fixture.database()), 1);
    }

    #[test]
    fn invalid_probe_marker_excludes_its_project_and_marks_the_scan_unavailable() {
        let fixture = FixtureRoot::new();
        let config = fixture.config();
        let probe = fixture.probe();
        fs::create_dir_all(&probe).unwrap();
        fs::write(
            probe.join(super::super::cli_probe::PROBE_SESSION_MARKER),
            "invalid-private-marker\n",
        )
        .unwrap();
        let project = match super::super::cli_probe::probe_transcript_exclusion(&probe, &config) {
            super::super::cli_probe::ProbeTranscriptExclusion::UnsafeProject(project) => project,
            _ => panic!("an invalid marker must fail closed for its project"),
        };
        write_transcript(
            &project.join("session.jsonl"),
            &[transcript_line(
                "fixture-probe-message",
                now() - Duration::minutes(1),
                "claude-sonnet-4-20250514",
                usage(500, 0, 0, 500),
            )],
        );

        let local = index_local_usage_at(&fixture.database(), &config, &probe, now())
            .expect("an unsafe marker must return an unavailable observation");

        assert_eq!(local.scan_status, UsageScanStatus::Unavailable);
        assert!(local.daily_usage.is_empty());
        assert_eq!(stored_message_count(&fixture.database()), 0);
    }

    #[test]
    fn debug_report_shows_sanitized_period_and_model_day_totals() {
        let fixture = FixtureRoot::new();
        let config = fixture.config();
        write_transcript(
            &config.join("projects/project-a/PRIVATE-SESSION.jsonl"),
            &[transcript_line(
                "PRIVATE-MESSAGE-ID",
                now() - Duration::minutes(1),
                "claude-sonnet-4-5-20250929",
                usage(10, 20, 30, 40),
            )],
        );

        let local = index_local_usage_at(&fixture.database(), &config, &fixture.probe(), now())
            .expect("the synthetic usage index must load");
        assert_eq!(
            local
                .top_model_usage
                .as_ref()
                .and_then(|top| top.model.as_deref()),
            Some("Claude Sonnet 4.5")
        );
        assert_eq!(
            local
                .top_model_usage
                .as_ref()
                .map(|top| top.observed_tokens),
            Some(100)
        );

        let report = debug_usage_report(&fixture.database(), &config, &fixture.probe(), now())
            .expect("the synthetic usage report must render");

        assert!(report.contains("[TouchGrassBar][claude-usage-report]"));
        assert!(report.contains("period=today"));
        assert!(report.contains("authoritative_tokens=100"));
        assert!(report.contains("model=claude-sonnet-4-5-20250929"));
        assert!(report.contains("observed_tokens=100"));
        for forbidden in [
            "PRIVATE-MESSAGE-ID",
            "PRIVATE-SESSION",
            "PRIVATE-CONTENT",
            "/PRIVATE/PATH",
        ] {
            assert!(!report.contains(forbidden));
        }
    }
}
