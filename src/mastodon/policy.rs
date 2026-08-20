use super::types::StatusVisibility;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountLifecycle {
    Active,
    Limited,
    Moved,
    Memorial,
    TemporarilySuspended,
    PermanentlyUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountSuspension {
    None,
    Temporary,
    Permanent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccountLifecycleFacts {
    pub suspension: AccountSuspension,
    pub silenced: bool,
    pub moved: bool,
    pub memorial: bool,
}

impl AccountLifecycleFacts {
    #[must_use]
    pub const fn browser_authentication_allowed(self) -> bool {
        !self.memorial
    }

    #[must_use]
    pub const fn functional_access_allowed(self) -> bool {
        AccountLifecycle::classify(self).functional_access_allowed()
    }
}

impl AccountLifecycle {
    #[must_use]
    pub const fn classify(facts: AccountLifecycleFacts) -> Self {
        if matches!(facts.suspension, AccountSuspension::Temporary) {
            Self::TemporarilySuspended
        } else if matches!(facts.suspension, AccountSuspension::Permanent) {
            Self::PermanentlyUnavailable
        } else if facts.memorial {
            Self::Memorial
        } else if facts.moved {
            Self::Moved
        } else if facts.silenced {
            Self::Limited
        } else {
            Self::Active
        }
    }

    #[must_use]
    pub const fn functional_access_allowed(self) -> bool {
        matches!(self, Self::Active | Self::Limited)
    }

    #[must_use]
    pub const fn unavailable(self) -> bool {
        matches!(
            self,
            Self::TemporarilySuspended | Self::PermanentlyUnavailable
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DomainSeverity {
    Silence,
    Suspend,
    Noop,
    Unknown(i32),
}

impl From<Option<i32>> for DomainSeverity {
    fn from(value: Option<i32>) -> Self {
        match value {
            Some(0) => Self::Silence,
            Some(1) => Self::Suspend,
            Some(2) => Self::Noop,
            Some(value) => Self::Unknown(value),
            None => Self::Unknown(i32::MIN),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GlobalDomainRule {
    pub domain: String,
    pub severity: DomainSeverity,
    pub reject_media: bool,
    pub reject_reports: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GlobalDomainPolicy<'a> {
    Allowed,
    Matched(&'a GlobalDomainRule),
    InvalidDomain,
}

impl GlobalDomainPolicy<'_> {
    #[must_use]
    pub const fn blocks_federation(self) -> bool {
        matches!(
            self,
            Self::InvalidDomain
                | Self::Matched(GlobalDomainRule {
                    severity: DomainSeverity::Suspend | DomainSeverity::Unknown(_),
                    ..
                })
        )
    }

    #[must_use]
    pub const fn limits_account(self) -> bool {
        matches!(
            self,
            Self::Matched(GlobalDomainRule {
                severity: DomainSeverity::Silence,
                ..
            })
        )
    }

    #[must_use]
    pub const fn rejects_media(self) -> bool {
        self.blocks_federation() || matches!(self, Self::Matched(rule) if rule.reject_media)
    }

    #[must_use]
    pub const fn rejects_reports(self) -> bool {
        self.blocks_federation() || matches!(self, Self::Matched(rule) if rule.reject_reports)
    }
}

#[must_use]
pub fn global_domain_policy<'a>(
    domain: &str,
    rules: &'a [GlobalDomainRule],
) -> GlobalDomainPolicy<'a> {
    let Some(domain) = normalize_policy_domain(domain) else {
        return GlobalDomainPolicy::InvalidDomain;
    };
    rules
        .iter()
        .filter_map(|rule| normalize_policy_domain(&rule.domain).map(|domain| (rule, domain)))
        .filter(|(_, rule_domain)| {
            domain == *rule_domain
                || domain
                    .strip_suffix(rule_domain.as_str())
                    .is_some_and(|prefix| prefix.ends_with('.'))
        })
        .max_by_key(|(_, domain)| domain.len())
        .map_or(GlobalDomainPolicy::Allowed, |(rule, _)| {
            GlobalDomainPolicy::Matched(rule)
        })
}

fn normalize_policy_domain(value: &str) -> Option<String> {
    let value = value.trim().replace('/', "");
    let value = value.trim_end_matches('.');
    if value.is_empty() || value.chars().any(char::is_whitespace) {
        return None;
    }
    #[allow(deprecated)]
    let domain = {
        let mut output = String::new();
        let mut processor = idna::Idna::new(idna::Config::default().transitional_processing(true));
        processor.to_ascii(value, &mut output).ok().map(|()| output)
    };
    domain.filter(|domain| {
        !domain.is_empty()
            && !domain.starts_with('.')
            && !domain.ends_with('.')
            && domain.split('.').all(|label| {
                !label.is_empty()
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            })
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct StatusAccessFacts {
    pub visibility: StatusVisibility,
    pub availability: StatusAvailability,
    pub viewer: ViewerFacts,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StatusAvailability {
    Available,
    Deleted,
    AuthorSuspended,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ViewerFacts {
    Anonymous,
    Authenticated(AuthenticatedViewerFacts),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AuthenticatedViewerFacts {
    pub is_author: bool,
    pub follows_author: bool,
    pub is_mentioned: bool,
    pub author_restriction: AuthorRestriction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AuthorRestriction {
    None,
    BlocksViewer,
    BlocksViewerDomain,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StatusAccess {
    Allowed,
    Denied(StatusAccessDenied),
}

impl StatusAccess {
    #[must_use]
    pub(crate) const fn is_allowed(self) -> bool {
        matches!(self, Self::Allowed)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StatusAccessDenied {
    StatusDeleted,
    AuthorSuspended,
    AuthenticationRequired,
    AuthorBlocksViewer,
    AuthorDomainBlocksViewer,
    NotInAudience,
    UnsupportedVisibility,
}

#[must_use]
pub(crate) const fn status_access(facts: StatusAccessFacts) -> StatusAccess {
    match facts.availability {
        StatusAvailability::Available => {}
        StatusAvailability::Deleted => {
            return StatusAccess::Denied(StatusAccessDenied::StatusDeleted);
        }
        StatusAvailability::AuthorSuspended => {
            return StatusAccess::Denied(StatusAccessDenied::AuthorSuspended);
        }
    }
    if matches!(facts.visibility, StatusVisibility::Unknown(_)) {
        return StatusAccess::Denied(StatusAccessDenied::UnsupportedVisibility);
    }
    let ViewerFacts::Authenticated(viewer) = facts.viewer else {
        return match facts.visibility {
            StatusVisibility::Public | StatusVisibility::Unlisted => StatusAccess::Allowed,
            StatusVisibility::Private | StatusVisibility::Direct | StatusVisibility::Limited => {
                StatusAccess::Denied(StatusAccessDenied::AuthenticationRequired)
            }
            StatusVisibility::Unknown(_) => {
                StatusAccess::Denied(StatusAccessDenied::UnsupportedVisibility)
            }
        };
    };
    if viewer.is_author {
        return StatusAccess::Allowed;
    }
    match facts.visibility {
        StatusVisibility::Public | StatusVisibility::Unlisted => match viewer.author_restriction {
            AuthorRestriction::None => StatusAccess::Allowed,
            AuthorRestriction::BlocksViewer => {
                StatusAccess::Denied(StatusAccessDenied::AuthorBlocksViewer)
            }
            AuthorRestriction::BlocksViewerDomain => {
                StatusAccess::Denied(StatusAccessDenied::AuthorDomainBlocksViewer)
            }
        },
        StatusVisibility::Private => {
            if viewer.follows_author || viewer.is_mentioned {
                StatusAccess::Allowed
            } else {
                StatusAccess::Denied(StatusAccessDenied::NotInAudience)
            }
        }
        StatusVisibility::Direct | StatusVisibility::Limited => {
            if viewer.is_mentioned {
                StatusAccess::Allowed
            } else {
                StatusAccess::Denied(StatusAccessDenied::NotInAudience)
            }
        }
        StatusVisibility::Unknown(_) => {
            StatusAccess::Denied(StatusAccessDenied::UnsupportedVisibility)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct StatusContextFacts {
    pub access: StatusAccessFacts,
    pub author_silenced: bool,
    pub viewer_restriction: ViewerRestriction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ViewerRestriction {
    None,
    BlocksAuthor,
    BlocksAuthorDomain,
    MutesAuthor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StatusContextAccess {
    Allowed,
    Denied(StatusContextDenied),
}

impl StatusContextAccess {
    #[must_use]
    pub(crate) const fn is_allowed(self) -> bool {
        matches!(self, Self::Allowed)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StatusContextDenied {
    StatusAccess(StatusAccessDenied),
    ViewerBlocksAuthor,
    ViewerDomainBlocksAuthor,
    ViewerMutesAuthor,
    SilencedAuthor,
}

#[must_use]
pub(crate) const fn status_context_access(facts: StatusContextFacts) -> StatusContextAccess {
    if let StatusAccess::Denied(reason) = status_access(facts.access) {
        return StatusContextAccess::Denied(StatusContextDenied::StatusAccess(reason));
    }
    let ViewerFacts::Authenticated(viewer) = facts.access.viewer else {
        return if facts.author_silenced {
            StatusContextAccess::Denied(StatusContextDenied::SilencedAuthor)
        } else {
            StatusContextAccess::Allowed
        };
    };
    if viewer.is_author {
        return StatusContextAccess::Allowed;
    }
    match facts.viewer_restriction {
        ViewerRestriction::BlocksAuthor => {
            return StatusContextAccess::Denied(StatusContextDenied::ViewerBlocksAuthor);
        }
        ViewerRestriction::BlocksAuthorDomain => {
            return StatusContextAccess::Denied(StatusContextDenied::ViewerDomainBlocksAuthor);
        }
        ViewerRestriction::MutesAuthor => {
            return StatusContextAccess::Denied(StatusContextDenied::ViewerMutesAuthor);
        }
        ViewerRestriction::None => {}
    }
    if facts.author_silenced && !viewer.follows_author {
        StatusContextAccess::Denied(StatusContextDenied::SilencedAuthor)
    } else {
        StatusContextAccess::Allowed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn public_facts() -> StatusAccessFacts {
        StatusAccessFacts {
            visibility: StatusVisibility::Public,
            availability: StatusAvailability::Available,
            viewer: ViewerFacts::Anonymous,
        }
    }

    #[test]
    fn status_access_is_fail_closed_and_preserves_explicit_audiences() {
        let mut facts = public_facts();
        assert_eq!(status_access(facts), StatusAccess::Allowed);

        facts.viewer = ViewerFacts::Authenticated(AuthenticatedViewerFacts {
            is_author: false,
            follows_author: false,
            is_mentioned: false,
            author_restriction: AuthorRestriction::BlocksViewer,
        });
        assert_eq!(
            status_access(facts),
            StatusAccess::Denied(StatusAccessDenied::AuthorBlocksViewer)
        );

        facts.visibility = StatusVisibility::Private;
        assert_eq!(
            status_access(facts),
            StatusAccess::Denied(StatusAccessDenied::NotInAudience)
        );
        let ViewerFacts::Authenticated(mut viewer) = facts.viewer else {
            unreachable!();
        };
        viewer.is_mentioned = true;
        facts.viewer = ViewerFacts::Authenticated(viewer);
        assert_eq!(status_access(facts), StatusAccess::Allowed);

        facts.visibility = StatusVisibility::Direct;
        viewer.is_mentioned = false;
        viewer.follows_author = true;
        facts.viewer = ViewerFacts::Authenticated(viewer);
        assert_eq!(
            status_access(facts),
            StatusAccess::Denied(StatusAccessDenied::NotInAudience)
        );
        viewer.is_author = true;
        facts.viewer = ViewerFacts::Authenticated(viewer);
        assert_eq!(status_access(facts), StatusAccess::Allowed);

        facts.availability = StatusAvailability::AuthorSuspended;
        assert_eq!(
            status_access(facts),
            StatusAccess::Denied(StatusAccessDenied::AuthorSuspended)
        );
        facts.availability = StatusAvailability::Available;
        facts.visibility = StatusVisibility::Unknown(99);
        assert_eq!(
            status_access(facts),
            StatusAccess::Denied(StatusAccessDenied::UnsupportedVisibility)
        );
        facts.visibility = StatusVisibility::Public;
        facts.availability = StatusAvailability::Deleted;
        assert_eq!(
            status_access(facts),
            StatusAccess::Denied(StatusAccessDenied::StatusDeleted)
        );
    }

    #[test]
    fn private_and_limited_statuses_require_authentication() {
        for visibility in [
            StatusVisibility::Private,
            StatusVisibility::Direct,
            StatusVisibility::Limited,
        ] {
            let facts = StatusAccessFacts {
                visibility,
                ..public_facts()
            };
            assert_eq!(
                status_access(facts),
                StatusAccess::Denied(StatusAccessDenied::AuthenticationRequired)
            );
        }
    }

    #[test]
    fn context_policy_adds_deny_only_presentation_filters() {
        let access = StatusAccessFacts {
            viewer: ViewerFacts::Authenticated(AuthenticatedViewerFacts {
                is_author: false,
                follows_author: false,
                is_mentioned: false,
                author_restriction: AuthorRestriction::None,
            }),
            ..public_facts()
        };
        let mut facts = StatusContextFacts {
            access,
            author_silenced: false,
            viewer_restriction: ViewerRestriction::None,
        };
        assert_eq!(status_context_access(facts), StatusContextAccess::Allowed);

        facts.viewer_restriction = ViewerRestriction::MutesAuthor;
        assert_eq!(
            status_context_access(facts),
            StatusContextAccess::Denied(StatusContextDenied::ViewerMutesAuthor)
        );
        facts.viewer_restriction = ViewerRestriction::None;
        facts.author_silenced = true;
        assert_eq!(
            status_context_access(facts),
            StatusContextAccess::Denied(StatusContextDenied::SilencedAuthor)
        );
        let ViewerFacts::Authenticated(mut viewer) = facts.access.viewer else {
            unreachable!();
        };
        viewer.follows_author = true;
        facts.access.viewer = ViewerFacts::Authenticated(viewer);
        assert_eq!(status_context_access(facts), StatusContextAccess::Allowed);
        viewer.is_author = true;
        facts.access.viewer = ViewerFacts::Authenticated(viewer);
        facts.viewer_restriction = ViewerRestriction::BlocksAuthor;
        assert_eq!(status_context_access(facts), StatusContextAccess::Allowed);
    }

    #[test]
    fn account_lifecycle_preserves_suspension_deletion_and_limited_precedence() {
        assert_eq!(
            AccountLifecycle::classify(AccountLifecycleFacts {
                suspension: AccountSuspension::None,
                silenced: false,
                moved: false,
                memorial: false,
            }),
            AccountLifecycle::Active
        );
        assert!(AccountLifecycle::Limited.functional_access_allowed());
        assert!(!AccountLifecycle::Moved.functional_access_allowed());
        assert!(
            AccountLifecycleFacts {
                suspension: AccountSuspension::Temporary,
                silenced: false,
                moved: true,
                memorial: false,
            }
            .browser_authentication_allowed()
        );
        assert!(
            !AccountLifecycleFacts {
                suspension: AccountSuspension::None,
                silenced: false,
                moved: true,
                memorial: true,
            }
            .browser_authentication_allowed()
        );
        assert_eq!(
            AccountLifecycle::classify(AccountLifecycleFacts {
                suspension: AccountSuspension::Temporary,
                silenced: true,
                moved: true,
                memorial: true,
            }),
            AccountLifecycle::TemporarilySuspended
        );
        assert_eq!(
            AccountLifecycle::classify(AccountLifecycleFacts {
                suspension: AccountSuspension::Permanent,
                silenced: false,
                moved: false,
                memorial: false,
            }),
            AccountLifecycle::PermanentlyUnavailable
        );
        assert_eq!(
            AccountLifecycle::classify(AccountLifecycleFacts {
                suspension: AccountSuspension::None,
                silenced: true,
                moved: false,
                memorial: false,
            }),
            AccountLifecycle::Limited
        );
    }

    #[test]
    fn global_domain_policy_uses_the_longest_parent_rule_and_fails_closed() {
        let rules = [
            GlobalDomainRule {
                domain: "example.com".to_owned(),
                severity: DomainSeverity::Silence,
                reject_media: false,
                reject_reports: false,
            },
            GlobalDomainRule {
                domain: "deep.example.com".to_owned(),
                severity: DomainSeverity::Noop,
                reject_media: true,
                reject_reports: false,
            },
            GlobalDomainRule {
                domain: "blocked.invalid".to_owned(),
                severity: DomainSeverity::Suspend,
                reject_media: false,
                reject_reports: false,
            },
            GlobalDomainRule {
                domain: "future.invalid".to_owned(),
                severity: DomainSeverity::Unknown(99),
                reject_media: false,
                reject_reports: false,
            },
        ];
        let deep = global_domain_policy("sub.deep.example.com", &rules);
        assert!(!deep.blocks_federation());
        assert!(!deep.limits_account());
        assert!(deep.rejects_media());
        assert!(global_domain_policy("foo.example.com", &rules).limits_account());
        assert!(global_domain_policy("blocked.invalid", &rules).blocks_federation());
        assert!(global_domain_policy("future.invalid", &rules).blocks_federation());
        assert_eq!(
            global_domain_policy("notexample.com", &rules),
            GlobalDomainPolicy::Allowed
        );
        assert!(global_domain_policy("bad domain", &rules).blocks_federation());
        assert!(global_domain_policy("  deep.example.com/", &rules).rejects_media());
        let idna_rules = [GlobalDomainRule {
            domain: "xn--r9j5b5b".to_owned(),
            severity: DomainSeverity::Suspend,
            reject_media: false,
            reject_reports: false,
        }];
        assert!(global_domain_policy("にゃん", &idna_rules).blocks_federation());
        let transitional_rules = [GlobalDomainRule {
            domain: "fass.de".to_owned(),
            severity: DomainSeverity::Suspend,
            reject_media: false,
            reject_reports: false,
        }];
        assert!(global_domain_policy("faß.de", &transitional_rules).blocks_federation());
        let null_severity = GlobalDomainRule {
            domain: "null.invalid".to_owned(),
            severity: DomainSeverity::from(None),
            reject_media: false,
            reject_reports: false,
        };
        assert!(GlobalDomainPolicy::Matched(&null_severity).blocks_federation());
    }
}
