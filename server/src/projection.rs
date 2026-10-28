//! Fail-closed public field policy, independent from future descriptor additions.
use moenotes_client::{ClientError, ErrorKind};
use prost_reflect::{Kind, MessageDescriptor};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResponseMode {
    Disabled,
    #[default]
    Public,
    Raw,
}

fn allowed(message: &str) -> &'static [&'static str] {
    match message {
        "app.announcement.GetResponse" => &["announcement"],
        "app.announcement.GetListResponse" => &["announcements", "externalContents"],
        "entity.Announcement" => &[
            "id",
            "category",
            "title",
            "bannerPath",
            "bannerUrl",
            "body",
            "startAt",
            "endAt",
            "sortOrder",
            "pickupBannerPath",
            "pickupBannerUrl",
            "pickupStartAt",
            "pickupEndAt",
            "lastUpdatedAt",
            "stylesheetId",
        ],
        "entity.AnnouncementExternalContent" => &["id", "bannerPath"],
        "app.arena.ArenaRankingResponse" => &["ranking", "maxRank"],
        "entity.ArenaRankingEntry" => &["profile", "deck", "rank", "point"],
        "entity.PlayerSimpleProfile" => &[
            "id",
            "name",
            "rankExp",
            "lastUpdatedAt",
            "favoriteMemberCard",
            "profileId",
            "favoriteMemberCardMasterId",
            "profileCard",
        ],
        "entity.DeckMemberCardDetail" => &[
            "cardId",
            "exp",
            "awakeCount",
            "cardRank",
            "liveSkillLevel",
            "performanceSkillLevel",
        ],
        "entity.SimpleProfileCard" => &["slot", "name", "thumbnailUrl"],
        "entity.DeckDetail" => &["id", "name", "cards", "totalPower"],
        "entity.DeckCardDetail" => &[
            "slotIndex",
            "performanceOrderIndex",
            "memberCard",
            "supportCard",
        ],
        "entity.DeckSupportCardDetail" => &["cardId", "exp", "rank"],
        "app.arena.DeckTrendResponse" => &["leaders", "members", "supports"],
        "entity.ArenaDeckTrendMemberCard" => &["memberCardId"],
        "entity.ArenaDeckTrendSupportCard" => &["supportCardId"],
        "app.circle.GetCircleResponse" => &["circle"],
        "entity.CircleDetailWithPlayerList" => &["detail", "players", "circleRanking"],
        "entity.Circle" => &[
            "id",
            "name",
            "description",
            "joinRule",
            "playStyle",
            "totalPoint",
            "weeklyTotalPoint",
            "memberCount",
            "masterLastLoginAt",
            "currentRank",
            "nextRankPoint",
        ],
        "entity.CirclePlayer" => &[
            "playerId",
            "circleId",
            "auth",
            "point",
            "weeklyTotalPoint",
            "profile",
        ],
        "app.circle.SearchResponse" => &["circles"],
        "entity.CircleWithMasterPlayer" => &["circle", "profile"],
        "app.event.GetChallengeMusicRankingResponse"
        | "app.event.GetChallengeMusicTotalRankingResponse"
        | "app.livemusic.GetRankingResponse" => &["players"],
        "app.event.ChallengeLiveRankingPlayer"
        | "app.event.ChallengeMusicTotalRankingPlayer"
        | "app.livemusic.LiveRankingPlayer" => &["playerData", "score", "highScoreDeck"],
        "app.event.GetDeckResponse" => &["deck"],
        "app.event.GetRankingListResponse" => &["ranking"],
        "app.event.EventRankingEntry" => &["profile", "rank", "point"],
        "app.friend.FindByProfileIDResponse" => &["playerProfile"],
        "app.gacha.ProbabilityResponse" => &[
            "lot",
            "prize",
            "ensuredLot",
            "ensuredPrize",
            "probabilities",
            "products",
            "ensuredProducts",
        ],
        "entity.LotProbability" => &["lotId", "probabilityPercent"],
        "entity.PrizeProbability" => &["prizeId", "probabilityPercent"],
        "entity.Probability" => &["lot", "prize"],
        "app.player.GetPlayerFavoriteStatusResponse" => &["totalFavorite"],
        "app.playerext.GetPlayerListResponse" => &["players"],
        "entity.PlayerBriefInfo" => &[
            "accountId",
            "playerId",
            "nickname",
            "level",
            "chatFrameId",
            "chatBubbleId",
            "chatThemeId",
            "avatarId",
        ],
        _ => &[],
    }
}
pub fn permitted(mode: ResponseMode, method: &str) -> bool {
    mode != ResponseMode::Public || method != "circle-recommendations"
}
pub fn project(method: &str, value: &Value) -> Result<Value, ClientError> {
    let method = moenotes_client::METHODS
        .iter()
        .find(|m| m.name == method)
        .ok_or_else(invalid)?;
    object(
        moenotes_proto::pool()
            .get_message_by_name(method.output)
            .ok_or_else(invalid)?,
        value,
    )
}
fn invalid() -> ClientError {
    ClientError::new(ErrorKind::Protocol)
}
fn object(desc: MessageDescriptor, value: &Value) -> Result<Value, ClientError> {
    let input = value.as_object().ok_or_else(invalid)?;
    let mut out = Map::new();
    for name in allowed(desc.full_name()) {
        if let Some(value) = input.get(*name) {
            let field = desc
                .fields()
                .find(|f| f.json_name() == *name)
                .ok_or_else(invalid)?;
            let projected = if field.is_map() {
                let Kind::Message(entry) = field.kind() else {
                    return Err(invalid());
                };
                let kind = entry.get_field_by_name("value").ok_or_else(invalid)?.kind();
                let mut map = Map::new();
                for (key, v) in value.as_object().ok_or_else(invalid)? {
                    map.insert(key.clone(), scalar(kind.clone(), v)?);
                }
                Value::Object(map)
            } else if field.is_list() {
                Value::Array(
                    value
                        .as_array()
                        .ok_or_else(invalid)?
                        .iter()
                        .map(|v| scalar(field.kind(), v))
                        .collect::<Result<_, _>>()?,
                )
            } else {
                scalar(field.kind(), value)?
            };
            out.insert((*name).into(), projected);
        }
    }
    Ok(Value::Object(out))
}
fn scalar(kind: Kind, value: &Value) -> Result<Value, ClientError> {
    match kind {
        Kind::Message(desc) => object(desc, value),
        _ => Ok(value.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn personal_and_unknown_fields_are_removed_recursively() {
        let value = json!({"myRank":1,"newSecret":"no","players":[{"score":9,"newSecret":"no","playerData":{"name":"synthetic","profileId":"9007199254740993","credential":"no"},"highScoreDeck":{"cards":[{"memberCard":{"cardId":"1","credential":"no"}}]}}]});
        let out = project("music-ranking", &value).unwrap();
        assert!(!out.to_string().contains("no"));
        assert!(out.get("myRank").is_none());
        assert_eq!(
            out["players"][0]["playerData"]["profileId"],
            "9007199254740993"
        );
        assert_eq!(
            project(
                "favorite-status",
                &json!({"totalFavorite":"9","isSentFavorite":true})
            )
            .unwrap(),
            json!({"totalFavorite":"9"})
        );
        assert!(!permitted(ResponseMode::Public, "circle-recommendations"));
    }
    #[test]
    fn challenge_total_ranking_drops_personal_fields_but_keeps_rows() {
        let value = json!({
            "players": [
                {"playerData": {"name": "synthetic", "profileId": "9007199254740993", "credential": "no"},
                 "score": 123456, "highScoreDeck": {"id": "1", "totalPower": "9", "credential": "no"},
                 "newSecret": "no"},
                {"score": 1}
            ],
            "myRank": 7,
            "myScore": 123456
        });
        let out = project("challenge-total-ranking", &value).unwrap();
        assert_eq!(
            out,
            json!({"players": [
                {"playerData": {"name": "synthetic", "profileId": "9007199254740993"},
                 "score": 123456, "highScoreDeck": {"id": "1", "totalPower": "9"}},
                {"score": 1}
            ]})
        );
        assert!(!out.to_string().contains("no"));
    }
    #[test]
    fn maps_and_presence_remain_intact() {
        assert_eq!(project("announcement", &json!({})).unwrap(), json!({}));
        assert_eq!(
            project("announcement", &json!({"announcement":{}})).unwrap(),
            json!({"announcement":{}})
        );
        assert_eq!(project("probability",&json!({"products":{"9007199254740993":1},"probabilities":{"1":{"prize":[{"prizeId":"7","probabilityPercent":"0.01","newField":3}]}}})).unwrap(),json!({"products":{"9007199254740993":1},"probabilities":{"1":{"prize":[{"prizeId":"7","probabilityPercent":"0.01"}]}}}));
    }
}
