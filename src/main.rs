mod context;
mod metrics;
mod xml;

use std::{error::Error, net::SocketAddr, sync::Arc};

use clap::Parser;
use prometheus::{core::Metric, labels, proto::LabelPair, Encoder, TextEncoder};
use reqwest::Url;
use tokio::sync::Mutex;
use tracing::{debug, error, info};
use warp::{
    http::HeaderValue,
    hyper::{header::CONTENT_TYPE, Body, StatusCode},
    Filter, Reply,
};

use crate::{context::Context, metrics::collect_metrics};

/// Prometheus data exporter for NGINX servers running the nginx-rtmp-module.
#[derive(Parser)]
struct Args {
    /// The RTMP statistics endpoint of NGINX.
    #[clap(long)]
    pub scrape_url: Url,
}

fn encode_metrics() -> Result<(TextEncoder, String), Box<dyn Error>> {
    let encoder = TextEncoder::new();
    let mut buf = String::new();
    // gather and encode metrics
    let metric_families = prometheus::gather();
    encoder.encode_utf8(&metric_families, &mut buf)?;
    // return encoder and buffer
    Ok((encoder, buf))
}

#[tokio::main]
async fn main() {
    let args: Args = Args::parse();
    // intialize tracing
    tracing_subscriber::fmt::init();
    // print splash
    info!("{} v{}", env!("CARGO_PKG_NAME"), env!("VERGEN_GIT_SEMVER"));
    // print version information
    debug!(
        build_timestamp = env!("VERGEN_BUILD_TIMESTAMP"),
        rustc_version = env!("VERGEN_RUSTC_SEMVER"),
        builder_host = env!("VERGEN_RUSTC_HOST_TRIPLE")
    );
    // create threadsafe context
    let ctx = Context::new(args.scrape_url);
    let ctx = Arc::new(Mutex::new(ctx));
    // create context filter
    let ctx = warp::any().map(move || ctx.clone());
    // create index filter
    let index = warp::get()
        .and(warp::path::end())
        .and(ctx)
        .then(|ctx: Arc<Mutex<Context>>| async move {
            let mut ctx = ctx.lock().await;
            collect_metrics(&mut ctx).await?;
            encode_metrics()
        })
        .map(|res: Result<(TextEncoder, String), Box<dyn Error>>| match res {
            Ok((encoder, buf)) => {
                let mut res = warp::reply::Response::new(Body::from(buf));
                res.headers_mut()
                    .insert(CONTENT_TYPE, HeaderValue::from_str(encoder.format_type()).unwrap());
                res
            }
            Err(err) => {
                error!("Failed to collect metrics");
                error!("{}", err);
                warp::reply::with_status(warp::reply(), StatusCode::INTERNAL_SERVER_ERROR)
                    .into_response()
            }
        })
        .with(warp::trace::request());
    // get address and listen
    let addr: SocketAddr = "127.0.0.1:3000".parse().unwrap();
    info!("Listening on {}", addr);
    warp::serve(index).try_bind(addr).await;
}
