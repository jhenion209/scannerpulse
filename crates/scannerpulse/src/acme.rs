//! Let's Encrypt, or another ACME authority: the web service gets its own
//! certificate and renews it. Each name is proved over the web (TLS-ALPN-01,
//! on the HTTPS port itself, since nothing listens on port 80) or through
//! DNS (DNS-01, a TXT record written at Cloudflare). Until the first one
//! arrives the site serves the certificate setup made. What arrives is kept
//! in `STATE`, so a restart serves it at once.

use std::collections::HashMap;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;

use crate::cloudflare::Cloudflare;

pub const LETS_ENCRYPT: &str = "https://acme-v02.api.letsencrypt.org/directory";

/// The account, the certificate and its key: in the service account's own
/// home, which the units' ProtectSystem=full leaves writable.
pub const STATE: &str = "/var/lib/scannerpulse/acme";

const ACME_TLS: &[u8] = b"acme-tls/1";

/// How often the certificate's age is looked at, and how soon a failed
/// request is made again (Let's Encrypt allows 5 failures an hour a name).
const CHECK: Duration = Duration::from_secs(12 * 3600);
const RETRY: Duration = Duration::from_secs(3600);

/// How long one request may take, from the account to the certificate: the
/// authority's client sets no limit of its own, and a request that hangs
/// would stop every renewal after it.
const ORDER_LIMIT: Duration = Duration::from_secs(600);

/// How the order and the certificate are polled: every 5 s (or when the
/// authority says to look again) for 3 minutes. A wait that doubled would
/// stop looking at about a minute, and give up at two.
const POLLS: (Duration, f32, Duration) = (Duration::from_secs(5), 1.0, Duration::from_secs(180));

/// How long Cloudflare's servers get to answer with a new TXT record before
/// the authority is told to look.
// ponytail: a fixed wait; ask Cloudflare's servers for the record first if
// the authority ever reads it too early.
const DNS_WAIT: Duration = Duration::from_secs(15);

/// How the names are proved.
#[derive(Clone, PartialEq, Eq)]
pub enum Proof {
    /// The authority reaches this machine's HTTPS port under each name.
    Web,
    /// A TXT record at Cloudflare, written with this token.
    Cloudflare { token: String },
}

impl Proof {
    pub fn name(&self) -> &'static str {
        match self {
            Proof::Web => "web",
            Proof::Cloudflare { .. } => "dns-cloudflare",
        }
    }
}

/// Never the token.
impl std::fmt::Debug for Proof {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub proof: Proof,
    pub names: Vec<String>,
    /// The authority's directory: Let's Encrypt's unless the settings file
    /// names another (its staging one, or a private authority).
    pub directory: String,
    /// Another address for Cloudflare's API (a stand-in, for a trial).
    pub cloudflare_api: Option<String>,
}

impl Settings {
    /// From the settings file's variables (`get`): none unless
    /// SCANNERPULSE_ACME is set.
    pub fn read(get: impl Fn(&str) -> Option<String>) -> Result<Option<Settings>, String> {
        let get = |name: &str| get(name).filter(|v| !v.trim().is_empty());
        let Some(proof) = get("SCANNERPULSE_ACME") else {
            return Ok(None);
        };
        let proof = match proof.trim() {
            "web" => Proof::Web,
            "dns-cloudflare" => Proof::Cloudflare {
                token: get("SCANNERPULSE_CLOUDFLARE_TOKEN").ok_or(
                    "SCANNERPULSE_ACME=dns-cloudflare needs SCANNERPULSE_CLOUDFLARE_TOKEN",
                )?,
            },
            other => {
                return Err(format!(
                    "SCANNERPULSE_ACME is {other:?}: web or dns-cloudflare"
                ));
            }
        };
        let names: Vec<String> = get("SCANNERPULSE_ACME_NAMES")
            .ok_or("SCANNERPULSE_ACME needs SCANNERPULSE_ACME_NAMES, the site's names")?
            .split_whitespace()
            .map(str::to_ascii_lowercase)
            .collect();
        for name in &names {
            check_name(name)?;
        }
        Ok(Some(Settings {
            proof,
            names,
            directory: get("SCANNERPULSE_ACME_DIRECTORY").unwrap_or_else(|| LETS_ENCRYPT.into()),
            cloudflare_api: get("SCANNERPULSE_CLOUDFLARE_API"),
        }))
    }

    pub fn from_env() -> Result<Option<Settings>, String> {
        Settings::read(|name| std::env::var(name).ok())
    }
}

/// Endings that only private networks use: no public authority proves a
/// name under them.
const PRIVATE: [&str; 9] = [
    "lan",
    "local",
    "home.arpa",
    "internal",
    "localhost",
    "test",
    "invalid",
    "home",
    "corp",
];

/// A name the authority can prove: a DNS name of two labels or more, not an
/// address, not a wildcard, and not under an ending private networks use.
pub fn check_name(name: &str) -> Result<(), String> {
    let lower = name.to_ascii_lowercase();
    if PRIVATE
        .iter()
        .any(|end| lower.ends_with(&format!(".{end}")))
    {
        return Err(format!(
            "{name:?} is a private name: Let's Encrypt proves only names under a public domain"
        ));
    }
    let label = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    };
    let labels: Vec<&str> = name.split('.').collect();
    let good = name.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(|l| label(l))
        && !labels[labels.len() - 1].chars().all(|c| c.is_ascii_digit());
    if good {
        Ok(())
    } else {
        Err(format!(
            "{name:?} is not a name Let's Encrypt can prove: write a name such as radio.example.org"
        ))
    }
}

fn same_names(have: &[String], want: &[String]) -> bool {
    let set = |names: &[String]| {
        let mut names: Vec<String> = names.iter().map(|n| n.to_ascii_lowercase()).collect();
        names.sort();
        names.dedup();
        names
    };
    set(have) == set(want)
}

/// Whether the certificate served (`found`) should give way to a new one:
/// it is not the authority's for exactly `names`, or a third of its life or
/// less is left.
pub fn due(found: &crate::cert::Found, names: &[String], now: DateTime<Utc>) -> bool {
    if found.self_signed || !same_names(&found.names, names) {
        return true;
    }
    found.not_after - now <= (found.not_after - found.not_before) / 3
}

/// The certificates the web serves: the site's own, replaced in place when
/// a new one arrives, and while a TLS-ALPN-01 check is under way, the
/// challenge's, shown only to a client that asks for `acme-tls/1`.
pub struct Certs {
    current: RwLock<Arc<CertifiedKey>>,
    challenges: Mutex<HashMap<String, Arc<CertifiedKey>>>,
}

impl Certs {
    pub fn new(current: Arc<CertifiedKey>) -> Certs {
        Certs {
            current: RwLock::new(current),
            challenges: Mutex::default(),
        }
    }

    pub fn current(&self) -> Arc<CertifiedKey> {
        self.current
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn serve(&self, key: Arc<CertifiedKey>) {
        *self.current.write().unwrap_or_else(|e| e.into_inner()) = key;
    }

    /// Shows `name`'s TLS-ALPN-01 challenge certificate (RFC 8737), which
    /// carries `digest`, the SHA-256 of the key authorization.
    pub fn answer(&self, name: &str, digest: &[u8]) -> Result<(), String> {
        let made = |error: rcgen::Error| format!("cannot make the challenge certificate: {error}");
        let mut params = rcgen::CertificateParams::new(vec![name.to_string()]).map_err(made)?;
        params.custom_extensions = vec![rcgen::CustomExtension::new_acme_identifier(digest)];
        let pair = rcgen::KeyPair::generate().map_err(made)?;
        let cert = params.self_signed(&pair).map_err(made)?;
        let key = rustls::pki_types::PrivateKeyDer::Pkcs8(pair.serialize_der().into());
        let signing = rustls::crypto::ring::sign::any_supported_type(&key)
            .map_err(|error| format!("cannot use the challenge key: {error}"))?;
        self.challenges
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                name.to_string(),
                Arc::new(CertifiedKey::new(vec![cert.der().clone()], signing)),
            );
        Ok(())
    }

    pub fn forget(&self, name: &str) {
        self.challenges
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(name);
    }
}

impl std::fmt::Debug for Certs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Certs")
    }
}

impl ResolvesServerCert for Certs {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        if hello.alpn().is_some_and(|mut p| p.any(|p| p == ACME_TLS)) {
            let name = hello.server_name()?;
            return self
                .challenges
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(name)
                .cloned();
        }
        Some(self.current())
    }
}

/// The web's TLS settings: `certs` chooses the certificate, and the ALPN
/// list adds `acme-tls/1` to HTTP's.
pub fn server_config(certs: Arc<Certs>) -> rustls::ServerConfig {
    let mut config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(certs);
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec(), ACME_TLS.to_vec()];
    config
}

/// The certificate kept in `dir` (cert.pem, key.pem), if it is for exactly
/// `names` and has not ended.
pub fn stored(dir: &Path, settings: &Settings) -> Option<CertifiedKey> {
    // A certificate from another authority (Let's Encrypt's staging server,
    // say) is not this one's, whatever it names.
    let authority = std::fs::read_to_string(dir.join("authority")).ok()?;
    if authority.trim() != settings.directory {
        return None;
    }
    let key = crate::cert::certified(&dir.join("cert.pem"), &dir.join("key.pem")).ok()?;
    let found = crate::cert::found(&key).ok()?;
    (same_names(&found.names, &settings.names) && found.not_after > Utc::now()).then_some(key)
}

/// Serves the certificate the authority gave (`chain` and `key`, PEM), then
/// keeps it for a restart. One that cannot be kept is still served: asking
/// again would only spend the authority's limit on new certificates.
pub fn adopt(
    dir: &Path,
    settings: &Settings,
    certs: &Certs,
    chain: &str,
    key: &str,
) -> Result<(), String> {
    let served = crate::cert::certified_from(
        chain.as_bytes(),
        key.as_bytes(),
        "the new certificate",
        "its key",
    )?;
    certs.serve(Arc::new(served));
    if let Err(why) = keep_certificate(dir, settings, chain, key) {
        crate::log::warn(format!(
            "acme: the new certificate is served, but cannot be kept for a restart: {why}"
        ));
    }
    Ok(())
}

/// Keeps the certificate the authority gave (`chain` and `key`, PEM) in
/// `dir`, with the authority's directory beside it.
pub fn keep_certificate(
    dir: &Path,
    settings: &Settings,
    chain: &str,
    key: &str,
) -> Result<(), String> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|error| format!("{}: {error}", dir.display()))?;
    keep(&dir.join("key.pem"), key.as_bytes(), 0o600)?;
    keep(&dir.join("cert.pem"), chain.as_bytes(), 0o644)?;
    keep(&dir.join("authority"), settings.directory.as_bytes(), 0o644)
}

/// What the admin page shows of the renewals.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Renewal {
    pub tried: Option<DateTime<Utc>>,
    pub renewed: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

impl Renewal {
    /// A request starts: the last one's error is behind it.
    pub fn begin(&mut self, now: DateTime<Utc>) {
        self.tried = Some(now);
        self.error = None;
    }
}

/// Serves the certificate kept in `dir` when this authority gave it for
/// these names, and renews in the background: at once when the one served
/// is not that kept one, then whenever a third of its life is left.
pub fn start(settings: Settings, dir: PathBuf, certs: Arc<Certs>, renewal: Arc<Mutex<Renewal>>) {
    let mut ours = match stored(&dir, &settings) {
        Some(kept) => {
            certs.serve(Arc::new(kept));
            true
        }
        None => false,
    };
    tokio::spawn(async move {
        let names = settings.names.join(", ");
        loop {
            let due = !ours
                || crate::cert::found(&certs.current())
                    .map_or(true, |found| due(&found, &settings.names, Utc::now()));
            let wait = if due {
                renewal
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .begin(Utc::now());
                let result = obtain(&settings, &dir, &certs, ORDER_LIMIT).await;
                let mut kept = renewal.lock().unwrap_or_else(|e| e.into_inner());
                match result {
                    Ok(()) => {
                        ours = true;
                        crate::log::info(format!("acme: a new certificate for {names} is served"));
                        kept.renewed = Some(Utc::now());
                        kept.error = None;
                        CHECK
                    }
                    Err(why) => {
                        crate::log::warn(format!("acme: no certificate for {names}: {why}"));
                        kept.error = Some(why);
                        RETRY
                    }
                }
            } else {
                CHECK
            };
            tokio::time::sleep(wait).await;
        }
    });
}

/// Asks the authority for a certificate for `settings.names`, proving each
/// one, within `limit`; keeps it in `dir`, and serves it.
pub async fn obtain(
    settings: &Settings,
    dir: &Path,
    certs: &Certs,
    limit: Duration,
) -> Result<(), String> {
    use instant_acme::{
        AuthorizationStatus, ChallengeType, Identifier, NewOrder, OrderStatus, RetryPolicy,
    };
    let cloudflare = match &settings.proof {
        Proof::Web => None,
        Proof::Cloudflare { token } => Some(match &settings.cloudflare_api {
            Some(api) => Cloudflare::with_api(api, token),
            None => Cloudflare::new(token),
        }),
    };
    let mut records = Vec::new();
    let polls = RetryPolicy::new()
        .initial_delay(POLLS.0)
        .backoff(POLLS.1)
        .timeout(POLLS.2);
    let attempt = async {
        let account = account(settings, dir).await?;
        let identifiers: Vec<Identifier> = settings
            .names
            .iter()
            .map(|n| Identifier::Dns(n.clone()))
            .collect();
        let mut order = account
            .new_order(&NewOrder::new(&identifiers))
            .await
            .map_err(|error| format!("the authority refused the order: {error}"))?;
        let mut authorizations = order.authorizations();
        while let Some(authz) = authorizations.next().await {
            let mut authz = authz.map_err(|error| format!("the authority: {error}"))?;
            match authz.status {
                AuthorizationStatus::Pending => {}
                AuthorizationStatus::Valid => continue,
                other => {
                    return Err(format!(
                        "{} is {other:?} at the authority",
                        authz.identifier()
                    ));
                }
            }
            let kind = match cloudflare {
                None => ChallengeType::TlsAlpn01,
                Some(_) => ChallengeType::Dns01,
            };
            let mut challenge = authz
                .challenge(kind.clone())
                .ok_or_else(|| format!("the authority offers no {kind:?} check"))?;
            let name = challenge.identifier().to_string();
            let proof = challenge.key_authorization();
            match &cloudflare {
                None => certs.answer(&name, proof.digest().as_ref())?,
                Some(cf) => {
                    let zone = cf.zone(&name).await?;
                    let id = cf
                        .add_txt(
                            &zone,
                            &format!("_acme-challenge.{name}"),
                            &proof.dns_value(),
                        )
                        .await?;
                    records.push((zone, id));
                    tokio::time::sleep(DNS_WAIT).await;
                }
            }
            challenge
                .set_ready()
                .await
                .map_err(|error| format!("the authority: {error}"))?;
        }
        let status = order
            .poll_ready(&polls)
            .await
            .map_err(|error| format!("the authority: {error}"))?;
        if status != OrderStatus::Ready {
            return Err(refusal(&mut order).await);
        }
        let key = order
            .finalize()
            .await
            .map_err(|error| format!("the authority would not sign: {error}"))?;
        let chain = order
            .poll_certificate(&polls)
            .await
            .map_err(|error| format!("the certificate did not come: {error}"))?;
        Ok((chain, key))
    };
    // Whatever happened, the proofs go after.
    let proved = tokio::time::timeout(limit, attempt)
        .await
        .unwrap_or_else(|_| {
            Err(format!(
                "the authority did not answer within {} s",
                limit.as_secs()
            ))
        });
    for name in &settings.names {
        certs.forget(name);
    }
    if let Some(cf) = &cloudflare {
        for (zone, id) in &records {
            if let Err(why) = cf.remove(zone, id).await {
                crate::log::warn(format!("acme: a proof record stays at Cloudflare: {why}"));
            }
        }
    }
    let (chain, key) = proved?;
    adopt(dir, settings, certs, &chain, &key)
}

/// What the authority said of each name it could not prove.
async fn refusal(order: &mut instant_acme::Order) -> String {
    let mut said = Vec::new();
    let mut authorizations = order.authorizations();
    while let Some(Ok(mut authz)) = authorizations.next().await {
        let Ok(state) = authz.refresh().await else {
            continue;
        };
        let name = state.identifier().to_string();
        for problem in state.challenges.iter().filter_map(|c| c.error.as_ref()) {
            said.push(format!(
                "{name}: {}",
                problem.detail.as_deref().unwrap_or("refused")
            ));
        }
    }
    if said.is_empty() {
        "the authority did not accept the proof".into()
    } else {
        said.join("; ")
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Kept {
    directory: String,
    credentials: instant_acme::AccountCredentials,
}

/// The account kept in `dir` for this authority, or a new one (kept there).
async fn account(settings: &Settings, dir: &Path) -> Result<instant_acme::Account, String> {
    use instant_acme::{Account, NewAccount};
    let path = dir.join("account.json");
    let builder =
        || Account::builder().map_err(|error| format!("cannot reach the authority: {error}"));
    let kept = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Kept>(&bytes).ok())
        .filter(|kept| kept.directory == settings.directory);
    if let Some(kept) = kept {
        return builder()?
            .from_credentials(kept.credentials)
            .await
            .map_err(|error| format!("the authority's account: {error}"));
    }
    let (account, credentials) = builder()?
        .create(
            &NewAccount {
                contact: &[],
                terms_of_service_agreed: true,
                only_return_existing: false,
            },
            settings.directory.clone(),
            None,
        )
        .await
        .map_err(|error| format!("cannot open an account at {}: {error}", settings.directory))?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|error| format!("{}: {error}", dir.display()))?;
    let json = serde_json::to_vec(&Kept {
        directory: settings.directory.clone(),
        credentials,
    })
    .map_err(|error| error.to_string())?;
    keep(&path, &json, 0o600)?;
    Ok(account)
}

/// Writes `bytes` to `path` whole: to a file beside it first, then renamed
/// over it, so a reader never meets half a file.
fn keep(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    let new = path.with_extension("new");
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(&new)
        .and_then(|mut file| file.write_all(bytes).and_then(|()| file.sync_all()))
        .and_then(|()| std::fs::rename(&new, path));
    written.map_err(|error| format!("cannot write {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    /// When the authority's client looks again, for a first wait of
    /// `delay`, a factor of `backoff` and a `limit` (seconds): it sleeps,
    /// grows the wait, and stops when the next wait would pass the limit.
    fn checks(delay: f32, backoff: f32, limit: f32) -> Vec<f32> {
        let (mut at, mut delay, mut seen) = (0.0, delay, Vec::new());
        loop {
            at += delay;
            delay *= backoff;
            if at + delay > limit {
                return seen;
            }
            seen.push(at);
        }
    }

    #[test]
    fn a_new_request_clears_the_last_ones_error() {
        let mut renewal = super::Renewal {
            error: Some("radio.example.org: refused".into()),
            ..Default::default()
        };
        let now = chrono::Utc::now();
        renewal.begin(now);
        assert_eq!(renewal.tried, Some(now));
        assert_eq!(renewal.error, None, "the page shows the request under way");
    }

    #[test]
    fn the_polls_look_until_near_their_limit() {
        let (delay, backoff, limit) = super::POLLS;
        let at = checks(delay.as_secs_f32(), backoff, limit.as_secs_f32());
        assert!(
            *at.last().unwrap() >= limit.as_secs_f32() - 10.0,
            "the last look comes long before the limit: {at:?}"
        );
        assert!(at.len() <= 60, "too many looks: {at:?}");
    }
}
