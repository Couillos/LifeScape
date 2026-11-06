#![forbid(unsafe_code)]
//! Minimal JSON value model, std-only.
//!
//! Single parse/write point for the workspace: daemon/web payloads,
//! game data dumps and dev check reports. Objects keep sorted
//! keys (like JSON `sort_keys`), plus arrays, strings, ints,
//! floats, bools and null.

use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Arr(Vec<Value>),
    Obj(BTreeMap<String, Value>),
}

impl Value {
    pub fn get(&self, key: &str) -> &Value {
        match self {
            Value::Obj(map) => map.get(key).unwrap_or(&Value::Null),
            _ => &Value::Null,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(text) => Some(text),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(num) => Some(*num),
            Value::Float(num) => Some(*num as i64),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(num) => Some(*num as f64),
            Value::Float(num) => Some(*num),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(flag) => Some(*flag),
            _ => None,
        }
    }

    pub fn as_arr(&self) -> Option<&Vec<Value>> {
        match self {
            Value::Arr(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_obj(&self) -> Option<&BTreeMap<String, Value>> {
        match self {
            Value::Obj(map) => Some(map),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
}

pub fn str_value(text: &str) -> Value {
    Value::Str(text.to_string())
}

pub fn object(pairs: Vec<(&str, Value)>) -> Value {
    let mut map = BTreeMap::new();
    for (key, value) in pairs {
        map.insert(key.to_string(), value);
    }
    Value::Obj(map)
}

pub fn parse(text: &str) -> Result<Value, String> {
    let mut parser = Parser {
        bytes: text.as_bytes(),
        pos: 0,
        depth: 0,
    };
    parser.skip_ws();
    let value = parser.parse_value()?;
    parser.skip_ws();
    if parser.pos != parser.bytes.len() {
        return Err("trailing characters".to_string());
    }
    Ok(value)
}

pub fn stringify(value: &Value) -> String {
    let mut out = String::new();
    write_value(value, &mut out, false, 0);
    out
}

pub fn stringify_pretty(value: &Value) -> String {
    let mut out = String::new();
    write_value(value, &mut out, true, 0);
    out
}

fn escape_into(text: &str, out: &mut String) {
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            ch if (ch as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", ch as u32));
            }
            ch => out.push(ch),
        }
    }
}

fn write_number(num: f64, out: &mut String) {
    if num.is_finite() && num.fract() == 0.0 && num.abs() < 1e21 {
        out.push_str(&format!("{num:.1}"));
    } else {
        out.push_str(&format!("{num}"));
    }
}

fn write_value(value: &Value, out: &mut String, pretty: bool, indent: usize) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Int(num) => out.push_str(&num.to_string()),
        Value::Float(num) => write_number(*num, out),
        Value::Str(text) => {
            out.push('"');
            escape_into(text, out);
            out.push('"');
        }
        Value::Arr(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                if pretty {
                    out.push('\n');
                    out.push_str(&"  ".repeat(indent + 1));
                } else if index > 0 {
                    out.push(' ');
                }
                write_value(item, out, pretty, indent + 1);
            }
            if pretty {
                out.push('\n');
                out.push_str(&"  ".repeat(indent));
            }
            out.push(']');
        }
        Value::Obj(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            let mut first = true;
            for (key, item) in map {
                if !first {
                    out.push(',');
                    if !pretty {
                        out.push(' ');
                    }
                }
                first = false;
                if pretty {
                    out.push('\n');
                    out.push_str(&"  ".repeat(indent + 1));
                }
                out.push('"');
                escape_into(key, out);
                out.push_str("\": ");
                write_value(item, out, pretty, indent + 1);
            }
            if pretty {
                out.push('\n');
                out.push_str(&"  ".repeat(indent));
            }
            out.push('}');
        }
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    depth: usize,
}

impl Parser<'_> {
    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn parse_value(&mut self) -> Result<Value, String> {
        self.depth += 1;
        if self.depth > 128 {
            return Err("too deeply nested".to_string());
        }
        let value = match self.peek() {
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'"') => Ok(Value::Str(self.parse_string()?)),
            Some(b't') => self.parse_literal("true", Value::Bool(true)),
            Some(b'f') => self.parse_literal("false", Value::Bool(false)),
            Some(b'n') => self.parse_literal("null", Value::Null),
            Some(ch) if ch == b'-' || ch.is_ascii_digit() => self.parse_number(),
            Some(ch) => Err(format!("unexpected character '{}'", ch as char)),
            None => Err("unexpected end of input".to_string()),
        };
        self.depth -= 1;
        value
    }

    fn parse_literal(&mut self, word: &str, value: Value) -> Result<Value, String> {
        if self.bytes[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Ok(value)
        } else {
            Err(format!("invalid literal near '{word}'"))
        }
    }

    fn parse_object(&mut self) -> Result<Value, String> {
        self.pos += 1; // {
        let mut map = BTreeMap::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Value::Obj(map));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err("expected string key".to_string());
            }
            let key = self.parse_string()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err("expected ':'".to_string());
            }
            self.pos += 1;
            self.skip_ws();
            let value = self.parse_value()?;
            map.insert(key, value);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Value::Obj(map));
                }
                _ => return Err("expected ',' or '}'".to_string()),
            }
        }
    }

    fn parse_array(&mut self) -> Result<Value, String> {
        self.pos += 1; // [
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Value::Arr(items));
        }
        loop {
            self.skip_ws();
            items.push(self.parse_value()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Value::Arr(items));
                }
                _ => return Err("expected ',' or ']'".to_string()),
            }
        }
    }

    fn parse_string(&mut self) -> Result<String, String> {
        self.pos += 1; // opening quote
        let mut out = String::new();
        loop {
            // Bulk-copy up to the next quote or escape: validating the
            // whole remaining buffer per character is quadratic.
            let rest = &self.bytes[self.pos..];
            let mut end = 0;
            while end < rest.len() && rest[end] != b'"' && rest[end] != b'\\' {
                end += 1;
            }
            if end == rest.len() {
                return Err("unterminated string".to_string());
            }
            out.push_str(std::str::from_utf8(&rest[..end]).map_err(|_| "invalid utf-8")?);
            self.pos += end;
            match rest[end] {
                b'"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                _ => {
                    self.pos += 1; // backslash
                    let esc = *self.bytes.get(self.pos).ok_or("unterminated escape")?;
                    self.pos += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{08}'),
                        b'f' => out.push('\u{0C}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => out.push(self.parse_unicode()?),
                        _ => return Err("invalid escape".to_string()),
                    }
                }
            }
        }
    }

    fn parse_unicode(&mut self) -> Result<char, String> {
        let high = self.parse_hex4()?;
        let code: u32 = if (0xD800..0xDC00).contains(&high) {
            if self.bytes.get(self.pos..self.pos + 2) == Some(b"\\u".as_slice()) {
                self.pos += 2;
                let low = self.parse_hex4()?;
                if !(0xDC00..0xE000).contains(&low) {
                    return Err("invalid low surrogate".to_string());
                }
                0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00)
            } else {
                return Err("missing low surrogate".to_string());
            }
        } else {
            high
        };
        char::from_u32(code).ok_or_else(|| "invalid code point".to_string())
    }

    fn parse_hex4(&mut self) -> Result<u32, String> {
        if self.pos + 4 > self.bytes.len() {
            return Err("unterminated \\u escape".to_string());
        }
        let mut code: u32 = 0;
        for byte in &self.bytes[self.pos..self.pos + 4] {
            code = code * 16
                + (*byte as char)
                    .to_digit(16)
                    .ok_or_else(|| "invalid \\u escape".to_string())?;
        }
        self.pos += 4;
        Ok(code)
    }

    fn parse_number(&mut self) -> Result<Value, String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while self.peek().is_some_and(|ch| ch.is_ascii_digit()) {
            self.pos += 1;
        }
        let mut is_float = false;
        if self.peek() == Some(b'.') {
            is_float = true;
            self.pos += 1;
            while self.peek().is_some_and(|ch| ch.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            is_float = true;
            self.pos += 1;
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                self.pos += 1;
            }
            while self.peek().is_some_and(|ch| ch.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        let raw = std::str::from_utf8(&self.bytes[start..self.pos]).map_err(|_| "bad number")?;
        if raw.is_empty() || raw == "-" {
            return Err("bad number".to_string());
        }
        if !is_float {
            if let Ok(num) = raw.parse::<i64>() {
                return Ok(Value::Int(num));
            }
        }
        raw.parse::<f64>()
            .map(Value::Float)
            .map_err(|_| "bad number".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_sorted_keys() {
        let value = parse(r#"{"b":[1, 2.5, true, null],"a":"x"}"#).unwrap();
        assert_eq!(stringify(&value), r#"{"a": "x", "b": [1, 2.5, true, null]}"#);
    }

    #[test]
    fn matches_reference_separators() {
        // reference: json.dumps({"a": {"y": {"k": "v"}, "z": 1.0},
        //   "b": [1, 2.5, True, None, "x"], "f": 0.5, "n": -3}, sort_keys=True)
        let value = parse(r#"{"a": {"y": {"k": "v"}, "z": 1.0}, "b": [1, 2.5, true, null, "x"], "f": 0.5, "n": -3}"#).unwrap();
        assert_eq!(
            stringify(&value),
            r#"{"a": {"y": {"k": "v"}, "z": 1.0}, "b": [1, 2.5, true, null, "x"], "f": 0.5, "n": -3}"#
        );
    }

    #[test]
    fn escapes_and_unicode() {
        let value = parse(r#""a\n\u00e9\ud83d\ude00""#).unwrap();
        assert_eq!(value, Value::Str("a\né😀".to_string()));
        assert_eq!(stringify(&value), r#""a\né😀""#);
    }

    #[test]
    fn rejects_trailing_and_depth() {
        assert!(parse("{} x").is_err());
        let mut deep = String::new();
        for _ in 0..200 {
            deep.push('[');
        }
        assert!(parse(&deep).is_err());
    }
}
