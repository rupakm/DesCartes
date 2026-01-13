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

fn put_u32(buf: &mut BytesMut, v: u32) {
    buf.put_u32_le(v);
}

fn get_u32(buf: &mut Bytes) -> Result<u32, &'static str> {
    if buf.remaining() < 4 {
        return Err("truncated");
    }
    Ok(buf.get_u32_le())
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
