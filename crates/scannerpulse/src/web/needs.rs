//! What each change to the admin API needs. Reading is never limited; a
//! change not listed here is a super admin's only, and a test fails for a
//! change route the router has and this list does not.

use crate::roles::Function;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// The function's switch (always on for a super admin).
    Function(Function),
    /// A super admin.
    Super,
    /// The handler decides from what the request names: a setting's key, the
    /// helper's kind, whose job or whose password.
    Inside,
}

use Function as F;
use Need::{Function as On, Inside, Super};

/// Each change route: its method, its path as the router has it, and its need.
const NEEDS: &[(&str, &str, Need)] = &[
    ("POST", "/api/admin/accounts", Super),
    ("POST", "/api/admin/accounts/{login}/password", Inside),
    ("POST", "/api/admin/accounts/{login}/disable", Super),
    ("POST", "/api/admin/accounts/{login}/enable", Super),
    ("PUT", "/api/admin/accounts/{login}/role", Super),
    (
        "PUT",
        "/api/admin/accounts/{login}/functions/{function}",
        Super,
    ),
    (
        "DELETE",
        "/api/admin/accounts/{login}/functions/{function}",
        Super,
    ),
    ("POST", "/api/admin/rescan", On(F::Rescan)),
    ("POST", "/api/admin/helper", Inside),
    (
        "PUT",
        "/api/admin/talkgroups/{id}/transcribe",
        On(F::Transcribe),
    ),
    ("POST", "/api/admin/queue/retry", On(F::Queue)),
    ("POST", "/api/admin/recorders", On(F::Recorders)),
    (
        "POST",
        "/api/admin/recorders/{id}/{change}",
        On(F::Recorders),
    ),
    ("POST", "/api/admin/names/{decision}", On(F::Names)),
    ("POST", "/api/admin/staging/{decision}", On(F::Names)),
    ("POST", "/api/admin/storage", On(F::Locations)),
    ("DELETE", "/api/admin/storage/{id}", On(F::Locations)),
    ("POST", "/api/admin/storage/move", On(F::Move)),
    ("POST", "/api/admin/storage/purge", On(F::Purge)),
    ("POST", "/api/admin/storage/import/review", On(F::Import)),
    ("POST", "/api/admin/storage/import", On(F::Import)),
    ("POST", "/api/admin/storage/consolidate", On(F::Consolidate)),
    ("POST", "/api/admin/talkgroups/{id}/rename", On(F::Rename)),
    ("POST", "/api/admin/talkgroups/{id}/correct", On(F::Correct)),
    ("POST", "/api/admin/talkgroups/{id}/merge", On(F::Merge)),
    (
        "POST",
        "/api/admin/talkgroups/{id}/purge",
        On(F::PurgeTalkgroup),
    ),
    ("POST", "/api/admin/jobs/queue", On(F::Queue)),
    ("POST", "/api/admin/jobs/asr-run", On(F::Evaluation)),
    ("POST", "/api/admin/jobs/{id}/stop", Inside),
    ("PUT", "/api/admin/favorites/{name}", On(F::Favorites)),
    ("DELETE", "/api/admin/favorites/{name}", On(F::Favorites)),
    ("PUT", "/api/admin/settings/{key}", Inside),
    ("DELETE", "/api/admin/settings/{key}", Inside),
];

/// What a change (`method` other than GET or HEAD) to `path`, the route's
/// pattern, needs.
pub fn need(method: &str, path: &str) -> Need {
    NEEDS
        .iter()
        .find(|(m, p, _)| *m == method && *p == path)
        .map_or(Super, |(_, _, need)| *need)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each admin route the router has, with its change methods, read from
    /// the router's source.
    fn change_routes() -> Vec<(&'static str, String)> {
        let source = include_str!("mod.rs");
        let mut found = Vec::new();
        for route in source.split(".route(").skip(1) {
            let path = route.split('"').nth(1).expect("a route names its path");
            if !path.starts_with("/api/admin") {
                continue;
            }
            for (method, call) in [
                ("POST", "post("),
                ("PUT", "put("),
                ("DELETE", "delete("),
                ("PATCH", "patch("),
            ] {
                if route.contains(call) {
                    found.push((method, path.to_string()));
                }
            }
        }
        found
    }

    #[test]
    fn every_change_route_names_what_it_needs() {
        let routes = change_routes();
        assert!(routes.len() > 20, "{routes:?}");
        for (method, path) in &routes {
            assert!(
                NEEDS.iter().any(|(m, p, _)| m == method && p == path),
                "{method} {path} names no function: add it to NEEDS"
            );
        }
        for (method, path, _) in NEEDS {
            assert!(
                routes.iter().any(|(m, p)| m == method && p == path),
                "NEEDS lists {method} {path}, which the router does not have"
            );
        }
    }

    #[test]
    fn an_unlisted_change_is_a_super_admins_only() {
        assert_eq!(need("POST", "/api/admin/something-new"), Super);
        assert_eq!(need("POST", "/api/admin/storage/move"), On(F::Move));
    }
}
