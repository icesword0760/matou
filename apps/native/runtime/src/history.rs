use std::{
    borrow::Cow,
    collections::{BTreeMap, VecDeque},
    fs, io,
    path::{Component, Path, PathBuf},
    sync::LazyLock,
};

use anyhow::{Context, Result, ensure};
use encoding_rs::{CoderResult, Decoder, UTF_8};
use matou_native_core::journal::{Frame, Record, codec::read_segment};
use regress::{Flags, Regex};
use serde::{Deserialize, Serialize};

pub(crate) const MAX_LINES: usize = 1_000;
const DEFAULT_CONTEXT_LINES: usize = 250;
const MAX_CONTEXT_LINES: usize = 499;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryCursor {
    pub sequence: u64,
    pub line_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryLine {
    pub sequence: u64,
    pub cursor: HistoryCursor,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HistoryGapCode {
    CorruptSegment,
    MissingSegment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryGap {
    pub segment_index: u32,
    pub code: HistoryGapCode,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPage {
    pub lines: Vec<HistoryLine>,
    pub gaps: Vec<HistoryGap>,
    pub has_more: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_more_before: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_more_after: Option<bool>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPageInput {
    pub before: Option<HistoryCursor>,
    pub line_limit: Option<f64>,
    pub around: Option<HistoryCursor>,
    pub before_lines: Option<f64>,
    pub after_lines: Option<f64>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HistorySearchOptions {
    pub case_sensitive: bool,
    pub regex: bool,
    pub whole_word: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistorySearchInput {
    pub query: String,
    pub before: Option<HistoryCursor>,
    pub limit: Option<f64>,
    pub options: Option<HistorySearchOptions>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistorySearchResult {
    pub matches: Vec<HistoryLine>,
    pub gaps: Vec<HistoryGap>,
    pub has_more: bool,
}

/// Reads archived terminal output without modifying the journal or its indexes.
pub struct JournalHistoryReader {
    data_root: PathBuf,
    #[cfg(test)]
    after_initial_listing: Option<AfterInitialListing>,
}

#[cfg(test)]
type AfterInitialListing = Box<dyn Fn(&BTreeMap<u32, Vec<PathBuf>>) -> Result<()> + Send + Sync>;

impl JournalHistoryReader {
    pub fn new(data_root: impl Into<PathBuf>) -> Self {
        Self {
            data_root: data_root.into(),
            #[cfg(test)]
            after_initial_listing: None,
        }
    }

    pub fn page(&self, session_id: &str, input: HistoryPageInput) -> Result<HistoryPage> {
        if let Some(around) = input.around {
            return self.context_window(session_id, around, input);
        }

        let limit = bounded_limit(input.line_limit, 1, MAX_LINES, MAX_LINES);
        let mut page = HistoryPage::default();
        let mut lines = VecDeque::new();
        self.scan(session_id, &mut page.gaps, |line| {
            if input.before.is_none_or(|before| line.cursor < before) {
                page.has_more |= push_recent(&mut lines, line, limit);
            }
            true
        })?;
        page.lines = lines.into();
        Ok(page)
    }

    fn context_window(
        &self,
        session_id: &str,
        around: HistoryCursor,
        input: HistoryPageInput,
    ) -> Result<HistoryPage> {
        let before_limit = bounded_limit(
            input.before_lines,
            0,
            MAX_CONTEXT_LINES,
            DEFAULT_CONTEXT_LINES,
        );
        let after_limit = bounded_limit(
            input.after_lines,
            0,
            MAX_CONTEXT_LINES,
            DEFAULT_CONTEXT_LINES,
        );
        let mut page = HistoryPage::default();
        let mut before = VecDeque::new();
        let mut after = Vec::new();
        let mut anchor = None;
        let mut has_more_before = false;
        let mut has_more_after = false;
        self.scan(session_id, &mut page.gaps, |line| {
            match line.cursor.cmp(&around) {
                std::cmp::Ordering::Less => {
                    has_more_before |= push_recent(&mut before, line, before_limit);
                }
                std::cmp::Ordering::Equal => anchor = Some(line),
                std::cmp::Ordering::Greater if after.len() < after_limit => after.push(line),
                std::cmp::Ordering::Greater => {
                    has_more_after = true;
                    return false;
                }
            }
            true
        })?;
        if let Some(anchor) = anchor {
            page.anchor_index = Some(before.len());
            page.has_more_before = Some(has_more_before);
            page.has_more_after = Some(has_more_after);
            page.has_more = has_more_before || has_more_after;
            page.lines = before.into_iter().chain([anchor]).chain(after).collect();
        }
        Ok(page)
    }

    pub fn search(
        &self,
        session_id: &str,
        input: HistorySearchInput,
    ) -> Result<HistorySearchResult> {
        let mut result = HistorySearchResult::default();
        if input.query.is_empty() {
            return Ok(result);
        }

        let limit = bounded_limit(input.limit, 1, MAX_LINES, MAX_LINES);
        let options = input.options.unwrap_or_default();
        let pattern = SearchPattern::compile(&input.query, options)?;
        let mut matches = VecDeque::new();
        self.scan(session_id, &mut result.gaps, |line| {
            if input.before.is_none_or(|before| line.cursor < before)
                && pattern.matches(&line.text, options.case_sensitive)
            {
                result.has_more |= push_recent(&mut matches, line, limit);
            }
            true
        })?;
        result.matches = matches.into_iter().rev().collect();
        Ok(result)
    }

    fn scan(
        &self,
        session_id: &str,
        gaps: &mut Vec<HistoryGap>,
        mut on_line: impl FnMut(HistoryLine) -> bool,
    ) -> Result<()> {
        let mut components = Path::new(session_id).components();
        ensure!(
            matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none(),
            "invalid journal session ID"
        );
        let directory = self.data_root.join("journal").join(session_id);
        let groups = match segment_groups(&directory) {
            Ok(groups) => groups,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        #[cfg(test)]
        if let Some(after_initial_listing) = &self.after_initial_listing {
            after_initial_listing(&groups)?;
        }
        let mut previous = None;
        for &index in groups.keys() {
            if let Some(previous) = previous {
                gaps.extend((previous + 1..index).map(|segment_index| HistoryGap {
                    segment_index,
                    code: HistoryGapCode::MissingSegment,
                    message: "Journal segment is missing".into(),
                }));
            }
            previous = Some(index);
        }

        let mut decoder = UTF_8.new_decoder_with_bom_removal();
        let mut decoded = String::new();
        let mut pending = String::new();
        let mut cursor = HistoryCursor {
            sequence: 0,
            line_index: 0,
        };
        for (segment_index, paths) in groups {
            let mut records = read_first_valid(&paths);
            if records.is_err() {
                // Compression publishes gzip before removing raw. Refresh this
                // group once in case the initial listing observed only the raw file.
                if let Some(paths) = segment_groups(&directory)?.remove(&segment_index) {
                    records = read_first_valid(&paths);
                }
            }

            let records = match records {
                Ok(records) => records,
                Err(error) => {
                    let (code, message) = match error {
                        Some(error) => (HistoryGapCode::CorruptSegment, error.to_string()),
                        None => (
                            HistoryGapCode::MissingSegment,
                            "Journal segment is unavailable".into(),
                        ),
                    };
                    gaps.push(HistoryGap {
                        segment_index,
                        code,
                        message,
                    });
                    decoder = UTF_8.new_decoder_with_bom_removal();
                    pending.clear();
                    cursor = HistoryCursor {
                        sequence: 0,
                        line_index: 0,
                    };
                    continue;
                }
            };
            for record in records {
                let Frame::Output { sequence, data } = record.frame else {
                    continue;
                };
                cursor = HistoryCursor {
                    sequence,
                    line_index: 0,
                };
                decoded.clear();
                decode_output(&mut decoder, &data, &mut decoded, false);
                let mut parts = decoded.split('\n').peekable();
                while let Some(part) = parts.next() {
                    pending.push_str(part);
                    if parts.peek().is_some() {
                        if !on_line(history_line(cursor, std::mem::take(&mut pending))) {
                            return Ok(());
                        }
                        cursor.line_index += 1;
                    }
                }
            }
        }

        decode_output(&mut decoder, &[], &mut pending, true);
        if !pending.is_empty() {
            on_line(history_line(cursor, pending));
        }
        gaps.sort_by_key(|gap| gap.segment_index);
        Ok(())
    }
}

fn bounded_limit(value: Option<f64>, min: usize, max: usize, default: usize) -> usize {
    value
        .filter(|value| value.is_finite())
        .map_or(default, |value| {
            value.floor().clamp(min as f64, max as f64) as usize
        })
}

fn push_recent(lines: &mut VecDeque<HistoryLine>, line: HistoryLine, limit: usize) -> bool {
    lines.push_back(line);
    if lines.len() > limit {
        lines.pop_front();
        return true;
    }
    false
}

pub(crate) fn segment_groups(directory: &Path) -> io::Result<BTreeMap<u32, Vec<PathBuf>>> {
    let mut groups = BTreeMap::<u32, Vec<(u8, PathBuf)>>::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let filename = entry.file_name();
        let Some((index, suffix)) = filename
            .to_str()
            .and_then(|name| name.strip_prefix("segment-"))
            .and_then(|name| name.split_once('.'))
        else {
            continue;
        };
        if index.len() != 6 || !index.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let priority = match suffix {
            "mtj" => 0,
            "bin" => 1,
            "mtj.gz" => 2,
            "bin.gz" => 3,
            _ => continue,
        };
        let index = index.parse().expect("six ASCII digits");
        groups
            .entry(index)
            .or_default()
            .push((priority, entry.path()));
    }
    Ok(groups
        .into_iter()
        .map(|(index, mut paths)| {
            paths.sort_by_key(|(priority, _)| *priority);
            (index, paths.into_iter().map(|(_, path)| path).collect())
        })
        .collect())
}

// Validate the complete segment before emitting any lines so a corrupt segment
// cannot leak a valid-looking prefix into the result.
pub(crate) fn read_first_valid(
    paths: &[PathBuf],
) -> std::result::Result<Vec<Record>, Option<anyhow::Error>> {
    let mut last_error = None;
    for path in paths {
        match read_segment(path).and_then(|reader| reader.collect::<Result<Vec<_>>>()) {
            Ok(records) => return Ok(records),
            Err(error)
                if error
                    .downcast_ref::<io::Error>()
                    .is_some_and(|error| error.kind() == io::ErrorKind::NotFound) => {}
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error)
}

pub(crate) fn decode_output(decoder: &mut Decoder, data: &[u8], text: &mut String, last: bool) {
    let capacity = decoder
        .max_utf8_buffer_length(data.len())
        .expect("journal output frame is bounded");
    text.reserve(capacity);
    let (result, read, _) = decoder.decode_to_string(data, text, last);
    debug_assert_eq!(result, CoderResult::InputEmpty);
    debug_assert_eq!(read, data.len());
}

fn history_line(cursor: HistoryCursor, mut text: String) -> HistoryLine {
    static ANSI: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07]*(?:\x07|\x1b\\))")
            .expect("valid terminal cleanup expression")
    });
    if text.ends_with('\r') {
        text.pop();
    }
    if text.contains('\x1b') {
        if let Cow::Owned(cleaned) = ANSI.replace_all(&text, "") {
            text = cleaned;
        }
    }
    HistoryLine {
        sequence: cursor.sequence,
        cursor,
        text,
    }
}

pub(crate) enum SearchPattern {
    Regex(Regex),
    WholeWord(Regex),
    Literal(String),
}

impl SearchPattern {
    pub(crate) fn compile(query: &str, options: HistorySearchOptions) -> Result<Self> {
        if options.regex {
            let flags = Flags {
                icase: !options.case_sensitive,
                ..Flags::default()
            };
            let regex = Regex::from_unicode(query.encode_utf16().map(u32::from), flags)
                .context("invalid terminal history search pattern")?;
            return Ok(Self::Regex(regex));
        }

        let needle = if options.case_sensitive {
            query.to_owned()
        } else {
            query.to_lowercase()
        };
        if !options.whole_word {
            return Ok(Self::Literal(needle));
        }
        let mut literal = String::with_capacity(needle.len());
        for ch in needle.chars() {
            if r".*+?^${}()|[]\".contains(ch) {
                literal.push('\\');
            }
            literal.push(ch);
        }
        let expression = format!(r"(?:^|[^\p{{L}}\p{{N}}_]){literal}(?=$|[^\p{{L}}\p{{N}}_])");
        Ok(Self::WholeWord(Regex::with_flags(&expression, "u")?))
    }

    pub(crate) fn matches(&self, text: &str, case_sensitive: bool) -> bool {
        if let Self::Regex(regex) = self {
            let units: Vec<u16> = text.encode_utf16().collect();
            return regex.find_from_ucs2(&units, 0).next().is_some();
        }

        let text = if case_sensitive {
            Cow::Borrowed(text)
        } else {
            Cow::Owned(text.to_lowercase())
        };
        match self {
            Self::Literal(needle) => text.contains(needle),
            Self::WholeWord(regex) => regex.find(&text).is_some(),
            Self::Regex(_) => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use matou_native_core::journal::{
        JournalOptions, MAGIC, SegmentJournal, codec::encode_frame, wait_for_compression,
    };

    #[test]
    fn search_relists_segment_published_as_gzip_after_initial_listing() -> Result<()> {
        let root = tempfile::tempdir()?;
        let session = "publishing";
        let directory = root.path().join("journal").join(session);
        fs::create_dir_all(&directory)?;
        let raw = directory.join("segment-000001.mtj");
        let gzip = raw.with_extension("mtj.gz");
        let active = directory.join("segment-000002.mtj");
        let output = |sequence, text: &str| {
            encode_frame(&Record {
                timestamp: 1_000,
                frame: Frame::Output {
                    sequence,
                    data: text.as_bytes().to_vec(),
                },
            })
        };
        let mut bytes = MAGIC.to_vec();
        bytes.extend(output(1, "match first\nmatch second\n")?);
        bytes.extend(output(2, "match third\n")?);
        fs::write(&raw, &bytes)?;
        let mut bytes = MAGIC.to_vec();
        bytes.extend(output(3, "match next segment\n")?);
        fs::write(active, bytes)?;

        let mut reader = JournalHistoryReader::new(root.path());
        let data_root = root.path().to_owned();
        reader.after_initial_listing = Some(Box::new(move |groups| {
            assert_eq!(groups[&1], [raw.clone()]);
            assert!(raw.is_file() && !gzip.exists());
            // Start the real compressor only after the reader captured the raw
            // path. Wait for publication and unlink to finish before first open.
            let mut journal = SegmentJournal::open(
                &data_root,
                session,
                JournalOptions {
                    raw_hot_bytes: 0,
                    ..JournalOptions::default()
                },
            )?;
            wait_for_compression();
            assert!(gzip.is_file());
            assert_eq!(
                fs::metadata(&raw).unwrap_err().kind(),
                io::ErrorKind::NotFound
            );
            journal.close()
        }));

        let result = reader.search(
            session,
            HistorySearchInput {
                query: "match".into(),
                ..HistorySearchInput::default()
            },
        )?;
        let expected = [
            (3, 0, "match next segment"),
            (2, 0, "match third"),
            (1, 1, "match second"),
            (1, 0, "match first"),
        ]
        .map(|(sequence, line_index, text)| HistoryLine {
            sequence,
            cursor: HistoryCursor {
                sequence,
                line_index,
            },
            text: text.into(),
        });
        assert!(result.gaps.is_empty(), "unexpected gaps: {:?}", result.gaps);
        assert_eq!(result.matches, expected);
        assert!(!result.has_more);
        Ok(())
    }
}
