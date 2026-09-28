use moenotes_proto::{generated::app, pool};
use prost::Message;
use prost_reflect::DynamicMessage;

use crate::{ClientError, ErrorKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Method {
    pub name: &'static str,
    pub path: &'static str,
    pub input: &'static str,
    pub output: &'static str,
    pub anonymous: bool,
    pub http: bool,
}

macro_rules! queries {
    ($( $variant:ident, $ty:path, $name:literal, $service:literal, $rpc:literal, $input:literal, $output:literal, $anonymous:literal, $http:literal; )*) => {
        /// Typed allowlist. No arbitrary RPC or write-operation escape hatch.
        #[derive(Clone, Debug)]
        pub enum Query { $( $variant($ty), )* }

        pub const METHODS: &[Method] = &[$(Method {
            name: $name, path: concat!("/", $service, "/", $rpc),
            input: $input, output: $output, anonymous: $anonymous, http: $http,
        },)*];

        impl Query {
            pub fn method(&self) -> &'static Method {
                let name = match self { $(Self::$variant(_) => $name,)* };
                METHODS.iter().find(|m| m.name == name).unwrap()
            }

            pub fn encode(&self) -> Vec<u8> {
                match self { $(Self::$variant(request) => request.encode_to_vec(),)* }
            }

            /// Strict protobuf JSON input; no unrecognized fields are accepted.
            pub fn from_json(name: &str, json: serde_json::Value) -> Result<Self, ClientError> {
                let method = METHODS.iter().find(|m| m.name == name).ok_or_else(invalid)?;
                let descriptor = pool().get_message_by_name(method.input).unwrap();
                let dynamic = DynamicMessage::deserialize(descriptor, json).map_err(|_| invalid())?;
                let bytes = dynamic.encode_to_vec();
                let query = match name {
                    $( $name => Self::$variant(<$ty>::decode(bytes.as_slice()).map_err(|_| invalid())?), )*
                    _ => return Err(invalid()),
                };
                query.validate()?;
                Ok(query)
            }
        }
    }
}

queries! {
    Announcement, app::announcement::GetRequest, "announcement", "app.announcement.AnnouncementService", "Get", "app.announcement.GetRequest", "app.announcement.GetResponse", true, true;
    Announcements, app::announcement::GetListRequest, "announcements", "app.announcement.AnnouncementService", "GetList", "app.announcement.GetListRequest", "app.announcement.GetListResponse", true, true;
    ArenaRanking, app::arena::ArenaRankingRequest, "arena-ranking", "app.arena.ArenaService", "ArenaRanking", "app.arena.ArenaRankingRequest", "app.arena.ArenaRankingResponse", false, true;
    DeckTrend, app::arena::DeckTrendRequest, "deck-trend", "app.arena.ArenaService", "DeckTrend", "app.arena.DeckTrendRequest", "app.arena.DeckTrendResponse", false, true;
    Circle, app::circle::GetCircleRequest, "circle", "app.circle.CircleService", "GetCircleDetail", "app.circle.GetCircleRequest", "app.circle.GetCircleResponse", false, true;
    CircleRecommendations, app::circle::GetRecommendedCircleListRequest, "circle-recommendations", "app.circle.CircleService", "GetRecommendedCircleList", "app.circle.GetRecommendedCircleListRequest", "app.circle.GetRecommendedCircleListResponse", false, true;
    CircleSearch, app::circle::SearchRequest, "circle-search", "app.circle.CircleService", "Search", "app.circle.SearchRequest", "app.circle.SearchResponse", false, true;
    ChallengeRanking, app::event::GetChallengeMusicRankingRequest, "challenge-ranking", "app.event.EventService", "GetChallengeMusicRanking", "app.event.GetChallengeMusicRankingRequest", "app.event.GetChallengeMusicRankingResponse", false, true;
    EventDeck, app::event::GetDeckRequest, "event-deck", "app.event.EventService", "GetDeck", "app.event.GetDeckRequest", "app.event.GetDeckResponse", false, true;
    EventRanking, app::event::GetRankingListRequest, "event-ranking", "app.event.EventService", "GetRankingList", "app.event.GetRankingListRequest", "app.event.GetRankingListResponse", false, true;
    Profile, app::friend::FindByProfileIdRequest, "profile", "app.friend.FriendService", "FindByProfileID", "app.friend.FindByProfileIDRequest", "app.friend.FindByProfileIDResponse", false, true;
    Probability, app::gacha::ProbabilityRequest, "probability", "app.gacha.GachaService", "Probability", "app.gacha.ProbabilityRequest", "app.gacha.ProbabilityResponse", false, true;
    MusicRanking, app::livemusic::GetRankingRequest, "music-ranking", "app.livemusic.LiveMusicService", "GetRanking", "app.livemusic.GetRankingRequest", "app.livemusic.GetRankingResponse", false, true;
    FavoriteStatus, app::player::GetPlayerFavoriteStatusRequest, "favorite-status", "app.player.PlayerService", "GetPlayerFavoriteStatus", "app.player.GetPlayerFavoriteStatusRequest", "app.player.GetPlayerFavoriteStatusResponse", false, true;
    Profiles, app::playerext::GetPlayerListRequest, "profiles", "app.playerext.PlayerExtService", "GetPlayerList", "app.playerext.GetPlayerListRequest", "app.playerext.GetPlayerListResponse", false, true;
    ServerList, app::playerlogin::GetServerListRequest, "server-list", "app.playerlogin.PlayerLoginService", "GetServerList", "app.playerlogin.GetServerListRequest", "app.playerlogin.GetServerListResponse", true, false;
    Version, app::masterdata::VersionRequest, "version", "app.masterdata.MasterdataService", "Version", "app.masterdata.VersionRequest", "app.masterdata.VersionResponse", true, false;
    Whoami, app::player::WhoamiRequest, "whoami", "app.player.PlayerService", "Whoami", "app.player.WhoamiRequest", "app.player.WhoamiResponse", false, false;
}

fn invalid() -> ClientError {
    ClientError::new(ErrorKind::InvalidRequest)
}

impl Query {
    pub fn method_for_region(&self, region: &str) -> Result<Method, ClientError> {
        let mut method = *self.method();
        if region == "jp" {
            if matches!(self, Self::ServerList(_)) {
                return Err(invalid());
            }
            if matches!(self, Self::Profiles(_)) {
                method.path = "/app.player.PlayerService/GetPlayerList";
                method.input = "app.player.GetPlayerListRequest";
                method.output = "app.player.GetPlayerListResponse";
            }
        }
        Ok(method)
    }

    pub fn validate(&self) -> Result<(), ClientError> {
        let valid = match self {
            Self::Announcement(r) => r.id > 0,
            Self::Announcements(r) => (0..=2).contains(&r.selected_tab),
            Self::ArenaRanking(r) => {
                r.arena_season_id > 0
                    && r.band_id.is_none_or(|v| v >= 0)
                    && r.ranking_start > 0
                    && r.ranking_end >= r.ranking_start
                    && r.ranking_end - r.ranking_start < 100
            }
            Self::DeckTrend(r) => r.music_id > 0 && r.arena_season_id > 0,
            Self::Circle(r) => r.circle_id > 0,
            Self::CircleSearch(r) => r.options.as_ref().is_some_and(|o| o.name.len() <= 256),
            Self::ChallengeRanking(r) => r.challenge_music_id > 0,
            Self::EventDeck(r) => r.event_id > 0 && valid_player(&r.player_id),
            Self::EventRanking(r) => {
                r.event_id > 0
                    && !r.ranks.is_empty()
                    && r.ranks.len() <= 100
                    && r.ranks.iter().all(|v| *v > 0)
            }
            Self::Profile(r) => r.player_profile_id > 0,
            #[allow(deprecated)]
            Self::Probability(r) => {
                r.gacha_id > 0
                    && r.product_id == 0
                    && r.selected_pick_up.len() <= 100
                    && r.selected_pick_up.iter().all(|v| *v > 0)
            }
            Self::MusicRanking(r) => r.music_id > 0,
            Self::FavoriteStatus(r) => valid_player(&r.player_id),
            Self::Profiles(r) => {
                !r.account_ids.is_empty()
                    && r.account_ids.len() <= 100
                    && r.account_ids.iter().all(|v| *v > 0)
            }
            _ => true,
        };
        if valid { Ok(()) } else { Err(invalid()) }
    }
}

fn valid_player(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256
}
