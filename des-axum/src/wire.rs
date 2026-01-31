use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HttpRequestWire {
    pub method: String,
    pub uri: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HttpResponseWire {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

pub fn encode_request(req: &HttpRequestWire) -> Result<Vec<u8>, bincode::Error> {
    bincode::serialize(req)
}

pub fn decode_request(bytes: &[u8]) -> Result<HttpRequestWire, bincode::Error> {
    bincode::deserialize(bytes)
}

pub fn encode_response(resp: &HttpResponseWire) -> Result<Vec<u8>, bincode::Error> {
    bincode::serialize(resp)
}

pub fn decode_response(bytes: &[u8]) -> Result<HttpResponseWire, bincode::Error> {
    bincode::deserialize(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_roundtrip() {
        let req = HttpRequestWire {
            method: "GET".to_string(),
            uri: "/hello?x=1".to_string(),
            headers: vec![("x-test".to_string(), "1".to_string())],
            body: b"hi".to_vec(),
        };

        let enc = encode_request(&req).expect("encode request");
        let dec = decode_request(&enc).expect("decode request");
        assert_eq!(dec, req);

        let resp = HttpResponseWire {
            status: 200,
            headers: vec![("content-type".to_string(), "text/plain".to_string())],
            body: b"ok".to_vec(),
        };

        let enc = encode_response(&resp).expect("encode response");
        let dec = decode_response(&enc).expect("decode response");
        assert_eq!(dec, resp);
    }
}
