//! JSON-RPC IDと[`ExternalId`]の可逆変換。数値と文字列を区別して保持する（`5`と`"5"`は別ID）。

use crate::backend::model::ExternalId;
use serde_json::{Number, Value};

/// 元の型を保ったJSON-RPC ID。
#[derive(Debug, Clone, PartialEq)]
pub enum RpcId {
    Num(Number),
    Str(String),
}

impl RpcId {
    /// JSON値から取り出す。数値・文字列以外（null等）は `None`。
    pub fn from_value(v: &Value) -> Option<RpcId> {
        match v {
            Value::Number(n) => Some(RpcId::Num(n.clone())),
            Value::String(s) => Some(RpcId::Str(s.clone())),
            _ => None,
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            RpcId::Num(n) => Value::Number(n.clone()),
            RpcId::Str(s) => Value::String(s.clone()),
        }
    }

    pub fn encode(&self) -> ExternalId {
        match self {
            RpcId::Num(n) => ExternalId(format!("n:{n}")),
            RpcId::Str(s) => ExternalId(format!("s:{s}")),
        }
    }

    /// 符号化済みIDを元の型へ戻す。形式不正は `None`。
    pub fn decode(id: &ExternalId) -> Option<RpcId> {
        if let Some(rest) = id.0.strip_prefix("n:") {
            match serde_json::from_str::<Value>(rest).ok()? {
                Value::Number(n) => Some(RpcId::Num(n)),
                _ => None,
            }
        } else {
            id.0.strip_prefix("s:").map(|s| RpcId::Str(s.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn roundtrip_number_and_string_are_distinct() {
        let n = RpcId::from_value(&json!(5)).unwrap();
        let s = RpcId::from_value(&json!("5")).unwrap();
        assert_eq!(n.encode().0, "n:5");
        assert_eq!(s.encode().0, "s:5");
        assert_ne!(n.encode(), s.encode());
        assert_eq!(RpcId::decode(&n.encode()).unwrap().to_value(), json!(5));
        assert_eq!(RpcId::decode(&s.encode()).unwrap().to_value(), json!("5"));
    }

    #[test]
    fn roundtrip_odd_strings_and_large_numbers() {
        for v in [json!("a:b:c"), json!(""), json!("n:7"), json!(u64::MAX), json!(-3)] {
            let id = RpcId::from_value(&v).unwrap();
            assert_eq!(RpcId::decode(&id.encode()).unwrap().to_value(), v);
        }
    }

    #[test]
    fn rejects_invalid() {
        assert!(RpcId::from_value(&Value::Null).is_none());
        assert!(RpcId::decode(&ExternalId("x:1".into())).is_none());
        assert!(RpcId::decode(&ExternalId("n:abc".into())).is_none());
    }
}
