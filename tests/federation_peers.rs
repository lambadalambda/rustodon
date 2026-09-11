//! Real peer smoke. Only tools/federation-peer-smoke supplies guarded disposable peers.
use std::error::Error;
use std::path::PathBuf;
use std::time::Duration;

use reqwest::{Client, Method};
use serde_json::Value;
use sqlx::{PgPool, Row};
use url::Url;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

struct Peer {
    name: &'static str,
    http: Url,
    pool: PgPool,
    token: String,
    local_id: i64,
    actor: String,
}

impl Peer {
    async fn load(name: &'static str, run: &str) -> Result<Self> {
        let prefix = format!("PEER_{}", name.to_ascii_uppercase());
        let http = Url::parse(&std::env::var(format!("{prefix}_HTTP"))?)?;
        let database = Url::parse(&std::env::var(format!("{prefix}_DATABASE"))?)?;
        let pid = run.strip_prefix("peer-").ok_or("invalid run")?;
        if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
            return Err("invalid run".into());
        }
        if http.scheme() != "http"
            || http.host_str() != Some("127.0.0.1")
            || http.port().is_none()
            || http.path() != "/"
            || !http.username().is_empty()
            || http.password().is_some()
            || http.query().is_some()
            || http.fragment().is_some()
            || database.scheme() != "postgresql"
            || database.host_str() != Some("127.0.0.1")
            || database.port().is_none()
            || database.path() != format!("/rustodon_peer_{pid}_{name}")
            || database.query().is_some()
            || database.fragment().is_some()
        {
            return Err("unguarded peer endpoint/database".into());
        }
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(Duration::from_secs(2))
            .after_connect(|connection, _| {
                Box::pin(async move {
                    sqlx::query("SET statement_timeout = '2s'")
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(database.as_str())
            .await?;
        let comment: Option<String> = sqlx::query_scalar(
            "SELECT shobj_description(oid, 'pg_database') FROM pg_database WHERE datname = current_database()",
        ).fetch_one(&pool).await?;
        if comment.as_deref() != Some(&format!("{run}/{name}")) {
            return Err("peer database marker mismatch".into());
        }
        let local_id =
            sqlx::query_scalar("SELECT id FROM accounts WHERE username=$1 AND domain IS NULL")
                .bind(name)
                .fetch_one(&pool)
                .await?;
        let remote_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM accounts WHERE domain IS NOT NULL")
                .fetch_one(&pool)
                .await?;
        assert_eq!(
            remote_count, 0,
            "{name}: discovery must start without remote actors"
        );
        let numeric: bool = sqlx::query_scalar("SELECT id_scheme=1 FROM accounts WHERE id=$1")
            .bind(local_id)
            .fetch_one(&pool)
            .await?;
        let actor = if numeric {
            format!("https://{name}.peer.invalid/ap/users/{local_id}")
        } else {
            format!("https://{name}.peer.invalid/users/{name}")
        };
        // Read only the freshly bootstrapped token in this guarded disposable DB.
        let token: String = sqlx::query_scalar("SELECT t.token FROM oauth_access_tokens t JOIN users u ON u.id=t.resource_owner_id WHERE u.account_id=$1 AND t.revoked_at IS NULL")
            .bind(local_id).fetch_one(&pool).await?;
        Ok(Self {
            name,
            http,
            pool,
            token,
            local_id,
            actor,
        })
    }

    fn actor(&self) -> String {
        self.actor.clone()
    }

    async fn api(
        &self,
        client: &Client,
        method: Method,
        path: &str,
        form: &[(&str, &str)],
    ) -> Result<Value> {
        let request = client
            .request(method.clone(), self.http.join(path)?)
            .header("Host", format!("{}.peer.invalid", self.name))
            .header("X-Forwarded-Proto", "https")
            .bearer_auth(&self.token);
        let request = if method == Method::GET {
            request.query(form)
        } else {
            request.form(form)
        };
        let response = request.send().await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(format!("{} {path}: {status} {body}", self.name).into());
        }
        Ok(serde_json::from_str(&body)?)
    }

    async fn discover(&self, client: &Client, other: &Self) -> Result<i64> {
        let acct = format!("{}@{}.peer.invalid", other.name, other.name);
        let response = self
            .api(
                client,
                Method::GET,
                "/api/v1/accounts/search",
                &[("q", &acct), ("resolve", "true")],
            )
            .await?;
        let accounts = response.as_array().ok_or("missing search accounts")?;
        assert_eq!(accounts.len(), 1, "{} discovery: {response}", self.name);
        assert_eq!(accounts[0]["acct"], acct);
        let id: i64 = accounts[0]["id"]
            .as_str()
            .ok_or("missing account ID")?
            .parse()?;
        let row = sqlx::query("SELECT uri FROM accounts WHERE id=$1")
            .bind(id)
            .fetch_one(&self.pool)
            .await?;
        assert_eq!(row.get::<String, _>("uri"), other.actor());
        let local_key: String = sqlx::query_scalar("SELECT public_key FROM accounts WHERE id=$1")
            .bind(other.local_id)
            .fetch_one(&other.pool)
            .await?;
        // Mastodon stores discovered keys in keypairs and clears the legacy account column.
        let received_keys: Vec<String> = sqlx::query_scalar("SELECT public_key FROM accounts WHERE id=$1 UNION ALL SELECT public_key FROM keypairs WHERE account_id=$1 AND uri=$2 AND NOT revoked AND (expires_at IS NULL OR expires_at > CURRENT_TIMESTAMP)")
            .bind(id).bind(format!("{}#main-key", other.actor())).fetch_all(&self.pool).await?;
        assert!(
            !local_key.is_empty() && received_keys.contains(&local_key),
            "{} received actor signing key mismatch",
            self.name
        );
        println!(
            "PASS {} fresh discovery: {} key matches",
            self.name,
            other.actor()
        );
        Ok(id)
    }

    async fn follows(&self, source: i64, target: i64) -> Result<bool> {
        Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM follows WHERE account_id=$1 AND target_account_id=$2) AND NOT EXISTS(SELECT 1 FROM follow_requests WHERE account_id=$1 AND target_account_id=$2)")
            .bind(source).bind(target).fetch_one(&self.pool).await?)
    }

    async fn received_status(&self, uri: &str, actor: &str, marker: &str) -> Result<bool> {
        // Deliberately SQL only: resolving/fetching the status URL would hide push failures.
        Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM statuses s JOIN accounts a ON a.id=s.account_id WHERE s.uri=$1 AND a.uri=$2 AND s.local=false AND s.visibility=0 AND s.text LIKE '%' || $3 || '%')")
            .bind(uri).bind(actor).bind(marker).fetch_one(&self.pool).await?)
    }
}

#[tokio::test]
#[ignore = "requires task-owned live Mastodon/Rustodon peers from tools/federation-peer-smoke"]
async fn discovery_follow_and_public_push_both_directions() -> Result<()> {
    tokio::time::timeout(Duration::from_mins(3), smoke()).await?
}

async fn smoke() -> Result<()> {
    if !cfg!(all(debug_assertions, feature = "test-support")) {
        return Err("debug test-support build required".into());
    }
    let run = std::env::var("PEER_RUN")?;
    let root = PathBuf::from(std::env::var("PEER_ROOT")?);
    if root
        != PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(&run)
        || std::fs::read_to_string(root.join(".peer-run"))?.trim() != run
    {
        return Err("invalid peer workspace marker".into());
    }
    let mastodon = Peer::load("mastodon", &run).await?;
    let rustodon = Peer::load("rustodon", &run).await?;
    assert_ne!(mastodon.http, rustodon.http);
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()?;
    let rust_on_masto = mastodon.discover(&client, &rustodon).await?;
    let masto_on_rust = rustodon.discover(&client, &mastodon).await?;
    // This is sequential convergence, not simultaneous reciprocal-follow stress.
    follow_and_accept(&client, &mastodon, &rustodon, rust_on_masto, masto_on_rust).await?;
    follow_and_accept(&client, &rustodon, &mastodon, masto_on_rust, rust_on_masto).await?;
    let mut statuses = Vec::new();
    for peer in [&mastodon, &rustodon] {
        let marker = format!("{run}-{}-public-push", peer.name);
        let response = peer
            .api(
                &client,
                Method::POST,
                "/api/v1/statuses",
                &[("status", &marker), ("visibility", "public")],
            )
            .await?;
        let uri = response["uri"]
            .as_str()
            .ok_or("status URI missing")?
            .to_owned();
        assert!(
            uri.starts_with(&format!("{}/statuses/", peer.actor())),
            "unexpected local status identity: {uri}"
        );
        statuses.push((uri, marker));
    }
    let mut received = [false; 2];
    for _ in 0..60 {
        received[0] = rustodon
            .received_status(&statuses[0].0, &mastodon.actor(), &statuses[0].1)
            .await?
            && has_push_audit(&root, &mastodon, &rustodon, &statuses[0].0)?;
        received[1] = mastodon
            .received_status(&statuses[1].0, &rustodon.actor(), &statuses[1].1)
            .await?
            && has_push_audit(&root, &rustodon, &mastodon, &statuses[1].0)?;
        if received.iter().all(|value| *value) {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    for (index, direction) in ["Mastodon -> Rustodon", "Rustodon -> Mastodon"]
        .iter()
        .enumerate()
    {
        println!(
            "{} public PUSH {direction}: {}",
            if received[index] { "PASS" } else { "BLOCKED" },
            statuses[index].0
        );
    }
    assert_eq!(
        received,
        [true, true],
        "public push did not converge; no status resolver fallback was used"
    );
    for (sender, receiver, (uri, _)) in [
        (&mastodon, &rustodon, &statuses[0]),
        (&rustodon, &mastodon, &statuses[1]),
    ] {
        assert_push_audit(&root, sender, receiver, uri)?;
    }
    Ok(())
}

fn assert_push_audit(
    root: &std::path::Path,
    sender: &Peer,
    receiver: &Peer,
    uri: &str,
) -> Result<()> {
    let status_url = Url::parse(uri)?;
    let source_audit =
        std::fs::read_to_string(root.join(format!("{}.peer.invalid.jsonl", sender.name)))?;
    for event in parse_audit(&source_audit)? {
        assert!(
            !(event["method"] == "GET"
                && event["path"]
                    .as_str()
                    .is_some_and(|path| path.split('?').next() == Some(status_url.path()))),
            "status was fetched instead of exclusively pushed: {uri}"
        );
    }
    println!(
        "PASS signed inbox Create audit and no status GET: {} -> {}",
        sender.name, receiver.name
    );
    Ok(())
}

async fn follow_and_accept(
    client: &Client,
    sender: &Peer,
    receiver: &Peer,
    target_on_sender: i64,
    source_on_receiver: i64,
) -> Result<()> {
    sender
        .api(
            client,
            Method::POST,
            &format!("/api/v1/accounts/{target_on_sender}/follow"),
            &[],
        )
        .await?;
    let mut accepted = false;
    for _ in 0..60 {
        accepted = sender.follows(sender.local_id, target_on_sender).await?
            && receiver
                .follows(source_on_receiver, receiver.local_id)
                .await?;
        if accepted {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert!(
        accepted,
        "Follow/Accept did not converge in both databases within 60s; inspect worker logs"
    );
    println!(
        "PASS Follow/Accept {} -> {}: both sender and receiver rows, no pending requests",
        sender.name, receiver.name
    );
    Ok(())
}

#[test]
fn audit_reader_waits_for_complete_records() {
    assert_eq!(parse_audit("{}\n{\"method\":").unwrap().len(), 1);
    assert!(parse_audit("{\"method\":\"POST\"}").unwrap().is_empty());
    assert!(parse_audit("bad\n").is_err());
}

fn parse_audit(text: &str) -> Result<Vec<Value>> {
    Ok(text
        .split_inclusive('\n')
        .filter(|line| line.ends_with('\n'))
        .map(serde_json::from_str)
        .collect::<std::result::Result<_, _>>()?)
}

fn has_push_audit(
    root: &std::path::Path,
    sender: &Peer,
    receiver: &Peer,
    uri: &str,
) -> Result<bool> {
    let path = root.join(format!("{}.peer.invalid.jsonl", receiver.name));
    let audit = match std::fs::read_to_string(path) {
        Ok(audit) => audit,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let events = parse_audit(&audit)?;
    Ok(events.iter().any(|event| {
        event["method"] == "POST"
            && event["path"]
                .as_str()
                .is_some_and(|path| path.ends_with("/inbox"))
            && event["activity"] == "Create"
            && event["object"] == uri
            && event["actor"] == sender.actor()
            && event["public"] == true
            && event["signed"] == true
            && event["status"]
                .as_u64()
                .is_some_and(|status| (200..300).contains(&status))
    }))
}
