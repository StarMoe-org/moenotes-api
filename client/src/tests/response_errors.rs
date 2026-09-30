use super::*;

async fn wire_error(
    payload: Option<&'static [u8]>,
    code: tonic::Code,
    business: Option<&'static str>,
) -> ClientError {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let stop = CancellationToken::new();
    let shutdown = stop.clone();
    let router = Router::new().fallback(move || async move {
        let mut frames = Vec::new();
        if let Some(payload) = payload {
            let mut data = vec![0];
            data.extend_from_slice(&(payload.len() as u32).to_be_bytes());
            data.extend_from_slice(payload);
            frames.push(Ok::<_, std::convert::Infallible>(http_body::Frame::data(
                bytes::Bytes::from(data),
            )));
        }
        let mut trailers = http::HeaderMap::new();
        trailers.insert("grpc-status", (code as i32).to_string().parse().unwrap());
        // A remote status can use the decoder's text: classification must not trust it.
        trailers.insert(
            "grpc-message",
            "protobuf%20decode%20failed".parse().unwrap(),
        );
        if let Some(business) = business {
            trailers.insert("x-sirius-error-code", business.parse().unwrap());
        }
        frames.push(Ok(http_body::Frame::trailers(trailers)));
        let mut response = http::Response::builder().header("content-type", "application/grpc");
        if payload.is_some()
            && let Some(business) = business
        {
            response = response.header("x-sirius-error-code", business);
        }
        response
            .body(Body::new(StreamBody::new(futures_util::stream::iter(
                frames,
            ))))
            .unwrap()
    });
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
            .unwrap()
    });
    let channel = tonic::transport::Endpoint::from_shared(format!("http://{address}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let client =
        Client::with_transport(config(), None, options(), Arc::new(mock_channel(channel))).unwrap();
    let result = client
        .query(Query::Version(Default::default()))
        .await
        .err()
        .unwrap();
    stop.cancel();
    server.await.unwrap();
    result
}

#[tokio::test]
async fn malformed_protobuf_responses_are_protocol_errors() {
    let error = wire_error(Some(b"\xff"), tonic::Code::Ok, None).await;
    assert_eq!(error.kind, ErrorKind::Protocol);
    assert_eq!(error.grpc_code, Some(tonic::Code::Internal));
    assert!(!format!("{error:?}").contains("decode failed"));
}

#[tokio::test]
async fn remote_internal_errors_keep_transport_and_business_classification() {
    let error = wire_error(None, tonic::Code::Internal, None).await;
    assert_eq!(error.kind, ErrorKind::Transport);
    assert_eq!(error.grpc_code, Some(tonic::Code::Internal));
    let error = wire_error(None, tonic::Code::Internal, Some("UNDER_MAINTENANCE")).await;
    assert_eq!(error.kind, ErrorKind::Maintenance);
    assert_eq!(error.grpc_code, Some(tonic::Code::Internal));
}

#[tokio::test]
async fn initial_business_codes_keep_priority_over_local_decode_errors() {
    for (code, kind) in [
        ("UNDER_MAINTENANCE", ErrorKind::Maintenance),
        ("UNKNOWN_SYNTHETIC_CODE", ErrorKind::Business),
    ] {
        let error = wire_error(Some(b"\xff"), tonic::Code::Ok, Some(code)).await;
        assert_eq!(error.kind, kind);
        assert_eq!(error.grpc_code, Some(tonic::Code::Internal));
        assert!(!format!("{error:?}").contains(code));
    }
}
