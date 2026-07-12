//! UUID 生成：v4（随机）/ v7·v6（时间有序）/ v5（命名空间 + 名称）。

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// UUID 版本。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdKind {
    UuidV4,
    UuidV7,
    UuidV5,
    UuidV6,
}

impl IdKind {
    pub const ALL: [IdKind; 4] = [IdKind::UuidV4, IdKind::UuidV7, IdKind::UuidV5, IdKind::UuidV6];

    /// 段控标签。
    pub fn label(self) -> &'static str {
        match self {
            IdKind::UuidV4 => "v4 随机",
            IdKind::UuidV7 => "v7 时序",
            IdKind::UuidV5 => "v5 命名",
            IdKind::UuidV6 => "v6 时序",
        }
    }

    pub fn is_named(self) -> bool {
        matches!(self, IdKind::UuidV5)
    }
}

/// v5 命名空间。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Namespace {
    Dns,
    Url,
    Oid,
    X500,
    Custom,
}

impl Namespace {
    pub const ALL: [Namespace; 5] = [
        Namespace::Dns,
        Namespace::Url,
        Namespace::Oid,
        Namespace::X500,
        Namespace::Custom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Namespace::Dns => "DNS",
            Namespace::Url => "URL",
            Namespace::Oid => "OID",
            Namespace::X500 => "X.500",
            Namespace::Custom => "自定义…",
        }
    }

    /// 解析为命名空间 UUID；`Custom` 时解析 `custom`，失败返回 `None`。
    pub fn uuid(self, custom: &str) -> Option<Uuid> {
        match self {
            Namespace::Dns => Some(Uuid::NAMESPACE_DNS),
            Namespace::Url => Some(Uuid::NAMESPACE_URL),
            Namespace::Oid => Some(Uuid::NAMESPACE_OID),
            Namespace::X500 => Some(Uuid::NAMESPACE_X500),
            Namespace::Custom => Uuid::parse_str(custom.trim()).ok(),
        }
    }
}

/// 生成参数。
pub struct Opts<'a> {
    pub kind: IdKind,
    pub count: usize,
    pub namespace: Namespace,
    pub custom_ns: &'a str,
    pub name: &'a str,
    pub upper: bool,
    pub nohyphen: bool,
    pub as_json: bool,
}

fn random_node() -> [u8; 6] {
    let b = Uuid::new_v4();
    let s = b.as_bytes();
    [s[0], s[1], s[2], s[3], s[4], s[5]]
}

fn fmt(u: Uuid, upper: bool, nohyphen: bool) -> String {
    let mut s = if nohyphen {
        u.simple().to_string()
    } else {
        u.hyphenated().to_string()
    };
    if upper {
        s = s.to_uppercase();
    }
    s
}

/// 批量生成。v5 缺少有效命名空间时返回 `Err(提示)`。
pub fn generate(o: &Opts) -> Result<Vec<String>, String> {
    let count = o.count.clamp(1, 1000);
    if o.kind == IdKind::UuidV5 {
        let ns = o
            .namespace
            .uuid(o.custom_ns)
            .ok_or_else(|| "无效的自定义命名空间 UUID".to_owned())?;
        let one = fmt(Uuid::new_v5(&ns, o.name.as_bytes()), o.upper, o.nohyphen);
        // v5 对同名同命名空间是确定性的，count 份相同。
        return Ok(vec![one; count]);
    }
    let items = (0..count)
        .map(|_| {
            let u = match o.kind {
                IdKind::UuidV4 => Uuid::new_v4(),
                IdKind::UuidV7 => Uuid::now_v7(),
                IdKind::UuidV6 => Uuid::now_v6(&random_node()),
                IdKind::UuidV5 => unreachable!(),
            };
            fmt(u, o.upper, o.nohyphen)
        })
        .collect();
    Ok(items)
}

/// 生成并渲染为 Raw（换行分隔）或 JSON 数组；错误直接作为文本返回。
pub fn generate_string(o: &Opts) -> String {
    match generate(o) {
        Ok(items) => {
            if o.as_json {
                serde_json::to_string_pretty(&items).unwrap_or_default()
            } else {
                items.join("\n")
            }
        }
        Err(e) => format!("（{e}）"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(kind: IdKind, count: usize) -> Opts<'static> {
        Opts {
            kind,
            count,
            namespace: Namespace::Dns,
            custom_ns: "",
            name: "example.com",
            upper: false,
            nohyphen: false,
            as_json: false,
        }
    }

    #[test]
    fn generates_requested_count() {
        assert_eq!(generate(&opts(IdKind::UuidV4, 5)).unwrap().len(), 5);
    }

    #[test]
    fn v4_unique_and_valid() {
        let ids = generate(&opts(IdKind::UuidV4, 100)).unwrap();
        let set: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(set.len(), 100);
        for id in &ids {
            assert!(Uuid::parse_str(id).is_ok());
        }
    }

    #[test]
    fn v5_is_deterministic() {
        let a = generate(&opts(IdKind::UuidV5, 1)).unwrap();
        let b = generate(&opts(IdKind::UuidV5, 1)).unwrap();
        assert_eq!(a, b); // 同命名空间+名称 → 稳定
    }

    #[test]
    fn v5_custom_invalid_errors() {
        let mut o = opts(IdKind::UuidV5, 1);
        o.namespace = Namespace::Custom;
        o.custom_ns = "not-a-uuid";
        assert!(generate(&o).is_err());
    }

    #[test]
    fn v6_and_v7_valid() {
        for k in [IdKind::UuidV6, IdKind::UuidV7] {
            for id in generate(&opts(k, 5)).unwrap() {
                assert!(Uuid::parse_str(&id).is_ok());
            }
        }
    }

    #[test]
    fn nohyphen_and_upper() {
        let mut o = opts(IdKind::UuidV4, 1);
        o.nohyphen = true;
        o.upper = true;
        let id = &generate(&o).unwrap()[0];
        assert!(!id.contains('-'));
        assert_eq!(id, &id.to_uppercase());
    }
}
