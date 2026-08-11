use std::fmt;

use sqlx::decode::Decode;
use sqlx::error::BoxDynError;
use sqlx::postgres::{PgTypeInfo, PgValueRef};
use sqlx::{Postgres, Type};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountIdScheme {
    Username,
    Numeric,
    Unknown(i32),
}

impl AccountIdScheme {
    #[must_use]
    pub const fn raw(self) -> i32 {
        match self {
            Self::Username => 0,
            Self::Numeric => 1,
            Self::Unknown(value) => value,
        }
    }
}

impl From<i32> for AccountIdScheme {
    fn from(value: i32) -> Self {
        match value {
            0 => Self::Username,
            1 => Self::Numeric,
            unknown => Self::Unknown(unknown),
        }
    }
}

impl Type<Postgres> for AccountIdScheme {
    fn type_info() -> PgTypeInfo {
        <i32 as Type<Postgres>>::type_info()
    }

    fn compatible(type_info: &PgTypeInfo) -> bool {
        <i32 as Type<Postgres>>::compatible(type_info)
    }
}

impl<'row> Decode<'row, Postgres> for AccountIdScheme {
    fn decode(value: PgValueRef<'row>) -> Result<Self, BoxDynError> {
        Ok(i32::decode(value)?.into())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusVisibility {
    Public,
    Unlisted,
    Private,
    Direct,
    Limited,
    Unknown(i32),
}

impl StatusVisibility {
    #[must_use]
    pub const fn raw(self) -> i32 {
        match self {
            Self::Public => 0,
            Self::Unlisted => 1,
            Self::Private => 2,
            Self::Direct => 3,
            Self::Limited => 4,
            Self::Unknown(value) => value,
        }
    }
}

impl From<i32> for StatusVisibility {
    fn from(value: i32) -> Self {
        match value {
            0 => Self::Public,
            1 => Self::Unlisted,
            2 => Self::Private,
            3 => Self::Direct,
            4 => Self::Limited,
            unknown => Self::Unknown(unknown),
        }
    }
}

impl Type<Postgres> for StatusVisibility {
    fn type_info() -> PgTypeInfo {
        <i32 as Type<Postgres>>::type_info()
    }

    fn compatible(type_info: &PgTypeInfo) -> bool {
        <i32 as Type<Postgres>>::compatible(type_info)
    }
}

impl<'row> Decode<'row, Postgres> for StatusVisibility {
    fn decode(value: PgValueRef<'row>) -> Result<Self, BoxDynError> {
        Ok(i32::decode(value)?.into())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NotificationType {
    Mention,
    Status,
    Reblog,
    Follow,
    FollowRequest,
    Favourite,
    Poll,
    Update,
    SeveredRelationships,
    ModerationWarning,
    AnnualReport,
    AdminSignUp,
    AdminReport,
    Quote,
    QuotedUpdate,
    AddedToCollection,
    CollectionUpdate,
    Unknown(String),
}

impl NotificationType {
    #[must_use]
    pub fn raw(&self) -> &str {
        match self {
            Self::Mention => "mention",
            Self::Status => "status",
            Self::Reblog => "reblog",
            Self::Follow => "follow",
            Self::FollowRequest => "follow_request",
            Self::Favourite => "favourite",
            Self::Poll => "poll",
            Self::Update => "update",
            Self::SeveredRelationships => "severed_relationships",
            Self::ModerationWarning => "moderation_warning",
            Self::AnnualReport => "annual_report",
            Self::AdminSignUp => "admin.sign_up",
            Self::AdminReport => "admin.report",
            Self::Quote => "quote",
            Self::QuotedUpdate => "quoted_update",
            Self::AddedToCollection => "added_to_collection",
            Self::CollectionUpdate => "collection_update",
            Self::Unknown(value) => value,
        }
    }
}

impl From<&str> for NotificationType {
    fn from(value: &str) -> Self {
        match value {
            "mention" => Self::Mention,
            "status" => Self::Status,
            "reblog" => Self::Reblog,
            "follow" => Self::Follow,
            "follow_request" => Self::FollowRequest,
            "favourite" => Self::Favourite,
            "poll" => Self::Poll,
            "update" => Self::Update,
            "severed_relationships" => Self::SeveredRelationships,
            "moderation_warning" => Self::ModerationWarning,
            "annual_report" => Self::AnnualReport,
            "admin.sign_up" => Self::AdminSignUp,
            "admin.report" => Self::AdminReport,
            "quote" => Self::Quote,
            "quoted_update" => Self::QuotedUpdate,
            "added_to_collection" => Self::AddedToCollection,
            "collection_update" => Self::CollectionUpdate,
            unknown => Self::Unknown(unknown.to_owned()),
        }
    }
}

impl Type<Postgres> for NotificationType {
    fn type_info() -> PgTypeInfo {
        <String as Type<Postgres>>::type_info()
    }

    fn compatible(type_info: &PgTypeInfo) -> bool {
        <String as Type<Postgres>>::compatible(type_info)
    }
}

impl<'row> Decode<'row, Postgres> for NotificationType {
    fn decode(value: PgValueRef<'row>) -> Result<Self, BoxDynError> {
        Ok(String::decode(value)?.as_str().into())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountKind {
    Remote,
    LocalService,
    LocalUnavailable,
    LocalLogin,
}

impl AccountKind {
    #[must_use]
    pub const fn classify(domain: Option<&str>, has_user: bool, login_capable_user: bool) -> Self {
        if domain.is_some() {
            Self::Remote
        } else if login_capable_user {
            Self::LocalLogin
        } else if has_user {
            Self::LocalUnavailable
        } else {
            Self::LocalService
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, sqlx::Type)]
#[sqlx(transparent)]
pub struct RawI32(pub i32);

#[derive(Clone, Debug, Eq, PartialEq, sqlx::Type)]
#[sqlx(transparent)]
pub struct RawString(pub String);

#[derive(Clone, Copy, Debug, Eq, PartialEq, sqlx::Type)]
#[sqlx(transparent)]
pub struct PermissionBits(pub i64);

#[derive(Clone, Eq, PartialEq, sqlx::Type)]
#[sqlx(transparent)]
pub struct SecretText(String);

impl SecretText {
    #[must_use]
    pub const fn new(value: String) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn is_present(&self) -> bool {
        !self.0.is_empty()
    }
}

impl fmt::Debug for SecretText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretText([REDACTED])")
    }
}
