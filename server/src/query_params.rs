use moenotes_client::{ClientError, ErrorKind, Query};
use prost_reflect::{FieldDescriptor, Kind, MessageDescriptor};
use serde_json::{Map, Value};

pub(crate) const MAX_QUERY_BYTES: usize = 8192;
const MAX_PARAMETERS: usize = 256;

fn invalid() -> ClientError {
    ClientError::new(ErrorKind::InvalidRequest)
}

pub(crate) fn fields(message: MessageDescriptor) -> Vec<(String, FieldDescriptor)> {
    fn collect(message: MessageDescriptor, prefix: &str, out: &mut Vec<(String, FieldDescriptor)>) {
        for field in message.fields() {
            let name = format!("{prefix}{}", field.json_name());
            if let Kind::Message(nested) = field.kind() {
                // The query allowlist currently has only singular nested filters.
                assert!(!field.is_list() && !field.is_map());
                collect(nested, &format!("{name}."), out);
            } else {
                out.push((name, field));
            }
        }
    }
    let mut out = Vec::new();
    collect(message, "", &mut out);
    out
}

pub(crate) fn parse(name: &str, raw: Option<&str>) -> Result<Query, ClientError> {
    let raw = raw.unwrap_or_default();
    if raw.len() > MAX_QUERY_BYTES {
        return Err(invalid());
    }
    // form_urlencoded is deliberately forgiving; reject malformed escaping/UTF-8 first.
    for (i, byte) in raw.bytes().enumerate() {
        if byte == b'%'
            && !raw
                .as_bytes()
                .get(i + 1..i + 3)
                .is_some_and(|pair| pair.iter().all(u8::is_ascii_hexdigit))
        {
            return Err(invalid());
        }
    }
    percent_encoding::percent_decode_str(raw)
        .decode_utf8()
        .map_err(|_| invalid())?;
    let method = moenotes_client::METHODS
        .iter()
        .find(|m| m.name == name)
        .ok_or_else(invalid)?;
    let descriptor = moenotes_proto::pool()
        .get_message_by_name(method.input)
        .unwrap();
    let allowed = fields(descriptor);
    let mut object = Map::new();
    // The game search request requires options presence even for an unfiltered query.
    if name == "circle-search" {
        object.insert("options".into(), Value::Object(Map::new()));
    }
    for (index, (key, value)) in url::form_urlencoded::parse(raw.as_bytes()).enumerate() {
        if index >= MAX_PARAMETERS {
            return Err(invalid());
        }
        let (_, field) = allowed
            .iter()
            .find(|(name, _)| name == &key)
            .ok_or_else(invalid)?;
        let value = scalar(field.kind(), &value)?;
        insert(
            &mut object,
            &key.split('.').collect::<Vec<_>>(),
            value,
            field.is_list(),
        )?;
    }
    Query::from_json(name, Value::Object(object))
}

fn insert(
    object: &mut Map<String, Value>,
    path: &[&str],
    value: Value,
    repeated: bool,
) -> Result<(), ClientError> {
    if path.len() > 1 {
        let nested = object
            .entry(path[0])
            .or_insert_with(|| Value::Object(Map::new()));
        return insert(
            nested.as_object_mut().ok_or_else(invalid)?,
            &path[1..],
            value,
            repeated,
        );
    }
    if repeated {
        object
            .entry(path[0])
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or_else(invalid)?
            .push(value);
    } else if object.insert(path[0].into(), value).is_some() {
        return Err(invalid());
    }
    Ok(())
}

fn scalar(kind: Kind, text: &str) -> Result<Value, ClientError> {
    fn digits(text: &str, signed: bool) -> bool {
        let value = if signed {
            text.strip_prefix('-').unwrap_or(text)
        } else {
            text
        };
        !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())
    }
    match kind {
        Kind::String => Ok(Value::String(text.into())),
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 if digits(text, true) => text
            .parse::<i64>()
            .map(|v| Value::String(v.to_string()))
            .map_err(|_| invalid()),
        Kind::Uint64 | Kind::Fixed64 if digits(text, false) => text
            .parse::<u64>()
            .map(|v| Value::String(v.to_string()))
            .map_err(|_| invalid()),
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 if digits(text, true) => {
            text.parse::<i32>().map(Value::from).map_err(|_| invalid())
        }
        Kind::Uint32 | Kind::Fixed32 if digits(text, false) => {
            text.parse::<u32>().map(Value::from).map_err(|_| invalid())
        }
        Kind::Enum(_) if digits(text, true) => {
            text.parse::<i32>().map(Value::from).map_err(|_| invalid())
        }
        Kind::Enum(_) => Ok(Value::String(text.into())),
        Kind::Bool if matches!(text, "true" | "false") => Ok(Value::Bool(text == "true")),
        _ => Err(invalid()),
    }
}

pub(crate) fn required(method: &str, field: &str) -> bool {
    match method {
        "announcement" => field == "id",
        "arena-ranking" => matches!(field, "arenaSeasonId" | "rankingStart" | "rankingEnd"),
        "deck-trend" => matches!(field, "musicId" | "arenaSeasonId"),
        "circle" => field == "circleId",
        "challenge-ranking" => field == "challengeMusicId",
        "challenge-total-ranking" => field == "eventId",
        "event-deck" => matches!(field, "eventId" | "playerId"),
        "event-ranking" => matches!(field, "eventId" | "ranks"),
        "profile" => field == "playerProfileId",
        "probability" => field == "gachaId",
        "music-ranking" => field == "musicId",
        "favorite-status" => field == "playerId",
        "profiles" => field == "accountIds",
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_values_preserve_order_duplicates_and_integer_precision() {
        let Query::EventRanking(r) = parse(
            "event-ranking",
            Some("eventId=9007199254740993&ranks=10&ranks=1&ranks=10"),
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(r.event_id, 9_007_199_254_740_993);
        assert_eq!(r.ranks, [10, 1, 10]);
        let Query::Circle(r) = parse("circle", Some("circleId=18446744073709551615")).unwrap()
        else {
            panic!()
        };
        assert_eq!(r.circle_id, u64::MAX);
        let Query::Profiles(r) = parse(
            "profiles",
            Some("accountIds=9007199254740993&accountIds=2&accountIds=2"),
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(r.account_ids, [9_007_199_254_740_993, 2, 2]);
        let Query::Probability(r) = parse(
            "probability",
            Some("gachaId=1&selectedPickUp=9&selectedPickUp=1&selectedPickUp=9"),
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(r.selected_pick_up, [9, 1, 9]);
    }

    #[test]
    fn empty_nested_options_and_optional_presence_are_preserved() {
        let Query::CircleSearch(r) = parse("circle-search", None).unwrap() else {
            panic!()
        };
        assert!(r.options.is_some());
        let Query::CircleSearch(r) = parse(
            "circle-search",
            Some("options.name=%E6%B5%8B%E8%AF%95%2B+team&options.memberRange=0"),
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(r.options.unwrap().name, "\u{6d4b}\u{8bd5}+ team");
        let base = "arenaSeasonId=1&rankingStart=1&rankingEnd=100";
        let Query::ArenaRanking(absent) = parse("arena-ranking", Some(base)).unwrap() else {
            panic!()
        };
        let Query::ArenaRanking(present) =
            parse("arena-ranking", Some(&format!("{base}&bandId=0"))).unwrap()
        else {
            panic!()
        };
        assert_eq!(absent.band_id, None);
        assert_eq!(present.band_id, Some(0));
        assert_eq!(
            parse("announcements", Some("selectedTab=BUG"))
                .unwrap()
                .encode(),
            parse("announcements", Some("selectedTab=2"))
                .unwrap()
                .encode()
        );
    }

    #[test]
    fn malformed_unknown_duplicate_and_out_of_range_parameters_are_rejected() {
        for (method, raw) in [
            ("profile", "playerProfileId=1&playerProfileId=2"),
            ("profile", "playerProfileId=1&player_profile_id=1"),
            ("profile", "playerProfileId=1&token=secret"),
            ("profile", "playerProfileId=9223372036854775808"),
            ("profile", "playerProfileId=1e3"),
            ("profile", "playerProfileId=1.0"),
            ("profile", "playerProfileId="),
            ("challenge-total-ranking", "eventId=0"),
            ("challenge-total-ranking", "eventId=1&eventId=2"),
            ("challenge-total-ranking", "eventId="),
            ("circle", "circleId=-1"),
            ("circle", "circleId=18446744073709551616"),
            ("event-ranking", "eventId=1&ranks=1,2"),
            ("event-ranking", "eventId=1&ranks[]=1"),
            ("event-ranking", "eventId=1&ranks=2147483648"),
            ("announcements", "selectedTab=3"),
            ("announcements", "selectedTab=invalid"),
            ("circle-search", "options.name=%"),
            ("circle-search", "options.name=%0"),
            ("circle-search", "options.name=%GG"),
            ("circle-search", "options.name=%ff"),
            ("circle-search", "options.name=%C0%AF"),
            ("circle-search", "options.name=a&options.name=b"),
            ("circle-search", "options={}"),
            ("circle-search", "options.unknown=a"),
            ("circle-recommendations", "unknown=1"),
        ] {
            assert!(parse(method, Some(raw)).is_err(), "{method}: {raw}");
        }
        assert!(parse("profile", None).is_err());
        assert!(parse("event-ranking", Some("eventId=1")).is_err());
        assert!(parse("profiles", Some("")).is_err());
    }

    #[test]
    fn uri_and_list_limits_apply_before_upstream() {
        let raw = format!("options.name={}", "a".repeat(MAX_QUERY_BYTES));
        assert!(parse("circle-search", Some(&raw)).is_err());
        for count in [101, 257] {
            let raw = format!("eventId=1{}", "&ranks=1".repeat(count));
            assert!(parse("event-ranking", Some(&raw)).is_err());
        }
        assert!(
            parse(
                "event-ranking",
                Some(&format!("eventId=1{}", "&ranks=1".repeat(100)))
            )
            .is_ok()
        );
    }
}
