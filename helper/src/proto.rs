//! Wire protocol (version 1): one JSON request line in, zero or more streamed
//! lines and exactly one final line out. See AGENT_JOBS.md "Helper protocol".

use serde::Deserialize;
use serde_json::{json, Value};

pub const VERSION: u32 = 1;
pub const MAX_REQUEST: usize = 4096;

#[derive(Debug, Deserialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum Op {
    Status,
    Install,
    Remove,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    v: u32,
    op: Op,
    id: Option<String>,
    pin: Option<String>,
}

/// A request that passed structural validation.
pub enum Checked {
    Status,
    Change { op: Op, id: String, pin: String },
}

pub fn valid_pin(pin: &str) -> bool {
    (4..=8).contains(&pin.len()) && pin.bytes().all(|b| b.is_ascii_digit())
}

pub fn parse_request(line: &str) -> Result<Checked, String> {
    // Fixed text on purpose: serde's messages can echo input fragments, and the PIN must never leave.
    let req: Request = serde_json::from_str(line.trim()).map_err(|_| "malformed request".to_string())?;
    if req.v != VERSION {
        return Err(format!("unsupported protocol version {}", req.v));
    }
    if req.op == Op::Status {
        return Ok(Checked::Status);
    }
    let id = req.id.ok_or("missing id")?;
    let pin = req.pin.ok_or("missing pin")?;
    if !crate::allowlist::valid_id(&id) {
        return Err("bad id".into());
    }
    if !valid_pin(&pin) {
        return Err("pin must be 4 to 8 digits".into());
    }
    Ok(Checked::Change { op: req.op, id, pin })
}

pub fn line(text: &str) -> Value {
    json!({ "line": text })
}
pub fn replace(text: &str) -> Value {
    json!({ "replace": text })
}
pub fn step(text: &str) -> Value {
    json!({ "step": text })
}
pub fn done() -> Value {
    json!({ "done": true })
}
pub fn failed(code: i32, message: &str) -> Value {
    json!({ "failed": { "code": code, "message": message } })
}
pub fn bad_pin(tries_left: Option<u32>, locked_for_s: u64) -> Value {
    json!({ "bad_pin": { "tries_left": tries_left, "locked_for_s": locked_for_s } })
}
pub fn locked(locked_for_s: u64) -> Value {
    json!({ "locked": { "locked_for_s": locked_for_s } })
}
pub fn no_pin() -> Value {
    json!({ "no_pin": true })
}
pub fn not_allowed(reason: &str) -> Value {
    json!({ "not_allowed": reason })
}
pub fn busy() -> Value {
    json!({ "busy": true })
}
pub fn error(msg: &str) -> Value {
    json!({ "error": msg })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_and_install() {
        assert!(matches!(parse_request(r#"{"v":1,"op":"status"}"#), Ok(Checked::Status)));
        match parse_request(r#"{"v":1,"op":"install","id":"sl","pin":"1234"}"#) {
            Ok(Checked::Change { op, id, pin }) => assert_eq!((op, id.as_str(), pin.as_str()), (Op::Install, "sl", "1234")),
            _ => panic!("expected Change"),
        }
    }

    #[test]
    fn rejects_bad_requests() {
        for bad in [
            "",
            "not json",
            r#"{"v":2,"op":"status"}"#,
            r#"{"v":1,"op":"reboot"}"#,
            r#"{"v":1,"op":"status","extra":1}"#,
            r#"{"v":1,"op":"install","id":"sl"}"#,
            r#"{"v":1,"op":"install","pin":"1234"}"#,
            r#"{"v":1,"op":"install","id":"sl","pin":"123"}"#,
            r#"{"v":1,"op":"install","id":"sl","pin":"123456789"}"#,
            r#"{"v":1,"op":"remove","id":"sl","pin":"12a4"}"#,
            r#"{"v":1,"op":"install","id":"../x","pin":"1234"}"#,
            r#"{"v":1,"op":"install","id":"Sl","pin":"1234"}"#,
        ] {
            assert!(parse_request(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn errors_do_not_echo_input() {
        let e = parse_request(r#"{"v":1,"op":"install","id":"sl","pin":"98765"#).err().unwrap();
        assert!(!e.contains("98765"));
    }

    #[test]
    fn pin_boundaries_and_lookalike_digits() {
        for ok in ["1234", "12345678", "0000", "00000000"] {
            assert!(valid_pin(ok), "{ok}");
        }
        // 3 and 9 digits, whitespace, signs, and non-ASCII digits (4 chars but 8 bytes) are not PINs.
        for bad in ["", "123", "123456789", " 1234", "1234 ", "12 34", "+1234", "-1234", "1.234", "١٢٣٤", "１２３４", "1234\n"] {
            assert!(!valid_pin(bad), "{bad:?}");
        }
    }

    #[test]
    fn wrong_field_types_are_rejected() {
        for bad in [
            r#"{"v":"1","op":"status"}"#,
            r#"{"op":"status"}"#,
            r#"{"v":1}"#,
            r#"{"v":1,"op":"INSTALL","id":"sl","pin":"1234"}"#,
            r#"{"v":1,"op":"install","id":"sl","pin":1234}"#,
            r#"{"v":1,"op":"install","id":null,"pin":"1234"}"#,
            r#"{"v":1,"op":"install","id":"sl","pin":null}"#,
            r#"{"v":1,"op":"install","id":["sl"],"pin":"1234"}"#,
            r#"[{"v":1,"op":"status"}]"#,
            r#"{"v":1,"op":"status"} {"v":1,"op":"status"}"#,
        ] {
            assert!(parse_request(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn id_length_limit_and_surrounding_whitespace() {
        let long = format!(r#"{{"v":1,"op":"install","id":"{}","pin":"1234"}}"#, "a".repeat(65));
        assert!(parse_request(&long).is_err());
        let max = format!(r#"{{"v":1,"op":"remove","id":"{}","pin":"1234"}}"#, "a".repeat(64));
        assert!(matches!(parse_request(&max), Ok(Checked::Change { op: Op::Remove, .. })));
        assert!(parse_request("  {\"v\":1,\"op\":\"status\"}\r\n").is_ok());
    }

    #[test]
    fn no_rejection_message_contains_the_pin() {
        for bad in [
            r#"{"v":1,"op":"install","id":"../x","pin":"73519"}"#,
            r#"{"v":1,"op":"install","id":"sl","pin":"73519x"}"#,
            r#"{"v":2,"op":"install","id":"sl","pin":"73519"}"#,
            r#"{"v":1,"op":"reboot","id":"sl","pin":"73519"}"#,
            r#"{"v":1,"op":"install","id":"sl","pin":"73519","extra":1}"#,
            r#"{"v":1,"op":"install","pin":"73519"}"#,
        ] {
            let e = parse_request(bad).err().unwrap_or_else(|| panic!("accepted {bad}"));
            assert!(!e.contains("73519"), "{bad} -> {e}");
        }
    }

    #[test]
    fn final_lines_have_the_documented_shape() {
        assert_eq!(done().to_string(), r#"{"done":true}"#);
        assert_eq!(no_pin().to_string(), r#"{"no_pin":true}"#);
        assert_eq!(busy().to_string(), r#"{"busy":true}"#);
        assert_eq!(bad_pin(None, 60).to_string(), r#"{"bad_pin":{"locked_for_s":60,"tries_left":null}}"#);
        assert_eq!(bad_pin(Some(3), 0)["bad_pin"]["tries_left"], 3);
        assert_eq!(locked(7)["locked"]["locked_for_s"], 7);
        assert_eq!(failed(100, "E: x")["failed"]["code"], 100);
        assert_eq!(not_allowed("why")["not_allowed"], "why");
        assert_eq!(error("m")["error"], "m");
    }
}
