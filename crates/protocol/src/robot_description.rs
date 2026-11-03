//! One complete, bounded robot-description document, transferred in ordered chunks.
//! Subscription/generation/sequence identities belong to the session envelope.
use serde::{Deserialize, Serialize};

pub const CONTRACT_VERSION: u32 = 1;
pub const MESSAGE_TYPE: &str = "std_msgs/msg/String";
pub const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;
pub const MAX_CHUNK_BYTES: usize = 192 * 1024;
pub const MAX_CHUNKS: usize = MAX_DOCUMENT_BYTES.div_ceil(MAX_CHUNK_BYTES);
pub const MAX_OWNERS: usize = 1;
pub const MAX_SAMPLE_BYTES: usize = crate::structured::MAX_SAMPLE_BYTES;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub contract_version: u32,
    pub max_document_bytes: usize,
    pub max_chunk_bytes: usize,
    pub max_chunks: usize,
    pub max_owners: usize,
}
impl Default for Capabilities {
    fn default() -> Self {
        Self {
            contract_version: CONTRACT_VERSION,
            max_document_bytes: MAX_DOCUMENT_BYTES,
            max_chunk_bytes: MAX_CHUNK_BYTES,
            max_chunks: MAX_CHUNKS,
            max_owners: MAX_OWNERS,
        }
    }
}

pub fn digest(bytes: &[u8]) -> String {
    crate::binary::sha256(bytes)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chunk {
    pub contract_version: u32,
    pub topic: String,
    pub message_type: String,
    pub index: usize,
    pub count: usize,
    pub document_bytes: usize,
    pub sha256: String,
    pub data_base64: String,
}
impl Chunk {
    fn raw_length(&self) -> Result<usize, String> {
        if self.contract_version != CONTRACT_VERSION
            || self.message_type != MESSAGE_TYPE
            || !crate::subscriptions::valid_topic(&self.topic)
            || !crate::binary::valid_sha256(&self.sha256)
        {
            return Err("Invalid robot-description chunk identity, bounds or digest".into());
        }
        crate::binary::chunk_length(
            self.document_bytes,
            self.index,
            self.count,
            MAX_DOCUMENT_BYTES,
            MAX_CHUNK_BYTES,
        )
    }
    pub fn validate(&self) -> Result<(), String> {
        crate::binary::validate_exact(&self.data_base64, self.raw_length()?, MAX_CHUNK_BYTES)
    }
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let mut bytes = crate::session::encode_line(self, MAX_SAMPLE_BYTES)?;
        bytes.pop();
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_SAMPLE_BYTES {
            return Err("Robot-description chunk exceeds sample budget".into());
        }
        let chunk: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        chunk.validate()?;
        Ok(chunk)
    }
}

/// Retain one document, producing a chunk only when its queue slot is available.
pub struct Document {
    topic: String,
    text: String,
    sha256: String,
}
impl Document {
    pub fn new(topic: &str, text: String) -> Result<Self, String> {
        if !crate::subscriptions::valid_topic(topic)
            || text.is_empty()
            || text.len() > MAX_DOCUMENT_BYTES
        {
            return Err("Robot description must be a nonempty UTF-8 document at most 1 MiB".into());
        }
        Ok(Self {
            topic: topic.into(),
            sha256: digest(text.as_bytes()),
            text,
        })
    }
    pub fn chunk(&self, index: usize) -> Option<Chunk> {
        let count = self.text.len().div_ceil(MAX_CHUNK_BYTES);
        if index >= count {
            return None;
        }
        // Chunks may split a UTF-8 code point: decode text only after reassembly.
        let raw = &self.text.as_bytes()
            [index * MAX_CHUNK_BYTES..((index + 1) * MAX_CHUNK_BYTES).min(self.text.len())];
        Some(Chunk {
            contract_version: CONTRACT_VERSION,
            topic: self.topic.clone(),
            message_type: MESSAGE_TYPE.into(),
            index,
            count,
            document_bytes: self.text.len(),
            sha256: self.sha256.clone(),
            data_base64: crate::binary::encode(raw),
        })
    }
}

/// One transfer, bound externally to an admitted generation/subscription owner.
/// Any error permanently poisons the transfer; no resynchronization or replay.
pub struct Reassembly {
    topic: String,
    next: usize,
    metadata: Option<(usize, String)>,
    bytes: Vec<u8>,
    finished: bool,
}
impl Reassembly {
    pub fn new(topic: String) -> Result<Self, String> {
        if !crate::subscriptions::valid_topic(&topic) {
            return Err("Invalid description topic".into());
        }
        Ok(Self {
            topic,
            next: 0,
            metadata: None,
            bytes: Vec::new(),
            finished: false,
        })
    }
    pub fn push(&mut self, chunk: &Chunk) -> Result<Option<String>, String> {
        let result = self.accept(chunk);
        if result.is_err() {
            self.finished = true;
            self.bytes.clear();
        }
        result
    }
    fn accept(&mut self, chunk: &Chunk) -> Result<Option<String>, String> {
        chunk.validate()?;
        if self.finished || chunk.topic != self.topic || chunk.index != self.next {
            return Err("Stale, repeated or out-of-order robot-description chunk".into());
        }
        if let Some((length, hash)) = &self.metadata {
            if *length != chunk.document_bytes || *hash != chunk.sha256 {
                return Err("Robot-description metadata changed during capture".into());
            }
        } else {
            self.metadata = Some((chunk.document_bytes, chunk.sha256.clone()));
            self.bytes.reserve_exact(chunk.document_bytes);
        }
        self.bytes.extend(crate::binary::decode_exact(
            &chunk.data_base64,
            chunk.raw_length()?,
            MAX_CHUNK_BYTES,
        )?);
        self.next += 1;
        if self.next != chunk.count {
            return Ok(None);
        }
        self.finished = true;
        if self.bytes.len() != chunk.document_bytes || digest(&self.bytes) != chunk.sha256 {
            return Err("Robot-description length or SHA-256 mismatch".into());
        }
        String::from_utf8(std::mem::take(&mut self.bytes))
            .map(Some)
            .map_err(|_| "Robot description is not UTF-8".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn transfer(text: String) -> (Document, Reassembly) {
        (
            Document::new("/robot_description", text).unwrap(),
            Reassembly::new("/robot_description".into()).unwrap(),
        )
    }
    #[test]
    fn exact_max_document_roundtrips_in_six_bounded_chunks() {
        // Deliberately straddle the chunk boundary with a multibyte character.
        let text = "x".repeat(MAX_CHUNK_BYTES - 1)
            + "é"
            + &"a".repeat(MAX_DOCUMENT_BYTES - MAX_CHUNK_BYTES - 1);
        let (document, mut receiver) = transfer(text.clone());
        for index in 0..MAX_CHUNKS {
            let chunk = document.chunk(index).unwrap();
            let encoded = chunk.encode().unwrap();
            assert!(encoded.len() < MAX_SAMPLE_BYTES);
            let output = receiver.push(&Chunk::decode(&encoded).unwrap()).unwrap();
            assert_eq!(output, (index == MAX_CHUNKS - 1).then(|| text.clone()));
        }
        assert!(document.chunk(MAX_CHUNKS).is_none());
        assert!(receiver.push(&document.chunk(0).unwrap()).is_err());
    }
    #[test]
    fn rejects_oversize_empty_and_hostile_chunk_metadata_before_allocation() {
        assert!(Document::new("/robot_description", String::new()).is_err());
        assert!(Document::new("/robot_description", "a".repeat(MAX_DOCUMENT_BYTES + 1)).is_err());
        let (document, _) = transfer("a".into());
        for mode in 0..10 {
            let mut chunk = document.chunk(0).unwrap();
            match mode {
                0 => chunk.document_bytes = usize::MAX,
                1 => chunk.index = usize::MAX,
                2 => chunk.count += 1,
                3 => chunk.message_type = "example/msg/String".into(),
                4 => chunk.contract_version += 1,
                5 => chunk.sha256 = "F".repeat(64),
                6 => chunk.topic.clear(),
                7 => chunk.data_base64 = "YR==".into(), // noncanonical trailing bits
                8 => chunk.data_base64.push('A'),
                _ => chunk.document_bytes = 0,
            }
            assert!(chunk.validate().is_err(), "mode {mode}");
        }
        let mut wire = serde_json::to_value(document.chunk(0).unwrap()).unwrap();
        wire["extra"] = true.into();
        assert!(Chunk::decode(&serde_json::to_vec(&wire).unwrap()).is_err());
    }
    #[test]
    fn rejects_gaps_duplicates_source_changes_and_metadata_changes_without_recovery() {
        let (document, _) = transfer("a".repeat(MAX_CHUNK_BYTES + 1));
        for mode in 0..5 {
            let mut receiver = Reassembly::new("/robot_description".into()).unwrap();
            let first = document.chunk(0).unwrap();
            if mode != 0 {
                assert!(receiver.push(&first).unwrap().is_none());
            }
            let mut bad = document.chunk(1).unwrap();
            match mode {
                0 => (), // missing first chunk
                1 => bad = first.clone(),
                2 => bad.topic = "/other".into(),
                3 => bad.sha256 = "0".repeat(64),
                _ => {
                    bad.document_bytes += 1;
                    bad.data_base64 = crate::binary::encode(b"aa");
                }
            }
            assert!(receiver.push(&bad).is_err(), "mode {mode}");
            assert!(receiver.push(&first).is_err());
            assert!(receiver.bytes.is_empty());
        }
    }
    #[test]
    fn digest_and_utf8_must_validate_before_a_complete_document_is_returned() {
        let (document, mut receiver) = transfer("a".into());
        let mut chunk = document.chunk(0).unwrap();
        chunk.data_base64 = crate::binary::encode(b"b");
        assert!(receiver.push(&chunk).unwrap_err().contains("SHA-256"));
        let (_, mut receiver) = transfer("a".into());
        chunk.data_base64 = crate::binary::encode(&[255]);
        chunk.sha256 = digest(&[255]);
        assert!(receiver.push(&chunk).unwrap_err().contains("UTF-8"));
    }
}
