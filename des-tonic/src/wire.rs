use bytes::{Buf, BufMut, Bytes, BytesMut};
use tonic::{Code, Status};

#[derive(Debug, Clone)]
pub struct UnaryRequestWire {
    pub method: String,
    pub metadata: Vec<(String, String)>,
    pub payload: Bytes,
}

#[derive(Debug, Clone)]
pub struct UnaryResponseWire {
    pub ok: bool,
    pub code: Code,
    pub message: String,
    pub metadata: Vec<(String, String)>,
    pub payload: Bytes,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamFrameKind {
    Open,
    Data,
    Close,
    Cancel,
}

impl StreamFrameKind {
    fn to_u8(self) -> u8 {
        match self {
            StreamFrameKind::Open => 0,
            StreamFrameKind::Data => 1,
            StreamFrameKind::Close => 2,
            StreamFrameKind::Cancel => 3,
        }
    }

    fn from_u8(v: u8) -> Result<Self, &'static str> {
        match v {
            0 => Ok(StreamFrameKind::Open),
            1 => Ok(StreamFrameKind::Data),
            2 => Ok(StreamFrameKind::Close),
            3 => Ok(StreamFrameKind::Cancel),
            _ => Err("invalid StreamFrameKind"),
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamDirection {
    ClientToServer,
    ServerToClient,
}

impl StreamDirection {
    fn to_u8(self) -> u8 {
        match self {
            StreamDirection::ClientToServer => 0,
            StreamDirection::ServerToClient => 1,
        }
    }

    fn from_u8(v: u8) -> Result<Self, &'static str> {
        match v {
            0 => Ok(StreamDirection::ClientToServer),
            1 => Ok(StreamDirection::ServerToClient),
            _ => Err("invalid StreamDirection"),
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamFrameWire {
    pub stream_id: String,
    pub direction: StreamDirection,
    pub seq: u64,
    pub kind: StreamFrameKind,

    pub method: Option<String>,
    pub metadata: Vec<(String, String)>,
    pub payload: Bytes,

    pub status_ok: bool,
    pub status_code: Code,
    pub status_message: String,
}

fn put_u32(buf: &mut BytesMut, v: u32) {
    buf.put_u32_le(v);
}

fn get_u32(buf: &mut Bytes) -> Result<u32, &'static str> {
    if buf.remaining() < 4 {
        return Err("truncated");
    }
    Ok(buf.get_u32_le())
}

#[allow(dead_code)]
fn put_u64(buf: &mut BytesMut, v: u64) {
    buf.put_u64_le(v);
}

#[allow(dead_code)]
fn get_u64(buf: &mut Bytes) -> Result<u64, &'static str> {
    if buf.remaining() < 8 {
        return Err("truncated");
    }
    Ok(buf.get_u64_le())
}

#[allow(dead_code)]
fn get_u8(buf: &mut Bytes) -> Result<u8, &'static str> {
    if buf.remaining() < 1 {
        return Err("truncated");
    }
    Ok(buf.get_u8())
}

fn put_bytes(buf: &mut BytesMut, bytes: &[u8]) {
    put_u32(buf, bytes.len() as u32);
    buf.put_slice(bytes);
}

fn get_bytes(buf: &mut Bytes) -> Result<Bytes, &'static str> {
    let len = get_u32(buf)? as usize;
    if buf.remaining() < len {
        return Err("truncated");
    }
    Ok(buf.split_to(len))
}

fn put_string(buf: &mut BytesMut, s: &str) {
    put_bytes(buf, s.as_bytes());
}

fn get_string(buf: &mut Bytes) -> Result<String, &'static str> {
    let b = get_bytes(buf)?;
    std::str::from_utf8(&b)
        .map(|s| s.to_string())
        .map_err(|_| "invalid utf-8")
}

fn put_kv(buf: &mut BytesMut, entries: &[(String, String)]) {
    put_u32(buf, entries.len() as u32);
    for (k, v) in entries {
        put_string(buf, k);
        put_string(buf, v);
    }
}

fn get_kv(buf: &mut Bytes) -> Result<Vec<(String, String)>, &'static str> {
    let n = get_u32(buf)? as usize;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let k = get_string(buf)?;
        let v = get_string(buf)?;
        out.push((k, v));
    }
    Ok(out)
}

pub fn encode_unary_request(msg: &UnaryRequestWire) -> Bytes {
    let mut buf = BytesMut::new();
    put_string(&mut buf, &msg.method);
    put_kv(&mut buf, &msg.metadata);
    put_bytes(&mut buf, &msg.payload);
    buf.freeze()
}

pub fn decode_unary_request(mut bytes: Bytes) -> Result<UnaryRequestWire, Status> {
    let method = get_string(&mut bytes).map_err(|e| Status::internal(e))?;
    let metadata = get_kv(&mut bytes).map_err(|e| Status::internal(e))?;
    let payload = get_bytes(&mut bytes).map_err(|e| Status::internal(e))?;
    Ok(UnaryRequestWire {
        method,
        metadata,
        payload,
    })
}

pub fn encode_unary_response(msg: &UnaryResponseWire) -> Bytes {
    let mut buf = BytesMut::new();
    buf.put_u8(if msg.ok { 1 } else { 0 });
    put_u32(&mut buf, msg.code as u32);
    put_string(&mut buf, &msg.message);
    put_kv(&mut buf, &msg.metadata);
    put_bytes(&mut buf, &msg.payload);
    buf.freeze()
}

pub fn decode_unary_response(mut bytes: Bytes) -> Result<UnaryResponseWire, Status> {
    if bytes.remaining() < 1 {
        return Err(Status::internal("truncated"));
    }
    let ok = bytes.get_u8() != 0;
    let code_u32 = get_u32(&mut bytes).map_err(|e| Status::internal(e))?;
    let code = Code::from_i32(code_u32 as i32);
    let message = get_string(&mut bytes).map_err(|e| Status::internal(e))?;
    let metadata = get_kv(&mut bytes).map_err(|e| Status::internal(e))?;
    let payload = get_bytes(&mut bytes).map_err(|e| Status::internal(e))?;

    Ok(UnaryResponseWire {
        ok,
        code,
        message,
        metadata,
        payload,
    })
}

#[allow(dead_code)]
pub fn encode_stream_frame(frame: &StreamFrameWire) -> Bytes {
    let mut buf = BytesMut::new();

    put_string(&mut buf, &frame.stream_id);
    buf.put_u8(frame.direction.to_u8());
    put_u64(&mut buf, frame.seq);
    buf.put_u8(frame.kind.to_u8());

    match frame.kind {
        StreamFrameKind::Open => {
            if let Some(method) = &frame.method {
                buf.put_u8(1);
                put_string(&mut buf, method);
            } else {
                // Encode an invalid frame; decoder will reject.
                buf.put_u8(0);
            }
        }
        _ => {
            buf.put_u8(0);
        }
    }

    put_kv(&mut buf, &frame.metadata);
    put_bytes(&mut buf, &frame.payload);

    if frame.kind == StreamFrameKind::Close {
        buf.put_u8(if frame.status_ok { 1 } else { 0 });
        put_u32(&mut buf, frame.status_code as u32);
        put_string(&mut buf, &frame.status_message);
    }

    buf.freeze()
}

#[allow(dead_code)]
pub fn decode_stream_frame(mut bytes: Bytes) -> Result<StreamFrameWire, Status> {
    let stream_id = get_string(&mut bytes).map_err(Status::internal)?;

    let direction_u8 = get_u8(&mut bytes).map_err(Status::internal)?;
    let direction = StreamDirection::from_u8(direction_u8).map_err(Status::internal)?;

    let seq = get_u64(&mut bytes).map_err(Status::internal)?;

    let kind_u8 = get_u8(&mut bytes).map_err(Status::internal)?;
    let kind = StreamFrameKind::from_u8(kind_u8).map_err(Status::internal)?;

    let method_present = get_u8(&mut bytes).map_err(Status::internal)?;
    let method = if method_present == 0 {
        None
    } else if method_present == 1 {
        Some(get_string(&mut bytes).map_err(Status::internal)?)
    } else {
        return Err(Status::internal("invalid method flag"));
    };

    if kind == StreamFrameKind::Open {
        if method.is_none() {
            return Err(Status::internal("Open frame missing method"));
        }
    } else if method.is_some() {
        return Err(Status::internal("method only allowed for Open"));
    }

    let metadata = get_kv(&mut bytes).map_err(Status::internal)?;
    let payload = get_bytes(&mut bytes).map_err(Status::internal)?;

    let (status_ok, status_code, status_message) = if kind == StreamFrameKind::Close {
        let status_ok = get_u8(&mut bytes).map_err(Status::internal)? != 0;
        let code_u32 = get_u32(&mut bytes).map_err(Status::internal)?;
        let status_code = Code::from_i32(code_u32 as i32);
        let status_message = get_string(&mut bytes).map_err(Status::internal)?;
        (status_ok, status_code, status_message)
    } else {
        (true, Code::Ok, String::new())
    };

    Ok(StreamFrameWire {
        stream_id,
        direction,
        seq,
        kind,
        method,
        metadata,
        payload,
        status_ok,
        status_code,
        status_message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(frame: StreamFrameWire) {
        let encoded = encode_stream_frame(&frame);
        let decoded = decode_stream_frame(encoded).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn stream_frame_round_trip_open() {
        round_trip(StreamFrameWire {
            stream_id: "s1".to_string(),
            direction: StreamDirection::ClientToServer,
            seq: 0,
            kind: StreamFrameKind::Open,
            method: Some("/svc/Method".to_string()),
            metadata: vec![("k".to_string(), "v".to_string())],
            payload: Bytes::new(),
            status_ok: true,
            status_code: Code::Ok,
            status_message: String::new(),
        });
    }

    #[test]
    fn stream_frame_round_trip_data() {
        round_trip(StreamFrameWire {
            stream_id: "s1".to_string(),
            direction: StreamDirection::ClientToServer,
            seq: 1,
            kind: StreamFrameKind::Data,
            method: None,
            metadata: Vec::new(),
            payload: Bytes::from_static(b"hello"),
            status_ok: true,
            status_code: Code::Ok,
            status_message: String::new(),
        });
    }

    #[test]
    fn stream_frame_round_trip_close() {
        round_trip(StreamFrameWire {
            stream_id: "s1".to_string(),
            direction: StreamDirection::ServerToClient,
            seq: 2,
            kind: StreamFrameKind::Close,
            method: None,
            metadata: Vec::new(),
            payload: Bytes::new(),
            status_ok: false,
            status_code: Code::NotFound,
            status_message: "nope".to_string(),
        });
    }

    #[test]
    fn stream_frame_round_trip_cancel() {
        round_trip(StreamFrameWire {
            stream_id: "s1".to_string(),
            direction: StreamDirection::ClientToServer,
            seq: 3,
            kind: StreamFrameKind::Cancel,
            method: None,
            metadata: Vec::new(),
            payload: Bytes::new(),
            status_ok: true,
            status_code: Code::Ok,
            status_message: String::new(),
        });
    }

    #[test]
    fn stream_frame_decode_truncated() {
        let frame = StreamFrameWire {
            stream_id: "s1".to_string(),
            direction: StreamDirection::ClientToServer,
            seq: 1,
            kind: StreamFrameKind::Data,
            method: None,
            metadata: Vec::new(),
            payload: Bytes::from_static(b"hello"),
            status_ok: true,
            status_code: Code::Ok,
            status_message: String::new(),
        };

        let encoded = encode_stream_frame(&frame);
        let truncated = encoded.slice(..encoded.len() - 1);
        let err = decode_stream_frame(truncated).unwrap_err();
        assert_eq!(err.code(), Code::Internal);
    }

    #[test]
    fn stream_frame_decode_invalid_kind() {
        let mut buf = BytesMut::new();
        put_string(&mut buf, "s1");
        buf.put_u8(StreamDirection::ClientToServer.to_u8());
        put_u64(&mut buf, 0);
        buf.put_u8(99);
        buf.put_u8(0);
        put_kv(&mut buf, &[] as &[(String, String)]);
        put_bytes(&mut buf, &[]);

        let err = decode_stream_frame(buf.freeze()).unwrap_err();
        assert_eq!(err.code(), Code::Internal);
    }

    #[test]
    fn stream_frame_decode_open_missing_method() {
        let mut buf = BytesMut::new();
        put_string(&mut buf, "s1");
        buf.put_u8(StreamDirection::ClientToServer.to_u8());
        put_u64(&mut buf, 0);
        buf.put_u8(StreamFrameKind::Open.to_u8());
        buf.put_u8(0); // missing method
        put_kv(&mut buf, &[] as &[(String, String)]);
        put_bytes(&mut buf, &[]);

        let err = decode_stream_frame(buf.freeze()).unwrap_err();
        assert_eq!(err.code(), Code::Internal);
    }
}
