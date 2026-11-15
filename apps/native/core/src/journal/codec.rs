use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufReader, Read},
    path::Path,
};

use anyhow::{Result, ensure};
use flate2::read::MultiGzDecoder;
use serde_json::{Value, json};

use super::model::{
    Frame, JournalCorruptionError, MAGIC, MAX_FRAME_BYTES, MAX_SAFE_INTEGER, Record,
};

const FRAME_PREFIX_BYTES: usize = 8;
const HEADER_PREFIX_BYTES: usize = 4;

/// Encode one frame without the segment magic.
pub fn encode_frame(record: &Record) -> Result<Vec<u8>> {
    let sequence = record.frame.sequence();
    ensure!(
        (1..=MAX_SAFE_INTEGER).contains(&sequence),
        "invalid journal sequence"
    );
    let data = record.frame.output();
    let kind = match &record.frame {
        Frame::Output { .. } => "output",
        Frame::Resize { .. } => "resize",
        Frame::Reset { .. } => "reset",
        Frame::Encoding { .. } => "encoding",
        Frame::DomainCursor { .. } => "domain-cursor",
        Frame::Exit { .. } => "exit",
    };
    // preserve_order keeps this header byte-compatible with the original writer.
    let mut header = json!({
        "kind": kind,
        "sequence": sequence,
        "timestamp": record.timestamp,
        "dataLength": data.len(),
    });
    match &record.frame {
        Frame::Output { .. } => {}
        Frame::Resize { cols, rows, .. } => {
            header["cols"] = json!(cols);
            header["rows"] = json!(rows);
        }
        Frame::Reset { screen_epoch, .. } => header["screenEpoch"] = json!(screen_epoch),
        Frame::Encoding { encoding, .. } => header["encoding"] = json!(encoding),
        Frame::DomainCursor {
            domain_event_sequence,
            ..
        } => {
            header["domainEventSequence"] = json!(domain_event_sequence);
        }
        Frame::Exit {
            exit_code, signal, ..
        } => {
            header["exitCode"] = json!(exit_code);
            if let Some(signal) = signal {
                header["signal"] = json!(signal);
            }
        }
    }
    let header = serde_json::to_vec(&header)?;
    let body_len = HEADER_PREFIX_BYTES
        .checked_add(header.len())
        .and_then(|len| len.checked_add(data.len()))
        .filter(|&len| len <= MAX_FRAME_BYTES);
    let Some(body_len) = body_len else {
        anyhow::bail!("invalid journal frame length");
    };
    let mut output = Vec::with_capacity(FRAME_PREFIX_BYTES + body_len);
    output.extend_from_slice(&(body_len as u32).to_be_bytes());
    output.extend_from_slice(&[0; 4]);
    output.extend_from_slice(&(header.len() as u32).to_be_bytes());
    output.extend_from_slice(&header);
    output.extend_from_slice(data);
    let checksum = crc32fast::hash(&output[FRAME_PREFIX_BYTES..]);
    output[4..FRAME_PREFIX_BYTES].copy_from_slice(&checksum.to_be_bytes());
    Ok(output)
}

/// Stream one frame at a time. Offsets describe the uncompressed stream.
pub struct FrameReader<R> {
    reader: R,
    tolerate_torn_tail: bool,
    last_good_offset: u64,
    last_sequence: u64,
    finished: bool,
}

impl<R: Read> FrameReader<R> {
    pub fn new(mut reader: R, tolerate_torn_tail: bool) -> Result<Self> {
        let mut magic = [0; MAGIC.len()];
        if !read_exact_or_short(&mut reader, &mut magic)? || &magic != MAGIC {
            return Err(JournalCorruptionError::new("invalid Journal V2 magic", 0).into());
        }
        Ok(Self {
            reader,
            tolerate_torn_tail,
            last_good_offset: MAGIC.len() as u64,
            last_sequence: 0,
            finished: false,
        })
    }

    pub fn last_good_offset(&self) -> u64 {
        self.last_good_offset
    }

    pub fn last_sequence(&self) -> u64 {
        self.last_sequence
    }

    fn read_frame(&mut self) -> Result<Option<Record>> {
        let mut prefix = [0; FRAME_PREFIX_BYTES];
        if !read_first_byte(&mut self.reader, &mut prefix[..1])? {
            return Ok(None);
        }
        if !read_exact_or_short(&mut self.reader, &mut prefix[1..])? {
            return self.torn_tail("truncated journal frame prefix");
        }
        let body_length = read_u32(&prefix[..4]) as usize;
        let expected_crc = read_u32(&prefix[4..]);
        if !(HEADER_PREFIX_BYTES..=MAX_FRAME_BYTES).contains(&body_length) {
            return Err(self.corruption("invalid journal frame length"));
        }

        let mut body = vec![0; body_length];
        if !read_exact_or_short(&mut self.reader, &mut body)? {
            return self.torn_tail("truncated journal frame body");
        }
        if crc32fast::hash(&body) != expected_crc {
            if self.tolerate_torn_tail && !read_first_byte(&mut self.reader, &mut [0])? {
                return Ok(None);
            }
            return Err(self.corruption("journal frame checksum mismatch"));
        }
        let record = self.decode_record(body)?;
        self.last_sequence = record.frame.sequence();
        self.last_good_offset += (FRAME_PREFIX_BYTES + body_length) as u64;
        Ok(Some(record))
    }

    fn decode_record(&self, mut body: Vec<u8>) -> Result<Record> {
        let header_length = read_u32(&body[..HEADER_PREFIX_BYTES]) as usize;
        if header_length < 2 || header_length > body.len() - HEADER_PREFIX_BYTES {
            return Err(self.corruption("invalid journal frame header length"));
        }
        let data_offset = HEADER_PREFIX_BYTES + header_length;
        let header = String::from_utf8_lossy(&body[HEADER_PREFIX_BYTES..data_offset]);
        let header: Value = serde_json::from_str(&header)
            .map_err(|_| self.corruption("invalid journal frame header JSON"))?;
        let sequence = unsigned_integer(&header["sequence"])
            .filter(|&sequence| sequence > self.last_sequence && sequence <= MAX_SAFE_INTEGER)
            .ok_or_else(|| self.corruption("non-monotonic journal sequence"))?;
        let data_length = body.len() - data_offset;
        if unsigned_integer(&header["dataLength"]) != Some(data_length as u64) {
            return Err(self.corruption("journal payload length mismatch"));
        }
        let kind = header["kind"].as_str();
        if kind != Some("output") && data_length != 0 {
            return Err(self.corruption("control frame contains an unexpected payload"));
        }

        let invalid_fields = || self.corruption("invalid journal frame header fields");
        let timestamp = signed_integer(&header["timestamp"]).ok_or_else(invalid_fields)?;
        let frame = match kind {
            Some("output") => {
                body.drain(..data_offset);
                Frame::Output {
                    sequence,
                    data: body,
                }
            }
            Some("resize") => Frame::Resize {
                sequence,
                cols: u32::try_from(unsigned_integer(&header["cols"]).ok_or_else(invalid_fields)?)
                    .map_err(|_| invalid_fields())?,
                rows: u32::try_from(unsigned_integer(&header["rows"]).ok_or_else(invalid_fields)?)
                    .map_err(|_| invalid_fields())?,
            },
            Some("reset") => Frame::Reset {
                sequence,
                screen_epoch: unsigned_integer(&header["screenEpoch"])
                    .ok_or_else(invalid_fields)?,
            },
            Some("encoding") => Frame::Encoding {
                sequence,
                encoding: header["encoding"]
                    .as_str()
                    .ok_or_else(invalid_fields)?
                    .to_owned(),
            },
            Some("domain-cursor") => Frame::DomainCursor {
                sequence,
                domain_event_sequence: unsigned_integer(&header["domainEventSequence"])
                    .ok_or_else(invalid_fields)?,
            },
            Some("exit") => Frame::Exit {
                sequence,
                exit_code: i32::try_from(
                    signed_integer(&header["exitCode"]).ok_or_else(invalid_fields)?,
                )
                .map_err(|_| invalid_fields())?,
                signal: header
                    .get("signal")
                    .map(|signal| {
                        i32::try_from(signed_integer(signal).ok_or_else(invalid_fields)?)
                            .map_err(|_| invalid_fields())
                    })
                    .transpose()?,
            },
            _ => return Err(invalid_fields()),
        };
        Ok(Record { timestamp, frame })
    }

    fn corruption(&self, message: &str) -> anyhow::Error {
        JournalCorruptionError::new(message, self.last_good_offset).into()
    }

    fn torn_tail(&self, message: &str) -> Result<Option<Record>> {
        if self.tolerate_torn_tail {
            Ok(None)
        } else {
            Err(self.corruption(message))
        }
    }
}

impl<R: Read> Iterator for FrameReader<R> {
    type Item = Result<Record>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        let frame = self.read_frame().transpose();
        self.finished = !matches!(frame, Some(Ok(_)));
        frame
    }
}

fn read_u32(data: &[u8]) -> u32 {
    u32::from_be_bytes(data.try_into().expect("four-byte journal integer"))
}

fn read_exact_or_short(reader: &mut impl Read, buffer: &mut [u8]) -> io::Result<bool> {
    match reader.read_exact(buffer) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(error),
    }
}

fn read_first_byte(reader: &mut impl Read, buffer: &mut [u8]) -> io::Result<bool> {
    debug_assert_eq!(buffer.len(), 1);
    loop {
        match reader.read(buffer) {
            Ok(read) => return Ok(read != 0),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
}

fn unsigned_integer(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        let float = value.as_f64()?;
        ((0.0..=MAX_SAFE_INTEGER as f64).contains(&float) && float.fract() == 0.0)
            .then_some(float as u64)
    })
}

fn signed_integer(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| {
        let float = value.as_f64()?;
        let safe = MAX_SAFE_INTEGER as f64;
        ((-safe..=safe).contains(&float) && float.fract() == 0.0).then_some(float as i64)
    })
}

pub fn read_segment(path: &Path) -> Result<FrameReader<Box<dyn Read>>> {
    let file = File::open(path)?;
    let raw = BufReader::new(file);
    let source: Box<dyn Read> = if path.extension().is_some_and(|extension| extension == "gz") {
        Box::new(BufReader::new(MultiGzDecoder::new(raw)))
    } else {
        Box::new(raw)
    };
    FrameReader::new(source, false)
}

#[derive(Debug, PartialEq, Eq)]
pub struct RepairReport {
    pub truncated_bytes: u64,
    pub last_sequence: u64,
}

/// Repair an active raw segment or quarantine a segment with non-tail corruption.
pub fn repair_segment_tail(path: &Path, now: i64) -> Result<RepairReport> {
    let file = File::open(path)?;
    let original_len = file.metadata()?.len();
    let scan = (|| -> Result<(u64, u64)> {
        let mut frames = FrameReader::new(BufReader::new(file), true)?;
        for frame in frames.by_ref() {
            frame?;
        }
        Ok((frames.last_good_offset(), frames.last_sequence()))
    })();
    match scan {
        Ok((last_good_offset, last_sequence)) => {
            let truncated_bytes = original_len
                .checked_sub(last_good_offset)
                .ok_or_else(|| anyhow::anyhow!("journal grew during tail repair"))?;
            if truncated_bytes != 0 {
                OpenOptions::new()
                    .write(true)
                    .open(path)?
                    .set_len(last_good_offset)?;
            }
            Ok(RepairReport {
                truncated_bytes,
                last_sequence,
            })
        }
        Err(error) => match error.downcast::<JournalCorruptionError>() {
            Ok(mut corruption) => {
                let mut quarantine_path = path.as_os_str().to_owned();
                quarantine_path.push(format!(".corrupt-{now}"));
                fs::rename(path, &quarantine_path)?;
                corruption.quarantine_path = Some(quarantine_path.into());
                Err(corruption.into())
            }
            Err(error) => Err(error),
        },
    }
}
