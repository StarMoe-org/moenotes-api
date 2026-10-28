//! Protocol snapshot for the Android com.bilibili.sirius 1.1.0 sample.
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
        assert_eq!(pool().files().count(), 125);
        assert_eq!(
            pool().services().map(|s| s.methods().len()).sum::<usize>(),
            276
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

    #[test]
    fn challenge_total_ranking_is_reflected() {
        let service = pool()
            .get_service_by_name("app.event.EventService")
            .unwrap();
        let rpc = service
            .methods()
            .find(|m| m.name() == "GetChallengeMusicTotalRanking")
            .unwrap();
        assert_eq!(
            rpc.input().full_name(),
            "app.event.GetChallengeMusicTotalRankingRequest"
        );
        assert_eq!(
            rpc.output().full_name(),
            "app.event.GetChallengeMusicTotalRankingResponse"
        );
        let request = rpc.input();
        let event = request.get_field_by_name("event_id").unwrap();
        assert_eq!(event.number(), 1);
        assert_eq!(event.kind(), prost_reflect::Kind::Int64);
        let response = rpc.output();
        let players = response.get_field_by_name("players").unwrap();
        assert!(players.is_list());
        assert_eq!(players.number(), 1);
        assert_eq!(
            players.kind().as_message().unwrap().full_name(),
            "app.event.ChallengeMusicTotalRankingPlayer"
        );
        assert_eq!(response.get_field_by_name("my_rank").unwrap().number(), 2);
        assert_eq!(response.get_field_by_name("my_score").unwrap().number(), 3);
        let row = pool()
            .get_message_by_name("app.event.ChallengeMusicTotalRankingPlayer")
            .unwrap();
        assert_eq!(
            row.get_field_by_name("player_data")
                .unwrap()
                .kind()
                .as_message()
                .unwrap()
                .full_name(),
            "entity.PlayerSimpleProfile"
        );
        assert_eq!(row.get_field_by_name("score").unwrap().number(), 2);
        assert_eq!(
            row.get_field_by_name("high_score_deck")
                .unwrap()
                .kind()
                .as_message()
                .unwrap()
                .full_name(),
            "entity.DeckDetail"
        );
    }

    /// The 1.1.0 client descriptors no longer contain these; the snapshot keeps
    /// them so the existing gateway surface does not silently lose an endpoint.
    #[test]
    fn methods_absent_from_1_1_0_are_retained() {
        let player = pool()
            .get_service_by_name("app.player.PlayerService")
            .unwrap();
        let whoami = player
            .methods()
            .find(|m| m.name() == "Whoami")
            .expect("Whoami retained from 1.0.1");
        assert_eq!(whoami.input().full_name(), "app.player.WhoamiRequest");
        assert_eq!(whoami.output().full_name(), "app.player.WhoamiResponse");
        assert!(
            pool()
                .get_message_by_name("app.player.WhoamiRequest")
                .is_some()
        );
        assert_eq!(
            pool()
                .get_message_by_name("app.player.WhoamiResponse")
                .unwrap()
                .get_field_by_name("player_id")
                .unwrap()
                .kind(),
            prost_reflect::Kind::String
        );
        let membercard = pool()
            .get_service_by_name("app.membercard.MemberCardService")
            .unwrap();
        assert!(membercard.methods().any(|m| m.name() == "LinkSkillLevelUp"));
    }
}
