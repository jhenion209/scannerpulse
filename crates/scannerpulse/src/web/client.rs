//! The visitor's address. `X-Forwarded-For` is believed only from the
//! addresses in the `web.trusted_proxies` setting; anyone else could write it.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::HeaderMap;
use axum::http::request::Parts;
use serde_json::Value;

use super::{ApiError, AppState};

/// The setting that lists the proxies whose `X-Forwarded-For` is believed.
pub const TRUSTED_PROXIES: &str = "web.trusted_proxies";

/// The address of the visitor who sent the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Client(pub IpAddr);

impl FromRequestParts<AppState> for Client {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Client, ApiError> {
        let Peer(peer) = Peer::from_request_parts(parts, state).await?;
        let trusted = trusted_proxies(&state.pool).await?;
        Ok(Client(address(peer, &parts.headers, &trusted)))
    }
}

/// The address the connection came from, whoever it is. Without one (a
/// test that sets none), the request counts as coming from nowhere in
/// particular.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Peer(pub IpAddr);

impl<S: Send + Sync> FromRequestParts<S> for Peer {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Peer, ApiError> {
        Ok(Peer(
            ConnectInfo::<SocketAddr>::from_request_parts(parts, state)
                .await
                .map(|info| info.0.ip())
                .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
        ))
    }
}

/// The proxies of the setting; an entry that is not an address is ignored.
pub async fn trusted_proxies(db: impl sqlx::PgExecutor<'_>) -> sqlx::Result<Vec<IpAddr>> {
    let value: Option<Value> = sqlx::query_scalar("SELECT value FROM settings WHERE key = $1")
        .bind(TRUSTED_PROXIES)
        .fetch_optional(db)
        .await?;
    Ok(value
        .as_ref()
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|v| v.as_str()?.trim().parse().ok())
                .collect()
        })
        .unwrap_or_default())
}

/// The connection's address, or, when it is a trusted proxy, the last
/// address in `X-Forwarded-For` that is not a trusted proxy: each proxy
/// appends the address it heard from, so the entries to its left may be
/// anyone's invention.
pub fn address(peer: IpAddr, headers: &HeaderMap, trusted: &[IpAddr]) -> IpAddr {
    if !trusted.contains(&peer) {
        return peer;
    }
    let forwarded: Vec<IpAddr> = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .collect::<Vec<&str>>()
        .into_iter()
        .rev()
        .map_while(|entry| entry.parse::<IpAddr>().ok())
        .collect();
    forwarded
        .into_iter()
        .find(|ip| !trusted.contains(ip))
        .unwrap_or(peer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(values: &[&str]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for value in values {
            map.append("x-forwarded-for", value.parse().unwrap());
        }
        map
    }

    const PROXY: &str = "192.0.2.10";

    #[test]
    fn a_header_from_anyone_else_is_ignored() {
        let peer: IpAddr = "198.51.100.7".parse().unwrap();
        let trusted = [PROXY.parse().unwrap()];
        assert_eq!(address(peer, &headers(&["203.0.113.9"]), &trusted), peer);
    }

    #[test]
    fn the_proxy_gives_the_last_address_it_did_not_add_itself() {
        let peer: IpAddr = PROXY.parse().unwrap();
        let trusted = [peer, "192.0.2.11".parse().unwrap()];
        let want: IpAddr = "203.0.113.9".parse().unwrap();
        assert_eq!(address(peer, &headers(&["203.0.113.9"]), &trusted), want);
        // A visitor's own invented entry sits to the left and is not believed.
        assert_eq!(
            address(peer, &headers(&["1.2.3.4, 203.0.113.9"]), &trusted),
            want
        );
        assert_eq!(
            address(
                peer,
                &headers(&["1.2.3.4", "203.0.113.9, 192.0.2.11"]),
                &trusted
            ),
            want
        );
    }

    #[test]
    fn a_missing_or_broken_header_leaves_the_proxy() {
        let peer: IpAddr = PROXY.parse().unwrap();
        let trusted = [peer];
        assert_eq!(address(peer, &headers(&[]), &trusted), peer);
        assert_eq!(address(peer, &headers(&["not an address"]), &trusted), peer);
        // Reading stops at the first entry that is not an address.
        assert_eq!(
            address(peer, &headers(&["203.0.113.9, junk"]), &trusted),
            peer
        );
    }
}
