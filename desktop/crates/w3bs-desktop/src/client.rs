use crate::transport::{self, Duplex, Endpoint};
use adl_schema::rpc::{Request, Response};
use adl_schema::{AdlError, ErrorCode, Result, SCHEMA_VERSION};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};

pub struct Client {
    reader: BufReader<Box<dyn Duplex>>,
    writer: Box<dyn Duplex>,
    next: u64,
    /// Events that arrived while waiting for a response (subscribed connections).
    pub events: Vec<Value>,
}

impl Client {
    pub fn connect(ep: &Endpoint) -> Result<Self> {
        let s = transport::connect(ep).map_err(|e| AdlError::new(ErrorCode::Blocked, format!("cannot reach the broker at {ep} ({e}); start it with `w3bs desktop daemon`")))?;
        Ok(Self { reader: BufReader::new(s.try_clone_box().map_err(|e| AdlError::internal(e.to_string()))?), writer: s, next: 1, events: vec![] })
    }

    pub fn send(&mut self, method: &str, params: Value) -> Result<String> {
        let id = self.next.to_string();
        self.next += 1;
        let req = Request { v: SCHEMA_VERSION, id: id.clone(), method: method.into(), params };
        let mut line = serde_json::to_string(&req).unwrap();
        line.push('\n');
        self.writer.write_all(line.as_bytes()).map_err(|e| AdlError::internal(format!("broker connection: {e}")))?;
        self.writer.flush().map_err(|e| AdlError::internal(format!("broker connection: {e}")))?;
        Ok(id)
    }

    pub fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.send(method, params)?;
        loop {
            let mut line = String::new();
            let n = self.reader.read_line(&mut line).map_err(|e| AdlError::internal(format!("broker connection: {e}")))?;
            if n == 0 {
                return Err(AdlError::internal("broker closed the connection"));
            }
            let v: Value = serde_json::from_str(&line).map_err(|e| AdlError::internal(format!("bad response: {e}")))?;
            if v.get("event").is_some() {
                self.events.push(v);
                continue;
            }
            let r: Response = serde_json::from_value(v).map_err(|e| AdlError::internal(format!("bad response: {e}")))?;
            if r.id != id {
                continue;
            }
            return match (r.result, r.error) {
                (_, Some(e)) => Err(e),
                (Some(v), None) => Ok(v),
                (None, None) => Ok(Value::Null),
            };
        }
    }
}
