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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum UserPermission {
    Administrator = 0,
    ViewDevops = 1,
    ViewAuditLog = 2,
    ViewDashboard = 3,
    ManageReports = 4,
    ManageFederation = 5,
    ManageSettings = 6,
    ManageBlocks = 7,
    ManageTaxonomies = 8,
    ManageAppeals = 9,
    ManageUsers = 10,
    ManageInvites = 11,
    ManageRules = 12,
    ManageAnnouncements = 13,
    ManageCustomEmojis = 14,
    ManageWebhooks = 15,
    InviteUsers = 16,
    ManageRoles = 17,
    ManageUserAccess = 18,
    DeleteUserData = 19,
    ViewFeeds = 20,
    InviteBypassApproval = 21,
    ManageEmailSubscriptions = 22,
}

impl UserPermission {
    #[must_use]
    pub const fn bit(self) -> i64 {
        1_i64 << (self as u8)
    }
}

impl PermissionBits {
    pub const NONE: Self = Self(0);
    pub const ALL: Self = Self((1_i64 << 23) - 1);

    const MODERATION: [UserPermission; 12] = [
        UserPermission::ViewDashboard,
        UserPermission::ViewAuditLog,
        UserPermission::ManageUsers,
        UserPermission::ManageUserAccess,
        UserPermission::DeleteUserData,
        UserPermission::ManageReports,
        UserPermission::ManageAppeals,
        UserPermission::ManageFederation,
        UserPermission::ManageBlocks,
        UserPermission::ManageTaxonomies,
        UserPermission::ManageInvites,
        UserPermission::ViewFeeds,
    ];

    #[must_use]
    pub const fn raw(self) -> i64 {
        self.0
    }

    #[must_use]
    pub const fn contains(self, permission: UserPermission) -> bool {
        self.0 & permission.bit() == permission.bit()
    }

    #[must_use]
    pub fn can_any(self, permissions: &[UserPermission]) -> bool {
        permissions
            .iter()
            .any(|permission| self.contains(*permission))
    }

    #[must_use]
    pub const fn effective(role_id: i64, role: Self, everyone: Self) -> EffectivePermissionBits {
        if role_id == -99 {
            EffectivePermissionBits(role)
        } else if role.contains(UserPermission::Administrator) {
            EffectivePermissionBits(Self::ALL)
        } else {
            EffectivePermissionBits(Self(role.0 | everyone.0))
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EffectivePermissionBits(PermissionBits);

impl EffectivePermissionBits {
    #[must_use]
    pub const fn raw(self) -> i64 {
        self.0.raw()
    }

    #[must_use]
    pub const fn contains(self, permission: UserPermission) -> bool {
        self.0.contains(permission)
    }

    #[must_use]
    pub fn can_any(self, permissions: &[UserPermission]) -> bool {
        self.0.can_any(permissions)
    }

    #[must_use]
    pub fn can_moderate(self) -> bool {
        self.can_any(&PermissionBits::MODERATION)
    }

    #[must_use]
    pub fn bypasses_block(
        self,
        highlighted: bool,
        position: i32,
        target_position: Option<i32>,
    ) -> bool {
        highlighted
            && self.can_moderate()
            && target_position.is_none_or(|target_position| position > target_position)
    }
}

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

#[cfg(test)]
mod tests {
    use super::{PermissionBits, UserPermission};

    #[test]
    fn user_permissions_match_mastodon_4_6_5_bits() {
        let permissions = [
            UserPermission::Administrator,
            UserPermission::ViewDevops,
            UserPermission::ViewAuditLog,
            UserPermission::ViewDashboard,
            UserPermission::ManageReports,
            UserPermission::ManageFederation,
            UserPermission::ManageSettings,
            UserPermission::ManageBlocks,
            UserPermission::ManageTaxonomies,
            UserPermission::ManageAppeals,
            UserPermission::ManageUsers,
            UserPermission::ManageInvites,
            UserPermission::ManageRules,
            UserPermission::ManageAnnouncements,
            UserPermission::ManageCustomEmojis,
            UserPermission::ManageWebhooks,
            UserPermission::InviteUsers,
            UserPermission::ManageRoles,
            UserPermission::ManageUserAccess,
            UserPermission::DeleteUserData,
            UserPermission::ViewFeeds,
            UserPermission::InviteBypassApproval,
            UserPermission::ManageEmailSubscriptions,
        ];
        for (position, permission) in permissions.into_iter().enumerate() {
            assert_eq!(permission.bit(), 1_i64 << position);
        }
        assert_eq!(PermissionBits::ALL.raw(), (1 << 23) - 1);
    }

    #[test]
    fn effective_permissions_preserve_everyone_and_expand_direct_administrators() {
        let everyone = PermissionBits(UserPermission::InviteUsers.bit() | (1_i64 << 60));
        assert_eq!(
            PermissionBits::effective(-99, everyone, everyone).raw(),
            everyone.raw()
        );

        let moderator = PermissionBits(UserPermission::ManageReports.bit());
        let effective = PermissionBits::effective(7, moderator, everyone);
        assert!(effective.contains(UserPermission::ManageReports));
        assert!(effective.contains(UserPermission::InviteUsers));
        assert_eq!(effective.raw() & (1_i64 << 60), 1_i64 << 60);

        let inherited_administrator = PermissionBits(UserPermission::Administrator.bit());
        assert_eq!(
            PermissionBits::effective(7, PermissionBits::NONE, inherited_administrator).raw(),
            inherited_administrator.raw()
        );
        assert_eq!(
            PermissionBits::effective(
                7,
                PermissionBits(UserPermission::Administrator.bit()),
                everyone
            )
            .raw(),
            PermissionBits::ALL.raw()
        );
        assert!(
            PermissionBits::effective(
                7,
                PermissionBits(UserPermission::Administrator.bit()),
                PermissionBits::NONE,
            )
            .bypasses_block(true, 10, Some(9))
        );
        assert!(
            PermissionBits::effective(
                7,
                PermissionBits::NONE,
                PermissionBits(UserPermission::ManageReports.bit()),
            )
            .bypasses_block(true, 10, Some(9))
        );
    }

    #[test]
    fn permission_queries_use_any_semantics_and_role_hierarchy_is_strict() {
        let permissions =
            PermissionBits(UserPermission::ManageReports.bit() | UserPermission::ManageUsers.bit());
        assert!(permissions.can_any(&[
            UserPermission::ManageFederation,
            UserPermission::ManageReports,
        ]));
        assert!(!permissions.can_any(&[
            UserPermission::ManageFederation,
            UserPermission::ManageSettings,
        ]));
        let effective = PermissionBits::effective(7, permissions, PermissionBits::NONE);
        assert!(effective.can_moderate());
        assert!(effective.bypasses_block(true, 10, Some(9)));
        assert!(effective.bypasses_block(true, 10, None));
        assert!(!effective.bypasses_block(true, 10, Some(10)));
        assert!(!effective.bypasses_block(false, 10, Some(9)));
    }
}
