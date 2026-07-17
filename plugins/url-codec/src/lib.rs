//! Ferric 示例插件：URL 编解码。
//!
//! 构建：`cargo build --release --target wasm32-unknown-unknown`
//! 产物：`target/wasm32-unknown-unknown/release/ferric_plugin_url_codec.wasm`
//! 安装：拷贝到 `%APPDATA%/ferric/plugins/`，重启 Ferric 即出现在侧栏「插件」分组。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const MANIFEST: &str = r#"{
    "api_version": 1,
    "id": "url-codec",
    "name": "URL 编解码",
    "group": "插件",
    "desc": "URL 百分号编码 / 解码（RFC 3986）",
    "keywords": ["url", "encode", "decode", "编码", "解码", "percent"],
    "input_label": "输入",
    "output_label": "输出",
    "options": [
        {"kind": "seg", "key": "mode", "label": "方向", "values": ["编码", "解码"], "default": 0},
        {"kind": "toggle", "key": "space_plus", "label": "空格用 +（表单风格）", "default": false}
    ]
}"#;

#[derive(Deserialize)]
struct ProcessIn {
    input: String,
    #[serde(default)]
    options: BTreeMap<String, String>,
}

#[derive(Serialize)]
struct ProcessOut {
    ok: bool,
    output: String,
    error: String,
}

// ---- ABI ----

fn pack(buf: Vec<u8>) -> i64 {
    let b = buf.into_boxed_slice();
    let len = b.len() as u64;
    let ptr = Box::into_raw(b) as *mut u8 as u32 as u64;
    ((ptr << 32) | len) as i64
}

#[no_mangle]
pub extern "C" fn ferric_alloc(len: i32) -> i32 {
    let b = vec![0u8; len.max(0) as usize].into_boxed_slice();
    Box::into_raw(b) as *mut u8 as i32
}

#[no_mangle]
pub extern "C" fn ferric_dealloc(ptr: i32, len: i32) {
    if ptr == 0 || len < 0 {
        return;
    }
    unsafe {
        let s = std::slice::from_raw_parts_mut(ptr as *mut u8, len as usize);
        drop(Box::from_raw(s as *mut [u8]));
    }
}

#[no_mangle]
pub extern "C" fn ferric_manifest() -> i64 {
    pack(MANIFEST.as_bytes().to_vec())
}

#[no_mangle]
pub extern "C" fn ferric_process(ptr: i32, len: i32) -> i64 {
    let input = unsafe { std::slice::from_raw_parts(ptr as *const u8, len.max(0) as usize) };
    let out = handle(input);
    pack(serde_json::to_vec(&out).unwrap_or_else(|_| b"{\"ok\":false,\"output\":\"\",\"error\":\"serialize failed\"}".to_vec()))
}

// ---- 业务逻辑 ----

fn handle(raw: &[u8]) -> ProcessOut {
    let pin: ProcessIn = match serde_json::from_slice(raw) {
        Ok(p) => p,
        Err(e) => return fail(format!("入参解析失败：{e}")),
    };
    let mode = pin.options.get("mode").map(String::as_str).unwrap_or("编码");
    let space_plus = pin.options.get("space_plus").map(String::as_str) == Some("true");
    match mode {
        "解码" => match decode(&pin.input, space_plus) {
            Ok(s) => okay(s),
            Err(e) => fail(e),
        },
        _ => okay(encode(&pin.input, space_plus)),
    }
}

fn okay(output: String) -> ProcessOut {
    ProcessOut {
        ok: true,
        output,
        error: String::new(),
    }
}

fn fail(error: String) -> ProcessOut {
    ProcessOut {
        ok: false,
        output: String::new(),
        error,
    }
}

/// RFC 3986 非保留字符不编码，其余按 UTF-8 字节 %XX（大写）。
fn encode(s: &str, space_plus: bool) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' if space_plus => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn decode(s: &str, space_plus: bool) -> Result<String, String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = bytes
                    .get(i + 1..i + 3)
                    .ok_or_else(|| format!("位置 {i}：% 后不足两位"))?;
                let h = std::str::from_utf8(hex).map_err(|_| format!("位置 {i}：非法转义"))?;
                let v = u8::from_str_radix(h, 16).map_err(|_| format!("位置 {i}：%{h} 不是十六进制"))?;
                out.push(v);
                i += 3;
            }
            b'+' if space_plus => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| "解码结果不是有效 UTF-8".into())
}
