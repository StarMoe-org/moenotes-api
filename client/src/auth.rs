//! Explicit Android OneSDK callback import and game-login exchange.
//!
//! This does not obtain SDK tokens, renew them, or implement password/social login.
//! Login can create an account or affect another device's session. Invoke it only
//! with the account owner's authorization; cancellation cannot undo a sent RPC.
#![doc = include_str!("../../docs/sdk-login.md")]
use crate::{
    CancellationToken, Client, ClientError, Credentials, ErrorKind, Generation, Method, Session,
    SessionConfig, session::origin,
};
use moenotes_proto::{generated::app::playerlogin, pool};
use prost::Message;
use prost_reflect::DynamicMessage;
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    sync::{Arc, RwLock},
};
use tonic::metadata::MetadataMap;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

pub const LOGIN: Method = Method {
    name: "player-login",
    path: "/app.playerlogin.PlayerLoginService/PlayerLogin",
    input: "app.playerlogin.PlayerLoginRequest",
    output: "app.playerlogin.PlayerLoginResponse",
    anonymous: true,
    http: false,
};
pub const PRE_LOGIN: Method = Method {
    name: "player-pre-login",
    path: "/app.playerlogin.PlayerLoginService/PlayerPreLogin",
    input: LOGIN.input,
    output: "app.playerlogin.PlayerPreLoginResponse",
    anonymous: true,
    http: false,
};

pub(crate) fn is_login_method(method: Method) -> bool {
    method == LOGIN || method == PRE_LOGIN
}

#[derive(Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "camelCase")]
struct Callback {
    uid: String,
    access_token: String,
    #[serde(default)]
    id_token: Option<String>,
}

/// Only the three fields consumed by native PlayerLogin are retained.
/// Debug is redacted; there is deliberately no Serialize implementation.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SdkAuthorization {
    region: String,
    origin: String,
    callback: Callback,
}

impl fmt::Debug for SdkAuthorization {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SdkAuthorization([REDACTED])")
    }
}

impl SdkAuthorization {
    /// Explicit, origin-bound SDK cache import. Does not validate token lifetime.
    pub fn from_file(config: &SessionConfig, path: &std::path::Path) -> Result<Self, ClientError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Stored {
            region: String,
            origin: String,
            callback: Callback,
        }
        let bytes = crate::secret_file::read(path)?;
        let stored: Stored = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        let sdk = Self {
            region: stored.region,
            origin: origin(&stored.origin)?,
            callback: stored.callback,
        };
        sdk.check_binding(config)?;
        let callback = sdk.callback_bytes()?;
        Self::from_callback_json(config, &callback)
    }

    /// Save to a new private Unix file, never automatically and never overwrite.
    pub fn save_to_file(&self, path: &std::path::Path) -> Result<(), ClientError> {
        let callback = self.callback_bytes()?;
        // Serialize through borrowed fields below; no password is part of this cache.
        #[derive(Serialize)]
        struct RawStored<'a> {
            region: &'a str,
            origin: &'a str,
            callback: &'a serde_json::value::RawValue,
        }
        let raw: &serde_json::value::RawValue =
            serde_json::from_slice(&callback).map_err(|_| invalid())?;
        let bytes = Zeroizing::new(
            serde_json::to_vec(&RawStored {
                region: &self.region,
                origin: &self.origin,
                callback: raw,
            })
            .map_err(|_| invalid())?,
        );
        crate::secret_file::create(path, &bytes)
    }

    fn callback_bytes(&self) -> Result<Zeroizing<Vec<u8>>, ClientError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Export<'a> {
            uid: &'a str,
            access_token: &'a str,
            id_token: &'a Option<String>,
        }
        serde_json::to_vec(&Export {
            uid: &self.callback.uid,
            access_token: &self.callback.access_token,
            id_token: &self.callback.id_token,
        })
        .map(Zeroizing::new)
        .map_err(|_| invalid())
    }

    /// Import an authorized OneSDK OnUserLoginSuccess JSON callback. No I/O.
    /// Unknown SDK fields are ignored. The caller owns and must protect `json`.
    /// Binding to config is a local disclosure boundary, not SDK token validation.
    pub fn from_callback_json(config: &SessionConfig, json: &[u8]) -> Result<Self, ClientError> {
        config.validate()?;
        if config.platform != "android" || json.len() > 64 * 1024 {
            return Err(invalid());
        }
        let callback: Callback = serde_json::from_slice(json).map_err(|_| invalid())?;
        if callback.uid.is_empty()
            || callback.uid.len() > 256
            || callback.access_token.is_empty()
            || callback.access_token.len() > 16 * 1024
            || callback
                .id_token
                .as_ref()
                .is_some_and(|v| v.len() > 16 * 1024)
        {
            return Err(invalid());
        }
        // BID will be used as an HTTP/2 ASCII metadata value.
        tonic::metadata::MetadataValue::try_from(callback.uid.as_str()).map_err(|_| invalid())?;
        Ok(Self {
            region: config.region.clone(),
            origin: origin(&config.origin)?,
            callback,
        })
    }

    fn check_binding(&self, config: &SessionConfig) -> Result<(), ClientError> {
        if config.platform != "android"
            || self.region != config.region
            || self.origin != origin(&config.origin)?
        {
            return Err(ClientError::new(ErrorKind::InvalidConfig));
        }
        Ok(())
    }
}

/// Explicit operator inputs. Identifiers are not generated or read from a device.
/// Numeric channel IDs come from SDKChannelInfo, not User.channelId or region names.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct AndroidLoginContext {
    pub device_model: String,
    pub operating_system: String,
    pub device_identifier: String,
    pub global_channel_id: u32,
    pub brand_id: u32,
    pub area_id: u32,
}

impl fmt::Debug for AndroidLoginContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AndroidLoginContext([REDACTED])")
    }
}

/// No credential or SDK token is returned to the caller or HTTP query surface.
#[derive(Clone, Copy, Debug)]
pub struct LoginReceipt {
    pub generation: Generation,
    /// Raw protobuf uint32, not a locally inferred boolean.
    pub is_new_user: u32,
}

fn invalid() -> ClientError {
    ClientError::new(ErrorKind::InvalidRequest)
}
fn protocol() -> ClientError {
    ClientError::new(ErrorKind::Protocol)
}

fn build_request(
    config: &SessionConfig,
    sdk: &SdkAuthorization,
    context: &AndroidLoginContext,
    method: Method,
) -> Result<(DynamicMessage, MetadataMap), ClientError> {
    sdk.check_binding(config)?;
    if [
        &context.device_model,
        &context.operating_system,
        &context.device_identifier,
    ]
    .iter()
    .any(|v| v.is_empty() || v.len() > 1024)
    {
        return Err(invalid());
    }
    // Native 0x59e448c, PlatformManager 0x693fd0c/1c/5c: platform=0,
    // fixed Android package, empty adId and no initial_data_group assignment.
    let mut request = playerlogin::PlayerLoginRequest {
        sdk_uid: sdk.callback.uid.clone(),
        sdk_access_token: sdk.callback.access_token.clone(),
        // The native server probe omits idToken, unlike the final login builder.
        id_token: if method == LOGIN {
            sdk.callback.id_token.clone().unwrap_or_default()
        } else {
            String::new()
        },
        platform: 0,
        device_model: context.device_model.clone(),
        operating_system: context.operating_system.clone(),
        client_version: config.client_version.clone(),
        uuid: Some(playerlogin::PlayerUuid {
            ad_id: String::new(),
            identifier: context.device_identifier.clone(),
        }),
        client_package: "com.bilibili.sirius".into(),
        initial_data_group: String::new(),
        global_channel_id: context.global_channel_id,
        brand_id: context.brand_id,
        area_id: context.area_id,
    };
    let bytes = Zeroizing::new(request.encode_to_vec());
    request.sdk_uid.zeroize();
    request.sdk_access_token.zeroize();
    request.id_token.zeroize();
    request.uuid.as_mut().unwrap().identifier.zeroize();
    let dynamic = DynamicMessage::decode(
        pool().get_message_by_name(LOGIN.input).unwrap(),
        bytes.as_slice(),
    )
    .map_err(|_| protocol())?;
    let mut metadata = config.metadata(None, &Generation::new_v4().to_string(), true)?;
    if method == PRE_LOGIN {
        // ProbeServer uses a direct client, not the normal authenticated invoker.
        metadata.remove("x-client-version");
    }
    let mut bid = tonic::metadata::MetadataValue::try_from(sdk.callback.uid.as_str())
        .map_err(|_| invalid())?;
    bid.set_sensitive(true);
    // Native Login sets BID from SDK uid before enqueuing the login RPC.
    metadata.insert("x-player-bid", bid);
    Ok((dynamic, metadata))
}

impl Client {
    /// One explicit pre-login RPC, without installing credentials or auto-login.
    /// A true result does not verify an existing game session or guarantee login.
    pub async fn pre_login_with_sdk(
        &self,
        generation: Generation,
        sdk: &SdkAuthorization,
        context: &AndroidLoginContext,
        cancel: CancellationToken,
    ) -> Result<bool, ClientError> {
        let session = self.session_for(generation)?;
        let (request, metadata) = build_request(&session.config, sdk, context, PRE_LOGIN)?;
        self.run_operation(session, PRE_LOGIN, request, metadata, cancel, |message| {
            let response =
                playerlogin::PlayerPreLoginResponse::decode(message.encode_to_vec().as_slice())
                    .map_err(|_| protocol())?;
            Ok(response.is_account_created)
        })
        .await
    }

    /// One explicit login exchange, then atomic installation into a new generation.
    /// No queue polling, refresh, pre-login, account registration workflow or disk save.
    /// The RPC itself may create a game account. No force-device header is sent.
    pub async fn login_with_sdk(
        &self,
        generation: Generation,
        sdk: &SdkAuthorization,
        context: &AndroidLoginContext,
        cancel: CancellationToken,
    ) -> Result<LoginReceipt, ClientError> {
        let session = self.session_for(generation)?;
        let (request, metadata) = build_request(&session.config, sdk, context, LOGIN)?;
        self.run_operation(
            session.clone(),
            LOGIN,
            request,
            metadata,
            cancel.clone(),
            |message| {
                let bytes = Zeroizing::new(message.encode_to_vec());
                let mut response = playerlogin::PlayerLoginResponse::decode(bytes.as_slice())
                    .map_err(|_| protocol())?;
                let mut credential = response.credential.take().ok_or_else(protocol)?;
                let credentials = Credentials {
                    player_id: std::mem::take(&mut credential.id),
                    credential: std::mem::take(&mut credential.credential),
                    // Native success uses SetupCertification(id, credential, null).
                    device_id: None,
                    bid: Some(sdk.callback.uid.clone()),
                };
                credential.device_id.zeroize();
                session
                    .config
                    .metadata(Some(&credentials), "validation", false)
                    .map_err(|_| protocol())?;
                let new = Arc::new(Session {
                    generation: Generation::new_v4(),
                    config: session.config.clone(),
                    credentials: Some(credentials),
                    transport: session.transport.clone(),
                    cancel: CancellationToken::new(),
                    blocked: RwLock::new(None),
                    master_mismatch: std::sync::atomic::AtomicBool::new(false),
                });
                let mut current = self.session.write().unwrap();
                if current.generation != generation {
                    return Err(ClientError::new(ErrorKind::SessionChanged));
                }
                if cancel.is_cancelled() {
                    return Err(ClientError::new(ErrorKind::Cancelled));
                }
                let receipt = LoginReceipt {
                    generation: new.generation,
                    is_new_user: response.is_new_user,
                };
                current.cancel.cancel();
                *current = new;
                Ok(receipt)
            },
        )
        .await
    }
}
