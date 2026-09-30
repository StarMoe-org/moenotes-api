use async_trait::async_trait;
use bytes::Buf;

use prost::Message;
use prost_reflect::{DynamicMessage, MessageDescriptor};
use std::{sync::Arc, time::Duration};
use tonic::{
    Code, Request, Status,
    codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder},
    metadata::MetadataMap,
    transport::{Channel, ClientTlsConfig, Endpoint},
};

use crate::{ClientError, ErrorKind, Method, SessionConfig};

/// Injectable for offline tests; production construction uses validated HTTPS origins.
#[async_trait]
pub trait Transport: Send + Sync {
    async fn call_with_metadata(
        &self,
        method: Method,
        request: DynamicMessage,
        metadata: MetadataMap,
        timeout: Duration,
    ) -> Result<TransportResponse, ClientError> {
        self.call(method, request, metadata, timeout)
            .await
            .map(|message| TransportResponse {
                message,
                asset_version: None,
            })
    }
    async fn call(
        &self,
        method: Method,
        request: DynamicMessage,
        metadata: MetadataMap,
        timeout: Duration,
    ) -> Result<DynamicMessage, ClientError>;
}

pub struct GrpcTransport {
    channel: Channel,
    region: String,
}

pub struct TransportResponse {
    pub message: DynamicMessage,
    /// Only public version metadata is retained. CDN passwords never leave transport.
    pub asset_version: Option<String>,
}

impl GrpcTransport {
    pub fn new(config: &SessionConfig) -> Result<Self, ClientError> {
        config.validate()?;
        let endpoint = Endpoint::from_shared(config.origin.clone())
            .map_err(|_| ClientError::new(ErrorKind::InvalidConfig))?
            .tls_config(ClientTlsConfig::new().with_webpki_roots())
            .map_err(|_| ClientError::new(ErrorKind::InvalidConfig))?
            .connect_timeout(Duration::from_secs(10));
        Ok(Self {
            channel: endpoint.connect_lazy(),
            region: config.region.clone(),
        })
    }
}

#[async_trait]
impl Transport for GrpcTransport {
    async fn call(
        &self,
        method: Method,
        message: DynamicMessage,
        metadata: MetadataMap,
        timeout: Duration,
    ) -> Result<DynamicMessage, ClientError> {
        self.call_with_metadata(method, message, metadata, timeout)
            .await
            .map(|r| r.message)
    }

    async fn call_with_metadata(
        &self,
        method: Method,
        message: DynamicMessage,
        metadata: MetadataMap,
        timeout: Duration,
    ) -> Result<TransportResponse, ClientError> {
        let mut grpc = tonic::client::Grpc::new(self.channel.clone())
            .max_decoding_message_size(8 * 1024 * 1024)
            .max_encoding_message_size(1024 * 1024);
        grpc.ready()
            .await
            .map_err(|_| ClientError::new(ErrorKind::Transport))?;
        let mut request = Request::new(message);
        *request.metadata_mut() = metadata;
        request.set_timeout(timeout);
        let codec = DynamicCodec(
            moenotes_proto::pool_for_region(&self.region)
                .get_message_by_name(method.output)
                .ok_or_else(|| ClientError::new(ErrorKind::Protocol))?,
        );
        // Unary wire format, with explicit stream consumption to retain initial and
        // trailing metadata separately even when the final gRPC status is an error.
        let response = grpc
            .server_streaming(
                request,
                http::uri::PathAndQuery::from_static(method.path),
                codec,
            )
            .await
            .map_err(|status| status_error(status, &MetadataMap::new()))?;
        let initial = response.metadata().clone();
        let mut stream = response.into_inner();
        let message = stream
            .message()
            .await
            .map_err(|s| status_error(s, &initial))?;
        if stream
            .message()
            .await
            .map_err(|s| status_error(s, &initial))?
            .is_some()
        {
            return Err(ClientError::new(ErrorKind::Protocol));
        }
        let trailing = stream
            .trailers()
            .await
            .map_err(|s| status_error(s, &initial))?
            .unwrap_or_default();
        if let Some(error) = ClientError::from_metadata(Code::Ok, &initial, &trailing) {
            return Err(error);
        }
        let asset_version = trailing
            .get_all("x-asset-version")
            .iter()
            .next_back()
            .or_else(|| initial.get_all("x-asset-version").iter().next_back())
            .and_then(|v| v.to_str().ok())
            .filter(|v| v.len() <= 64 * 1024)
            .map(str::to_owned);
        Ok(TransportResponse {
            message: message.ok_or_else(|| ClientError::new(ErrorKind::Protocol))?,
            asset_version,
        })
    }
}

fn status_error(status: Status, initial: &MetadataMap) -> ClientError {
    ClientError::from_status(&status, initial)
}

#[derive(Clone)]
pub struct DynamicCodec(pub MessageDescriptor);
pub struct DynamicEncoder;
pub struct DynamicDecoder(MessageDescriptor);
impl Codec for DynamicCodec {
    type Encode = DynamicMessage;
    type Decode = DynamicMessage;
    type Encoder = DynamicEncoder;
    type Decoder = DynamicDecoder;
    fn encoder(&mut self) -> Self::Encoder {
        DynamicEncoder
    }
    fn decoder(&mut self) -> Self::Decoder {
        DynamicDecoder(self.0.clone())
    }
}
impl Encoder for DynamicEncoder {
    type Item = DynamicMessage;
    type Error = Status;
    fn encode(&mut self, item: Self::Item, dst: &mut EncodeBuf<'_>) -> Result<(), Status> {
        item.encode(dst)
            .map_err(|_| Status::internal("protobuf encode failed"))
    }
}
impl Decoder for DynamicDecoder {
    type Item = DynamicMessage;
    type Error = Status;
    fn decode(&mut self, src: &mut DecodeBuf<'_>) -> Result<Option<Self::Item>, Status> {
        let bytes = src.copy_to_bytes(src.remaining());
        DynamicMessage::decode(self.0.clone(), bytes)
            .map(Some)
            .map_err(|error| {
                let mut status = Status::internal("protobuf decode failed");
                status.set_source(Arc::new(error));
                status
            })
    }
}

#[cfg(test)]
pub(crate) fn mock_channel(channel: Channel) -> GrpcTransport {
    GrpcTransport {
        channel,
        region: String::new(),
    }
}
