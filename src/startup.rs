use std::fmt;
use std::time::Duration;

use sqlx::{Connection, PgConnection};

use crate::config::Config;
use crate::operational_schema;
use crate::preflight::{self, Diagnostic, Severity};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartupReport {
    diagnostics: Vec<Diagnostic>,
}

impl StartupReport {
    #[must_use]
    pub fn from_diagnostics(diagnostics: impl IntoIterator<Item = Diagnostic>) -> Self {
        let mut diagnostics = diagnostics.into_iter().collect::<Vec<_>>();
        diagnostics.sort_by(|left, right| {
            severity_rank(left.severity())
                .cmp(&severity_rank(right.severity()))
                .then_with(|| left.code().cmp(right.code()))
                .then_with(|| left.message().cmp(right.message()))
        });
        Self { diagnostics }
    }

    #[must_use]
    pub fn is_success(&self) -> bool {
        self.diagnostics
            .iter()
            .all(|diagnostic| diagnostic.severity() == Severity::Warning)
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}

impl fmt::Display for StartupReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let fatal = self
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity() == Severity::Fatal)
            .count();
        let warnings = self.diagnostics.len() - fatal;
        if fatal == 0 {
            write!(formatter, "startup accepted: {warnings} warning")?;
        } else {
            write!(
                formatter,
                "startup refused: {fatal} fatal, {warnings} warning"
            )?;
        }
        for diagnostic in &self.diagnostics {
            write!(formatter, "\n{diagnostic}")?;
        }
        Ok(())
    }
}

const fn severity_rank(severity: Severity) -> u8 {
    match severity {
        Severity::Fatal => 0,
        Severity::Warning => 1,
    }
}

#[must_use]
pub fn operational_failure() -> Diagnostic {
    Diagnostic::fatal(
        "STARTUP_OPERATIONAL_SCHEMA",
        "the Rustodon operational schema or runtime database role is not ready",
        "run the explicit operational migration and grant the documented runtime privileges",
    )
}

/// Validates all production runtime requirements without Redis or persistent side effects.
pub async fn validate(config: &Config) -> StartupReport {
    let mut diagnostics = preflight::runtime_diagnostics(config).await;
    diagnostics.extend(preflight::writer_diagnostics(config).await);
    let operational = async {
        let options = preflight::postgres_options(config).map_err(|_| ())?;
        let mut connection = tokio::time::timeout(
            Duration::from_secs(10),
            PgConnection::connect_with(&options),
        )
        .await
        .map_err(|_| ())?
        .map_err(|_| ())?;
        if let Some(writer) = config
            .write_database
            .as_ref()
            .and_then(preflight::postgres_username_for)
        {
            sqlx::query("SELECT pg_catalog.set_config('rustodon.writer_role', $1, false)")
                .bind(writer)
                .execute(&mut connection)
                .await
                .map_err(|_| ())?;
        }
        operational_schema::validate(&mut connection)
            .await
            .map_err(|_| ())
    }
    .await;
    if operational.is_err() {
        diagnostics.push(operational_failure());
    }
    StartupReport::from_diagnostics(diagnostics)
}
