//! This proxy instruments JavaScript on the fly, capturing coverage information. It's meant to
//! be run by the browser tests and used as a proxy for Chrome. You can, however, run it as a
//! standalone proxy for testing:
//!
//! ```not_rust
//! $ cargo run -- proxy --port=3000
//! ```
//!
//! In another terminal:
//!
//! ```not_rust
//! $ curl -v -x "127.0.0.1:3000" https://tokio.rs
//! ```

use anyhow::{anyhow, Result};
use axum::{
    body::Body,
    extract::Request,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use http::{uri::PathAndQuery, HeaderValue, Method};
use http_body_util::BodyExt;
use hyper::server::conn::http1;
use hyper::{body::Incoming, upgrade::Upgraded};
use hyper_util::rt::TokioIo;
use oxc::span::SourceType;
use std::{
    convert::Infallible,
    hash::{DefaultHasher, Hash, Hasher},
    net::SocketAddr,
    str::from_utf8,
};
use tokio::{
    net::{TcpListener, TcpStream},
    spawn,
    sync::oneshot,
};
use tower::Service;

use crate::instrumentation;

pub struct Proxy {
    pub port: u16,
    shutdown: oneshot::Sender<()>,
}

impl Proxy {
    pub async fn spawn(port: u16) -> Result<Self> {
        let (sender, receiver) = oneshot::channel();
        let port = start_proxy(port, receiver).await?;
        Ok(Proxy {
            port,
            shutdown: sender,
        })
    }

    pub fn stop(self) {
        match self.shutdown.send(()) {
            Ok(_) => {}
            Err(_) => log::error!("failed to stop proxy"),
        }
    }

    pub async fn done(&mut self) {
        self.shutdown.closed().await
    }
}

async fn start_proxy(
    port: u16,
    mut shutdown: oneshot::Receiver<()>,
) -> Result<u16> {
    let tower_service = tower::service_fn(move |req: Request<_>| async move {
        let req = req.map(Body::new);
        if req.method() == Method::CONNECT {
            log::debug!("proxying https");
            match proxy_https(req).await {
                Ok(response) => Ok::<Response<Body>, Infallible>(response),
                Err(err) => {
                    log::warn!("proxy (https) error: {:?}", err);
                    Ok((StatusCode::INTERNAL_SERVER_ERROR, "proxy error")
                        .into_response())
                }
            }
        } else {
            log::debug!("proxying with instrumentation: {:?}", req.uri());
            match proxy_with_instrumentation(req).await {
                Ok(response) => Ok::<Response<Body>, Infallible>(response),
                Err(err) => {
                    log::warn!("proxy (instrumenting) error: {:?}", err);
                    Ok((StatusCode::INTERNAL_SERVER_ERROR, "proxy error")
                        .into_response())
                }
            }
        }
    });

    let addr = SocketAddr::from(([127, 0, 0, 1], port));

    let listener = TcpListener::bind(addr).await.unwrap();
    let port_actual = listener.local_addr()?.port();

    log::info!("proxy listening on port {}", port_actual);

    spawn(async move {
        loop {
            if let Ok(_) = shutdown.try_recv() {
                log::info!("shutting down proxy");
                break;
            }
            let (stream, _) = listener.accept().await.unwrap();
            let io = TokioIo::new(stream);
            tokio::task::spawn(async move {
                if let Err(err) = http1::Builder::new()
                    .preserve_header_case(true)
                    .title_case_headers(true)
                    .serve_connection(io, {
                        hyper::service::service_fn(
                            move |request: Request<Incoming>| {
                                tower_service.clone().call(request)
                            },
                        )
                    })
                    .with_upgrades()
                    .await
                {
                    println!("Failed to serve connection: {err:?}");
                }
            });
        }
    });

    Ok(port_actual)
}

async fn proxy_https(req: Request) -> Result<Response, hyper::Error> {
    log::trace!("{:?}", req);

    if let Some(host_addr) = req.uri().authority().map(|auth| auth.to_string())
    {
        tokio::task::spawn(async move {
            match hyper::upgrade::on(req).await {
                Ok(upgraded) => {
                    if let Err(e) = tunnel(upgraded, host_addr).await {
                        log::warn!("server io error: {}", e);
                    };
                }
                Err(e) => log::warn!("upgrade error: {}", e),
            }
        });

        Ok(Response::new(Body::empty()))
    } else {
        log::warn!("CONNECT host is not socket addr: {:?}", req.uri());
        Ok((
            StatusCode::BAD_REQUEST,
            "CONNECT must be to a socket address",
        )
            .into_response())
    }
}

async fn tunnel(upgraded: Upgraded, addr: String) -> std::io::Result<()> {
    let mut server = TcpStream::connect(addr).await?;
    let mut upgraded = TokioIo::new(upgraded);

    let (from_client, from_server) =
        tokio::io::copy_bidirectional(&mut upgraded, &mut server).await?;

    log::debug!(
        "client wrote {} bytes and received {} bytes",
        from_client,
        from_server
    );

    Ok(())
}

async fn proxy_with_instrumentation(
    req: Request<Body>,
) -> Result<Response<Body>> {
    let (parts, body) = req.into_parts();

    let host = parts.uri.host().ok_or(anyhow!("no host in request uri"))?;
    let port = parts.uri.port().map(|port| port.as_u16()).unwrap_or(80);

    let stream = match TcpStream::connect(format!("{}:{}", host, port)).await {
        Ok(stream) => stream,
        Err(err) => {
            log::debug!("couldn't connect to {}: {}", host, err);
            return Ok((
                StatusCode::SERVICE_UNAVAILABLE,
                "upstream server connection error",
            )
                .into_response());
        }
    };
    let io = TokioIo::new(stream);
    let (mut sender, conn) =
        hyper::client::conn::http1::handshake::<_, Body>(io).await?;

    let mut target_request_builder = Request::builder()
        .method(parts.method)
        .version(parts.version)
        .uri(
            parts
                .uri
                .path_and_query()
                .unwrap_or(&PathAndQuery::from_static("/"))
                .as_str(),
        )
        .extension(parts.extensions);

    for (name, value) in parts.headers {
        if let Some(name) = name {
            if name != "proxy-connection" {
                target_request_builder =
                    target_request_builder.header(&name, &value);
            }
        }
    }

    let target_request = target_request_builder.body(body)?;

    spawn(async move {
        if let Err(err) = conn.await {
            log::error!("connection failed: {:?}", err);
        }
    });

    log::debug!("requesting from target: {:?}", &target_request);

    let (mut response_parts, response_body) = sender
        .send_request(target_request)
        .await?
        .map(Body::new)
        .into_parts();
    log::debug!("response from target: {:?}", &response_parts);

    let body = if parts.uri.path().contains("node_modules") {
        log::info!("not instrumenting third-party files: {}", parts.uri);
        response_body
    } else if let Some(content_type) =
        response_parts.headers.get("content-type")
        && content_type.to_str()?.starts_with("text/javascript")
    {
        let bytes = response_body.collect().await?.to_bytes();

        let etag = response_parts
            .headers
            .get("etag")
            .and_then(|value| value.to_str().ok());
        let source_id = source_id(etag, &bytes);
        match instrument_javascript(source_id, &bytes) {
            Ok(code) => {
                let headers = response_parts
                    .headers
                    .get_mut("content-length")
                    .ok_or(anyhow!("no content-length"))?;
                *headers = HeaderValue::from_str(&format!("{}", code.len()))?;
                Body::from(code)
            }
            Err(_) => Body::from(bytes),
        }
    } else {
        response_body
    };

    Ok(Response::from_parts(response_parts, body))
}

/// Calculate source ID from etag or body.
fn source_id(etag: Option<&str>, body: &Bytes) -> instrumentation::SourceId {
    let mut hasher = DefaultHasher::new();
    match etag {
        Some(etag) => etag.hash(&mut hasher),
        None => body.hash(&mut hasher),
    };
    instrumentation::SourceId(hasher.finish())
}

fn instrument_javascript(
    source_id: instrumentation::SourceId,
    body: &Bytes,
) -> Result<Bytes> {
    let code = instrumentation::instrument_source_code(
        source_id,
        from_utf8(&body)?,
        SourceType::cjs(),
    )?;
    Ok(Bytes::from(code))
}
