//! What an admin may change. A super admin may change everything; an
//! associate only the functions a super admin turned on for that person.
//! Reading is never limited.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Super,
    Associate,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::Super => "super",
            Role::Associate => "associate",
        }
    }

    pub fn parse(text: &str) -> Option<Role> {
        [Role::Super, Role::Associate]
            .into_iter()
            .find(|role| role.name() == text)
    }
}

/// A change an associate may be given, one switch each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Function {
    Locations,
    Move,
    Purge,
    Import,
    Consolidate,
    Rename,
    Correct,
    Merge,
    PurgeTalkgroup,
    Recorders,
    Names,
    Update,
    Switchover,
    Convert,
    Transcribe,
    Queue,
    Evaluation,
    Settings,
    Maintenance,
    Favorites,
    StopJobs,
    Rescan,
}

impl Function {
    pub const ALL: [Function; 22] = [
        Function::Locations,
        Function::Move,
        Function::Purge,
        Function::Import,
        Function::Consolidate,
        Function::Rename,
        Function::Correct,
        Function::Merge,
        Function::PurgeTalkgroup,
        Function::Recorders,
        Function::Names,
        Function::Update,
        Function::Switchover,
        Function::Convert,
        Function::Transcribe,
        Function::Queue,
        Function::Evaluation,
        Function::Settings,
        Function::Maintenance,
        Function::Favorites,
        Function::StopJobs,
        Function::Rescan,
    ];

    /// The name stored and typed in commands.
    pub fn name(self) -> &'static str {
        match self {
            Function::Locations => "locations",
            Function::Move => "move",
            Function::Purge => "purge",
            Function::Import => "import",
            Function::Consolidate => "consolidate",
            Function::Rename => "rename",
            Function::Correct => "correct",
            Function::Merge => "merge",
            Function::PurgeTalkgroup => "purge-talkgroup",
            Function::Recorders => "recorders",
            Function::Names => "names",
            Function::Update => "update",
            Function::Switchover => "switchover",
            Function::Convert => "convert",
            Function::Transcribe => "transcribe",
            Function::Queue => "queue",
            Function::Evaluation => "evaluation",
            Function::Settings => "settings",
            Function::Maintenance => "maintenance",
            Function::Favorites => "favorites",
            Function::StopJobs => "stop-jobs",
            Function::Rescan => "rescan",
        }
    }

    /// The group and the words the admin page shows.
    pub fn shown(self) -> (&'static str, &'static str) {
        match self {
            Function::Locations => ("Storage", "Locations"),
            Function::Move => ("Storage", "Move"),
            Function::Purge => ("Storage", "Purge older than"),
            Function::Import => ("Storage", "Import"),
            Function::Consolidate => ("Storage", "Consolidate"),
            Function::Rename => ("Talkgroups", "Rename"),
            Function::Correct => ("Talkgroups", "Correct"),
            Function::Merge => ("Talkgroups", "Merge"),
            Function::PurgeTalkgroup => ("Talkgroups", "Purge talkgroup"),
            Function::Recorders => ("Recorders", "Recorders"),
            Function::Names => ("Recorders", "Names and systems"),
            Function::Update => ("Machines", "Update now"),
            Function::Switchover => ("Machines", "Switchover"),
            Function::Convert => ("Machines", "Convert to HA"),
            Function::Transcribe => ("Transcription", "Transcribe"),
            Function::Queue => ("Transcription", "Queue"),
            Function::Evaluation => ("Transcription", "Evaluation runs"),
            Function::Settings => ("Site", "Settings"),
            Function::Maintenance => ("Site", "Maintenance"),
            Function::Favorites => ("Site", "Favorites"),
            Function::StopJobs => ("Site", "Stop a job"),
            Function::Rescan => ("Site", "Rescan"),
        }
    }

    pub fn parse(text: &str) -> Option<Function> {
        Function::ALL.into_iter().find(|f| f.name() == text)
    }
}

/// Every function, as the admin page lists the switches.
pub fn catalog() -> Vec<serde_json::Value> {
    Function::ALL
        .into_iter()
        .map(|f| {
            let (group, label) = f.shown();
            serde_json::json!({"name": f.name(), "group": group, "label": label})
        })
        .collect()
}

/// What an admin holds: the role and, for an associate, the functions on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub role: Role,
    pub functions: Vec<Function>,
}

impl Grant {
    /// From the account's stored role and function names; a name no
    /// function has (from a newer release) gives nothing.
    pub fn stored(role: &str, functions: &[String]) -> Grant {
        Grant {
            // The column allows only the two names.
            role: Role::parse(role).unwrap_or(Role::Associate),
            functions: functions
                .iter()
                .filter_map(|f| Function::parse(f))
                .collect(),
        }
    }

    pub fn may(&self, function: Function) -> bool {
        self.role == Role::Super || self.functions.contains(&function)
    }

    /// The names of the functions on: every one for a super admin.
    pub fn names(&self) -> Vec<&'static str> {
        Function::ALL
            .into_iter()
            .filter(|f| self.may(*f))
            .map(Function::name)
            .collect()
    }
}

/// The function a setting's change needs; `None` for a setting that decides
/// who signs in, which only a super admin changes.
pub fn setting_function(key: &str) -> Option<Function> {
    if key.starts_with("oidc.") || key == "auth.password_signin" || key == "web.trusted_proxies" {
        None
    } else if key == "maintenance.enabled" {
        Some(Function::Maintenance)
    } else {
        Some(Function::Settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_function_has_its_own_name_and_parses_back() {
        let names: std::collections::HashSet<_> = Function::ALL.map(Function::name).into();
        assert_eq!(names.len(), Function::ALL.len());
        for f in Function::ALL {
            assert_eq!(Function::parse(f.name()), Some(f));
        }
        assert_eq!(Function::parse("delete"), None);
    }

    #[test]
    fn a_super_admin_may_everything_and_an_associate_what_is_on() {
        let boss = Grant::stored("super", &[]);
        assert!(Function::ALL.into_iter().all(|f| boss.may(f)));
        assert_eq!(boss.names().len(), Function::ALL.len());
        let helper = Grant::stored("associate", &["move".into(), "later-thing".into()]);
        assert!(helper.may(Function::Move));
        assert!(!helper.may(Function::Purge));
        assert_eq!(helper.names(), vec!["move"]);
    }

    #[test]
    fn sign_in_settings_need_a_super_admin_and_maintenance_its_own_switch() {
        for key in [
            "oidc.issuer",
            "oidc.admin_group",
            "auth.password_signin",
            "web.trusted_proxies",
        ] {
            assert_eq!(setting_function(key), None, "{key}");
        }
        assert_eq!(
            setting_function("maintenance.enabled"),
            Some(Function::Maintenance)
        );
        assert_eq!(
            setting_function("web.system_order"),
            Some(Function::Settings)
        );
    }
}
