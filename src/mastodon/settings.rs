use saphyr::{LoadableYamlNode, ScanError, Yaml};

#[derive(Clone, Debug, Eq, PartialEq, sqlx::Type)]
#[sqlx(transparent)]
pub struct RawJsonText(String);

impl RawJsonText {
    #[must_use]
    pub const fn new(raw: String) -> Self {
        Self(raw)
    }

    #[must_use]
    pub fn raw(&self) -> &str {
        &self.0
    }

    /// Parse the retained text as JSON without modifying it.
    ///
    /// # Errors
    ///
    /// Returns an error when the database text is not valid JSON.
    pub fn parse(&self) -> serde_json::Result<serde_json::Value> {
        serde_json::from_str(&self.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, sqlx::Type)]
#[sqlx(transparent)]
pub struct RawYamlText(String);

impl RawYamlText {
    #[must_use]
    pub const fn new(raw: String) -> Self {
        Self(raw)
    }

    #[must_use]
    pub fn raw(&self) -> &str {
        &self.0
    }

    /// Parse the retained text as safe YAML, including user-defined Rails tags.
    ///
    /// # Errors
    ///
    /// Returns an error when the database text is not valid YAML.
    pub fn parse(&self) -> Result<Vec<Yaml<'_>>, ScanError> {
        Yaml::load_from_str(&self.0)
    }
}
