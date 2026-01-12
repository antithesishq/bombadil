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
use http::{uri::PathAndQuery, HeaderValue, Method};
use http_body_util::BodyExt;
use hyper::server::conn::http1;
use hyper::{body::Incoming, upgrade::Upgraded};
use hyper_util::rt::TokioIo;
use oxc::span::SourceType;
use std::{convert::Infallible, net::SocketAddr, str::from_utf8};
use tokio::{
    net::{TcpListener, TcpStream},
    spawn,
};
use tower::Service;

use crate::instrumentation;

pub async fn start_proxy(port: u16) -> Result<()> {
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
    log::debug!("listening on {}", addr);

    let listener = TcpListener::bind(addr).await.unwrap();
    loop {
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
    let host = parts
        .headers
        .get("host")
        .ok_or(anyhow!("no `host` header in request"))?
        .to_str()?;

    // // Fake for now:
    // let host = "localhost:8000";
    //

    let stream = TcpStream::connect(host).await?;
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
        log::info!("not instrumenting third-party scripts: {}", parts.uri);
        response_body
    } else if let Some(content_type) =
        response_parts.headers.get("content-type")
        && content_type.to_str()?.starts_with("text/javascript")
    {
        let bytes = response_body.collect().await?.to_bytes();
        match instrumentation::instrument_source_code(
            from_utf8(&bytes)?,
            SourceType::cjs(),
        ) {
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

    // log::info!("we have javascript");

    Ok(Response::from_parts(response_parts, body))
}
