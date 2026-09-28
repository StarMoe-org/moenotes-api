//! Protocol snapshot for the Android com.bilibili.sirius 1.0.1 sample.
//! Generated types are experimental. This is not an official API contract.

use prost_reflect::{DescriptorPool, DynamicMessage};
use std::sync::OnceLock;

#[allow(clippy::all)]
pub mod generated {
    include!(concat!(env!("OUT_DIR"), "/generated.rs"));
}

pub const DESCRIPTORS: &[u8] = include_bytes!("../descriptors.pb");
pub const JP_DESCRIPTORS: &[u8] = include_bytes!("../jp-descriptors.pb");

pub fn pool_for_region(region: &str) -> &'static DescriptorPool {
    if region == "jp" {
        static JP: OnceLock<DescriptorPool> = OnceLock::new();
        JP.get_or_init(|| {
            DescriptorPool::decode(JP_DESCRIPTORS).expect("validated JP protocol snapshot")
        })
    } else {
        pool()
    }
}

pub fn pool() -> &'static DescriptorPool {
    static POOL: OnceLock<DescriptorPool> = OnceLock::new();
    POOL.get_or_init(|| DescriptorPool::decode(DESCRIPTORS).expect("validated protocol snapshot"))
}

/// Uses protobuf JSON, including decimal strings for all 64-bit integers.
pub fn to_json(message: &DynamicMessage) -> Result<serde_json::Value, serde_json::Error> {
    serde_json::to_value(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;
    use prost_reflect::Value;

    #[test]
    fn snapshot_and_int64_json() {
        assert_eq!(pool().files().count(), 123);
        assert_eq!(
            pool().services().map(|s| s.methods().len()).sum::<usize>(),
            261
        );
        let mut message = DynamicMessage::new(
            pool()
                .get_message_by_name("app.gacha.ProbabilityRequest")
                .unwrap(),
        );
        message.set_field_by_name("gacha_id", Value::I64(9_007_199_254_740_993));
        message.set_field_by_name(
            "selected_pick_up",
            Value::List(vec![Value::I64(9), Value::I64(1), Value::I64(9)]),
        );
        let json = to_json(&message).unwrap();
        assert_eq!(json["gachaId"], "9007199254740993");
        assert_eq!(json["selectedPickUp"], serde_json::json!(["9", "1", "9"]));
    }

    #[test]
    fn unknown_fields_and_presence_survive_wire() {
        let desc = pool()
            .get_message_by_name("app.announcement.GetResponse")
            .unwrap();
        let absent = DynamicMessage::new(desc.clone());
        let present = DynamicMessage::decode(desc.clone(), &[10, 0][..]).unwrap();
        assert_ne!(to_json(&absent).unwrap(), to_json(&present).unwrap());
        let unknown = DynamicMessage::decode(desc, &[0x98, 0x06, 7][..]).unwrap();
        assert_eq!(unknown.encode_to_vec(), [0x98, 0x06, 7]);
        assert_eq!(to_json(&unknown).unwrap(), serde_json::json!({}));
    }

    #[test]
    fn enums_and_maps_use_protobuf_json() {
        let desc = pool()
            .get_message_by_name("app.announcement.GetListRequest")
            .unwrap();
        let msg = DynamicMessage::decode(desc, &[8, 99][..]).unwrap();
        assert_eq!(to_json(&msg).unwrap()["selectedTab"], 99);
        let desc = pool()
            .get_message_by_name("app.gacha.ProbabilityResponse")
            .unwrap();
        let msg = DynamicMessage::deserialize(
            desc,
            &serde_json::json!({"products":{"9007199254740993":7}}),
        )
        .unwrap();
        assert_eq!(to_json(&msg).unwrap()["products"]["9007199254740993"], 7);
    }
}
