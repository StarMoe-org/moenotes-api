use prost_reflect::{Kind, MessageDescriptor};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub fn document_for(mode: crate::projection::ResponseMode) -> Value {
    let mut paths = BTreeMap::new();
    let mut schemas = BTreeMap::new();
    for &(path, name) in crate::ROUTES {
        let method = moenotes_client::METHODS
            .iter()
            .find(|m| m.name == name)
            .unwrap();
        for message in [method.input, method.output] {
            schema(
                moenotes_proto::pool().get_message_by_name(message).unwrap(),
                &mut schemas,
            );
        }
        let parameters: Vec<_> = crate::query_params::fields(
            moenotes_proto::pool().get_message_by_name(method.input).unwrap()
        ).into_iter().map(|(field_name, field)| {
            let mut value = kind(field.kind(), &mut schemas);
            if field.is_list() {
                value = json!({"type":"array","items":value,"maxItems":100});
                if crate::query_params::required(name, &field_name) {
                    value["minItems"] = json!(1);
                }
            }
            json!({"name":field_name,"in":"query","required":crate::query_params::required(name, &field_name),
                "style":"form","explode":true,"deprecated":field.options().get_field_by_name("deprecated").is_some_and(|v|v.as_bool()==Some(true)),
                "schema":value,"description":if field.is_list() {"Repeat the parameter for each item; order and duplicates are preserved."} else {"Single occurrence only. Nested filters use dotted field names."}})
        }).collect();
        paths.insert(path, json!({"get":{
            "operationId":name,"tags":["read queries"],
            "description":"No request body. Public mode removes unapproved/account-specific fields; raw mode is operator-only. Query string limited to 8192 bytes and 256 parameters.",
            "security":[{"apiKey":[]}],
            "parameters":parameters,
            "responses":{
                "200":{"description":"Raw protobuf JSON; 64-bit integers are strings. Fetch time is Unix milliseconds.",
                    "headers":{"X-Moenotes-Cache":{"schema":{"type":"string","enum":["HIT","MISS","COALESCED"]}},"X-Moenotes-Fetched-At":{"schema":{"type":"string"}}},
                    "content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{}",method.output)}}}},
                "400":{"description":"Invalid query"},"401":{"description":"Missing or invalid HTTP API key"},
                "403":{"description":"Route prohibited by response policy"},"404":{"description":"Route disabled or unknown"},"405":{"description":"Only GET is supported; HEAD does not query upstream"},"429":{"description":"Local request queue full"},
                "502":{"description":"Upstream business, transport or protocol error"},
                "503":{"description":"Upstream session or availability blocked"},"504":{"description":"Request deadline exceeded"}
            }
        }}));
    }
    if mode == crate::projection::ResponseMode::Public {
        paths.remove("/v1/circles/recommended");
        for (name, field) in [
            ("app.event.GetChallengeMusicRankingResponse", "myRank"),
            ("app.event.GetChallengeMusicRankingResponse", "myScore"),
            ("app.livemusic.GetRankingResponse", "myRank"),
            (
                "app.player.GetPlayerFavoriteStatusResponse",
                "isSentFavorite",
            ),
        ] {
            if let Some(properties) = schemas
                .get_mut(name)
                .and_then(|v| v.get_mut("properties"))
                .and_then(Value::as_object_mut)
            {
                properties.remove(field);
            }
        }
    }
    for path in ["/v1/status", "/readyz"] {
        paths.insert(path,json!({"get":{"security":[{"apiKey":[]}],"responses":{"200":{"description":"Sanitized local operational state"},"401":{"description":"Missing API key"},"503":{"description":"Not ready"}}}}));
    }
    let mut paths: BTreeMap<String, Value> = paths
        .into_iter()
        .map(|(path, value)| (path.to_owned(), value))
        .collect();
    let existing = paths.clone();
    for route in crate::path_routes::ROUTES {
        let Some((legacy, _)) = crate::ROUTES.iter().find(|(_, name)| *name == route.method) else {
            continue;
        };
        let Some(source) = existing.get(*legacy) else {
            continue;
        };
        for region in [None, Some("tw"), Some("en"), Some("kr"), Some("jp")] {
            let path = region
                .map(|r| format!("/v1/{r}{}", route.path.trim_start_matches("/v1")))
                .unwrap_or_else(|| route.path.to_owned());
            let mut operation = source.clone();
            let get = &mut operation["get"];
            get["operationId"] = json!(format!("path:{}", path));
            get["description"] = json!(if route.method == "profile" && region == Some("jp") {
                "JP requires an explicit JP route and a positive int64 profile ID; no international prefix inference."
            } else if route.method == "profile" {
                "11-digit profile ID: 2 = tw, 3 = en, 4 = kr. Selects the matching configured region; explicit region must agree. Unknown prefixes return 400 and missing regions return 503. No cross-region fallback."
            } else {
                "Readable GET alias. Optional filters remain query parameters. Lists in a path use commas, preserving order and duplicates. Path-bound fields cannot also be query parameters. An explicit region selects its independent session; otherwise the default region is used."
            });
            for parameter in get["parameters"].as_array_mut().unwrap() {
                if let Some((path_name, _, list)) = route
                    .fields
                    .iter()
                    .find(|(_, field, _)| parameter["name"] == *field)
                {
                    parameter["name"] = json!(path_name);
                    parameter["in"] = json!("path");
                    parameter["required"] = json!(true);
                    parameter["style"] = json!("simple");
                    parameter["explode"] = json!(false);
                    parameter["description"] = json!(if *list {
                        "Comma-separated items; order and duplicates are preserved."
                    } else {
                        "URL-encoded path segment."
                    });
                    if route.method == "profile" {
                        parameter["schema"] = if region == Some("jp") {
                            json!({"type":"string","pattern":"^[0-9]+$"})
                        } else {
                            json!({"type":"string","pattern":"^[234][0-9]{10}$"})
                        };
                    }
                }
            }
            get["responses"]["200"]["headers"]["X-Moenotes-Region"] = json!({"description":"Selected region for automatic-profile or explicit-region requests.","schema":{"type":"string","enum":["tw","en","kr","jp"]}});
            paths.insert(path, operation);
        }
    }
    for &(path, _) in crate::ROUTES {
        let Some(source) = existing.get(path) else {
            continue;
        };
        for region in ["tw", "en", "kr", "jp"] {
            let path = format!("/v1/{region}{}", path.trim_start_matches("/v1"));
            let mut operation = source.clone();
            operation["get"]["operationId"] = json!(format!("region:{}", path));
            operation["get"]["description"] = json!(
                "Existing query contract against an explicit configured region. Missing regions return 503; no fallback or cross-region credential reuse."
            );
            paths.insert(path, operation);
        }
    }
    paths.insert(crate::profile_images::ROUTE.into(), json!({"get": {
        "operationId":"jp-profile-card-image", "tags":["read queries"],
        "description":"JP custom profile-card page as PNG. Page is 1-based in thumbnailUrl order. No request body or query parameters. Server-side CDN authentication; existing profile JSON is unchanged.",
        "security":[{"apiKey":[]}],
        "parameters":[
            {"name":"profileId","in":"path","required":true,"schema":{"type":"string","pattern":"^[0-9]+$"},"description":"Positive int64 JP profile ID."},
            {"name":"page","in":"path","required":true,"schema":{"type":"integer","minimum":1},"description":"1-based page index."}
        ],
        "responses":{
            "200":{"description":"PNG, at most 8 MiB. Cache-Control: no-store; internal image cache keyed by the full upstream URL.","content":{"image/png":{"schema":{"type":"string","format":"binary"}}}},
            "400":{"description":"Invalid ID, page, query or body"},"401":{"description":"Missing or invalid API key"},
            "404":{"description":"Profile/card/page missing or route disabled"},"405":{"description":"GET only"},
            "429":{"description":"Local admission/download limit"},"502":{"description":"Invalid upstream image or CDN failure"},
            "503":{"description":"JP/proxy unconfigured or upstream session unavailable"},"504":{"description":"Request deadline exceeded"}
        }
    }}));
    json!({"openapi":"3.1.0","info":{"title":"moenotes-api","version":env!("CARGO_PKG_VERSION"),"description":"Experimental GET query gateway with limited live validation. Not an official or stable API."},
        "paths":paths,"components":{"securitySchemes":{"apiKey":{"type":"http","scheme":"bearer"}},"schemas":schemas}})
}

fn schema(message: MessageDescriptor, schemas: &mut BTreeMap<String, Value>) {
    let name = message.full_name().to_owned();
    if schemas.contains_key(&name) {
        return;
    }
    schemas.insert(name.clone(), json!({}));
    let mut properties = BTreeMap::new();
    for field in message.fields() {
        let value = if field.is_map() {
            let Kind::Message(entry) = field.kind() else {
                unreachable!()
            };
            json!({"type":"object","additionalProperties":kind(entry.get_field_by_name("value").unwrap().kind(),schemas)})
        } else if field.is_list() {
            json!({"type":"array","items":kind(field.kind(),schemas)})
        } else {
            kind(field.kind(), schemas)
        };
        properties.insert(field.json_name().to_owned(), value);
    }
    schemas.insert(
        name,
        json!({"type":"object","properties":properties,"additionalProperties":false}),
    );
}
fn kind(kind: Kind, schemas: &mut BTreeMap<String, Value>) -> Value {
    match kind {
        Kind::Message(message) => {
            let name = message.full_name().to_owned();
            schema(message, schemas);
            json!({"$ref":format!("#/components/schemas/{name}")})
        }
        Kind::Enum(enumeration) => {
            json!({"oneOf":[{"type":"string","enum":enumeration.values().map(|v|v.name().to_owned()).collect::<Vec<_>>()},{"type":"integer","format":"int32"}]})
        }
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => {
            json!({"type":"string","pattern":"^-?[0-9]+$"})
        }
        Kind::Uint64 | Kind::Fixed64 => json!({"type":"string","pattern":"^[0-9]+$"}),
        Kind::Bool => json!({"type":"boolean"}),
        Kind::String => json!({"type":"string"}),
        Kind::Bytes => json!({"type":"string","contentEncoding":"base64"}),
        Kind::Float | Kind::Double => {
            json!({"oneOf":[{"type":"number"},{"type":"string","enum":["NaN","Infinity","-Infinity"]}]})
        }
        _ => json!({"type":"integer"}),
    }
}
