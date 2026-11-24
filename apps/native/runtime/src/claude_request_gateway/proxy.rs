use hyper::{Request, Uri, client::conn::http1::SendRequest};

use super::{Duration, Fault, GatewayBody, ProxyEnvironment, io};
use crate::http_proxy::{self, AbortOnDrop};

type Exchange = (SendRequest<GatewayBody>, AbortOnDrop);

pub(super) async fn connect_proxy(
    environment: &ProxyEnvironment,
    destination: &Uri,
    request: &mut Request<GatewayBody>,
) -> Result<Option<Exchange>, Fault> {
    let Some(proxy) = environment.resolve(destination, || None).map_err(|_| {
        Fault::bad_gateway(
            "Claude request gateway proxy configuration is unavailable or unsupported.",
        )
    })?
    else {
        return Ok(None);
    };
    let exchange = tokio::time::timeout(
        Duration::from_secs(30),
        http_proxy::connect_proxy(&proxy, destination, request),
    )
    .await
    .map_err(|_| Fault::connection(io::ErrorKind::TimedOut))?
    .map_err(|error| Fault::transport(error.as_ref()))?;
    Ok(Some(exchange))
}
