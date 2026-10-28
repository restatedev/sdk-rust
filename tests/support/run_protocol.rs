// Minimal fixtures for the public service-invocation protocol V6/V7.
// Field numbers follow restatedev/service-protocol; these types are independent
// of the VM implementation so the tests exercise the SDK's streaming endpoint.
use bytes::{Buf, Bytes, BytesMut};
use prost::Message;
use std::collections::VecDeque;

pub const START: u16 = 0;
pub const SUSPENSION: u16 = 1;
pub const ERROR: u16 = 2;
pub const END: u16 = 3;
pub const PROPOSAL: u16 = 5;
pub const AWAITING: u16 = 6;
pub const ACK: u16 = 7;
pub const INPUT: u16 = 0x0400;
pub const OUTPUT: u16 = 0x0401;
pub const RUN: u16 = 0x0411;
pub const COMPLETION: u16 = 0x8011;
pub const SIGNAL: u16 = 0xfbff;

#[derive(Clone, PartialEq, Message)]
pub struct Start {
    #[prost(bytes = "bytes", tag = "1")]
    pub id: Bytes,
    #[prost(string, tag = "2")]
    pub debug_id: String,
    #[prost(uint32, tag = "3")]
    pub known_entries: u32,
    #[prost(uint32, tag = "7")]
    pub retry_count: u32,
}
#[derive(Clone, PartialEq, Message)]
pub struct Value {
    #[prost(bytes = "bytes", tag = "1")]
    pub content: Bytes,
}
#[derive(Clone, PartialEq, Message)]
pub struct Failure {
    #[prost(uint32, tag = "1")]
    pub code: u32,
    #[prost(string, tag = "2")]
    pub message: String,
}
#[derive(Clone, PartialEq, Message)]
pub struct Input {
    #[prost(message, optional, tag = "14")]
    pub value: Option<Value>,
}
#[derive(Clone, PartialEq, Message)]
pub struct Output {
    #[prost(message, optional, tag = "14")]
    pub value: Option<Value>,
    #[prost(message, optional, tag = "15")]
    pub failure: Option<Failure>,
}
#[derive(Clone, PartialEq, Message)]
pub struct Run {
    #[prost(uint32, tag = "11")]
    pub completion_id: u32,
    #[prost(string, tag = "12")]
    pub name: String,
}
#[derive(Clone, PartialEq, Message)]
pub struct Proposal {
    #[prost(uint32, tag = "1")]
    pub completion_id: u32,
    #[prost(bytes = "bytes", optional, tag = "14")]
    pub value: Option<Bytes>,
    #[prost(message, optional, tag = "15")]
    pub failure: Option<Failure>,
}
#[derive(Clone, PartialEq, Message)]
pub struct Completion {
    #[prost(uint32, tag = "1")]
    pub completion_id: u32,
    #[prost(message, optional, tag = "5")]
    pub value: Option<Value>,
    #[prost(message, optional, tag = "6")]
    pub failure: Option<Failure>,
}
#[derive(Clone, PartialEq, Message)]
pub struct Ack {
    #[prost(uint32, tag = "1")]
    pub completion_id: u32,
}
#[derive(Clone, PartialEq, Message)]
pub struct Error {
    #[prost(uint32, tag = "1")]
    pub code: u32,
    #[prost(string, tag = "2")]
    pub message: String,
    #[prost(uint32, optional, tag = "4")]
    pub command_index: Option<u32>,
    #[prost(uint64, optional, tag = "8")]
    pub next_retry_delay: Option<u64>,
}
#[derive(Clone, PartialEq, Message)]
pub struct Signal {
    #[prost(uint32, optional, tag = "2")]
    pub index: Option<u32>,
    #[prost(message, optional, tag = "6")]
    pub failure: Option<Failure>,
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub kind: u16,
    pub requests_ack: bool,
    pub payload: Bytes,
}
impl Frame {
    pub fn decode<M: Message + Default>(&self) -> M {
        M::decode(self.payload.clone()).expect("invalid protocol message")
    }
}
pub fn encode<M: Message>(kind: u16, message: &M) -> Bytes {
    let payload = message.encode_to_vec();
    let header = ((kind as u64) << 48) | payload.len() as u64;
    let mut result = Vec::with_capacity(8 + payload.len());
    result.extend_from_slice(&header.to_be_bytes());
    result.extend_from_slice(&payload);
    result.into()
}
pub fn decode(buffer: &mut BytesMut, frames: &mut VecDeque<Frame>) {
    while buffer.len() >= 8 {
        let header = u64::from_be_bytes(buffer[..8].try_into().unwrap());
        let size = header as u32 as usize;
        if buffer.len() < 8 + size {
            break;
        }
        buffer.advance(8);
        frames.push_back(Frame {
            kind: (header >> 48) as u16,
            requests_ack: header & 0x8000_0000_0000 != 0,
            payload: buffer.split_to(size).freeze(),
        });
    }
}
pub fn completion(proposal: &Proposal) -> Bytes {
    encode(
        COMPLETION,
        &Completion {
            completion_id: proposal.completion_id,
            value: proposal.value.clone().map(|content| Value { content }),
            failure: proposal.failure.clone(),
        },
    )
}

#[derive(Clone, PartialEq, Message)]
pub struct Void {}
#[derive(Clone, PartialEq, Message)]
pub struct SleepCompletion {
    #[prost(uint32, tag = "1")]
    pub completion_id: u32,
    #[prost(message, optional, tag = "4")]
    pub void: Option<Void>,
}
