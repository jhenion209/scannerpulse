//! An install on one machine converted to high availability (5E3): the plan
//! setup's planner saves on it (`scannerpulse setup`, run again there), and
//! the steps the admin page's guide shows from that plan, each saying what
//! it does, where the person runs it or what the root helper does once an
//! admin approves it, and how it is checked before the next one opens.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::agent::Kind;
use crate::layout::Layout;

/// Where the planner saves the plan: readable by the services, since it
/// holds no password.
pub const PATH: &str = "/etc/scannerpulse/convert.json";

/// The install line, as the README gives it.
pub const INSTALL: &str = "curl -fsSL https://github.com/jhenion209/scannerpulse/releases/latest/download/install.sh | sh";

/// The conversion's plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Convert {
    /// The database machines; the first becomes the primary.
    pub nodes: [String; 2],
    /// This machine's address, as the other machines reach it.
    pub hub: String,
    /// The referee, when the standby is promoted by itself.
    #[serde(default)]
    pub referee: Option<String>,
    /// The second ScannerPulse machine, and the address the two share.
    #[serde(default)]
    pub second: Option<String>,
    #[serde(default)]
    pub shared: Option<String>,
    /// The archive's new folder, a network share both machines mount; none
    /// when the archive stays where it is.
    #[serde(default)]
    pub archive: Option<String>,
}

impl Convert {
    pub fn read(path: &Path) -> Result<Convert, String> {
        let text = std::fs::read_to_string(path).map_err(|error| {
            format!(
                "{}: {error} (plan the conversion with `sudo /usr/local/bin/scannerpulse setup`)",
                path.display()
            )
        })?;
        serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))
    }

    /// Written whole, then put in place, readable by all.
    pub fn write(&self, path: &Path) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let fail = |error: std::io::Error| format!("cannot write {}: {error}", path.display());
        let new = path.with_extension("json.new");
        let _ = std::fs::remove_file(&new);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(&new)
            .map_err(fail)?;
        let text = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        file.write_all(text.as_bytes()).map_err(fail)?;
        file.sync_all().map_err(fail)?;
        std::fs::rename(&new, path).map_err(fail)
    }
}

/// What the guide reads besides the plan.
#[derive(Debug, Default)]
pub struct Seen {
    /// The archive the database names now.
    pub archive: Option<String>,
    /// The install's plan's public part, once the move has begun.
    pub layout: Option<Layout>,
    /// The newest request of each kind the guide asks for: whether it ended
    /// well (none while it waits or runs).
    pub asked: Vec<(Kind, Option<bool>)>,
    /// The ScannerPulse machines that answer, by name.
    pub up: Vec<String>,
    /// A database machine answers as the primary, and another as its
    /// standby, close behind.
    pub standby_current: bool,
    /// Whether the referee answers; none without one.
    pub referee_answers: Option<bool>,
}

/// What the root helper does for a step once an admin approves it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Ask {
    pub kind: Kind,
    pub label: String,
    pub question: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Done,
    /// The step to take now.
    Now,
    Later,
}

/// One step of the guide.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Step {
    pub id: &'static str,
    pub title: String,
    /// What it does, and how it is checked.
    pub text: String,
    /// Where the person runs `run`.
    pub on: Option<String>,
    pub run: Vec<String>,
    pub ask: Option<Ask>,
    pub state: State,
}

/// A check of the last step, and whether it passes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Check {
    pub text: String,
    pub ok: bool,
}

/// The guide: the steps in order, the checks of the last, whether the
/// conversion is complete, and, while the database moves, how to go back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Guide {
    pub steps: Vec<Step>,
    pub checks: Vec<Check>,
    pub complete: bool,
    pub back: Option<String>,
}

/// How to go back while the database moves, before the switch.
pub const BACK: &str = "Until the switch, `sudo /usr/local/bin/scannerpulse db move --cancel` on this machine takes the site back to its own database alone; it says what to do with the machines that joined.";

/// The newest request of `kind` ended as `how`.
fn ended(seen: &Seen, kind: Kind, how: bool) -> bool {
    seen.asked
        .iter()
        .any(|(asked, ok)| *asked == kind && *ok == Some(how))
}

/// What the person runs on a machine joining the install.
fn join_lines() -> Vec<String> {
    vec![
        INSTALL.to_string(),
        "sudo /usr/local/bin/scannerpulse setup".to_string(),
    ]
}

fn join_text(convert: &Convert, what: &str) -> String {
    format!(
        "There, choose Advanced, then \"join an existing install\", and give this machine's address ({}). {what}",
        convert.hub
    )
}

/// The guide for `convert`, from what is `seen`: a step is done once it and
/// every step before it are, and the first that is not is the one to take
/// now.
pub fn guide(convert: &Convert, seen: &Seen) -> Guide {
    let layout = seen.layout.as_ref().filter(|l| l.nodes == convert.nodes);
    let joined = |name: &str| layout.is_some_and(|l| l.joined.iter().any(|j| j == name));
    let switched = layout.is_some_and(|l| !l.moving);
    // Adding the second has the database machines join again, so their
    // first joins are known from the steps after them.
    let added = convert.second.is_some()
        && layout.is_some_and(|l| l.second.is_some() && l.second == convert.second);
    let [db1, db2] = &convert.nodes;
    // Each step, with its own check.
    let mut steps: Vec<(Step, bool)> = Vec::new();
    let mut step = |id, title: String, text: String, on: Option<&str>, run, ask, done| {
        steps.push((
            Step {
                id,
                title,
                text,
                on: on.map(String::from),
                run,
                ask,
                state: State::Later,
            },
            done,
        ));
    };

    if let Some(new) = &convert.archive {
        let moved = seen.archive.as_deref() == Some(new.as_str());
        let old = seen.archive.as_deref().unwrap_or("ARCHIVE");
        step(
            "archive-copy",
            format!("Copy the archive to {new}"),
            "Filing goes on meanwhile. Run the copy as often as you like, then ask the root helper to compare the two: that changes nothing.".into(),
            Some("this machine"),
            vec![crate::setup::archive_move::copy_line(
                Path::new(old),
                Path::new(new),
            )],
            Some(Ask {
                kind: Kind::ArchiveCheck,
                label: "Check the copy".into(),
                question: format!(
                    "Compare the archive with {new} now? Nothing changes; a large archive takes a while."
                ),
            }),
            moved || ended(seen, Kind::ArchiveCheck, true),
        );
        step(
            "archive-switch",
            format!("Switch the archive to {new}"),
            "Filing pauses (the recordings wait in their folder), the calls filed since the check are copied and compared, and the site files into the share. The old archive stays as it was.".into(),
            None,
            Vec::new(),
            Some(Ask {
                kind: Kind::ArchiveSwitch,
                label: format!("Switch the archive to {new}"),
                question: format!(
                    "Pause filing, copy the calls filed since the check, and point the site at {new} now?"
                ),
            }),
            moved,
        );
    }

    let move_ask = |label: &str, question: String| {
        Some(Ask {
            kind: Kind::DbMove,
            label: label.into(),
            question,
        })
    };
    step(
        "db-open",
        format!("Open this machine's database to {db1} and {db2}"),
        "Its PostgreSQL takes their connections, restarting once if its settings need it (a few seconds); the site then names this database and the first database machine, so that it follows whichever is the primary.".into(),
        None,
        Vec::new(),
        move_ask(
            "Open this machine's database",
            format!("Let {db1} and {db2} copy this machine's database now? Writes pause for a few seconds."),
        ),
        // The plan is written first: a move that then stopped is asked
        // again here, until a machine has joined it.
        layout.is_some()
            && (!ended(seen, Kind::DbMove, false)
                || joined(db1)
                || convert.referee.as_deref().is_some_and(joined)),
    );
    if let Some(referee) = &convert.referee {
        step(
            "referee-join",
            format!("Set up {referee} as the referee"),
            join_text(
                convert,
                "It installs pg_auto_failover's monitor, which decides which database machine is the primary.",
            ),
            Some(referee),
            join_lines(),
            None,
            joined(referee),
        );
        step(
            "db-adopt",
            format!("Give this machine's database to the referee {referee}"),
            "pg_auto_failover runs this machine's database from now on, as the referee's first primary: it restarts once (a few seconds).".into(),
            None,
            Vec::new(),
            move_ask(
                "Give the database to the referee",
                format!("Make this machine's database {referee}'s first primary now? Writes pause for a few seconds."),
            ),
            joined(&convert.hub) || switched,
        );
    }
    step(
        "db1-join",
        format!("Join {db1}, the first database machine"),
        join_text(
            convert,
            "It copies this machine's database and follows it: the Layout below shows how far behind it is.",
        ),
        Some(db1),
        join_lines(),
        None,
        joined(db1) || switched,
    );
    step(
        "db-switch",
        format!("Switch the database to {db1}"),
        format!(
            "Once {db1} has caught up, it becomes the primary and the site uses it: writes pause for the seconds it takes, and nothing is lost. This machine's old database is kept until it is removed below."
        ),
        None,
        Vec::new(),
        Some(Ask {
            kind: Kind::DbSwitch,
            label: format!("Switch the database to {db1}"),
            question: format!("Make {db1} the primary now? Writes pause for a few seconds."),
        }),
        switched,
    );
    step(
        "db2-join",
        format!("Join {db2}, the second database machine"),
        join_text(
            convert,
            &format!("It copies {db1} and follows it, as its standby."),
        ),
        Some(db2),
        join_lines(),
        None,
        joined(db2) || added,
    );
    step(
        "db-retire",
        "Remove this machine's old database".into(),
        "Its backups stay in the backups' folder until you remove them; each database machine takes a nightly dump.".into(),
        None,
        Vec::new(),
        Some(Ask {
            kind: Kind::DbRetire,
            label: "Remove the old database".into(),
            question: "Remove this machine's old database for good now? The database machines hold the site's database.".into(),
        }),
        layout.is_some_and(|l| l.retired),
    );

    if let Some(second) = &convert.second {
        let sharing = convert
            .shared
            .as_deref()
            .map(|shared| format!(", sharing {shared} with this one"))
            .unwrap_or_default();
        step(
            "second-add",
            format!("Add {second} as the second ScannerPulse machine{sharing}"),
            match &convert.shared {
                Some(shared) => format!(
                    "keepalived starts here to hold {shared}, the address the two share: point bookmarks and port forwards at it."
                ),
                None => "The install's plan names it, so that it may join.".into(),
            },
            None,
            Vec::new(),
            Some(Ask {
                kind: Kind::Second,
                label: format!("Add {second}"),
                question: format!("Add {second} as the second ScannerPulse machine{sharing} now?"),
            }),
            // The plan names it first: an addition that then stopped is
            // asked again.
            added && !ended(seen, Kind::Second, false),
        );
        step(
            "nodes-rejoin",
            format!("Join {db1} and {db2} again"),
            format!(
                "On each, run setup and join as before (Advanced, \"join an existing install\", {}): each lets {second} in.",
                convert.hub
            ),
            Some(&format!("{db1}, then {db2}")),
            vec!["sudo /usr/local/bin/scannerpulse setup".to_string()],
            None,
            added && joined(db1) && joined(db2),
        );
        step(
            "second-join",
            format!("Join {second}, the second ScannerPulse machine"),
            join_text(
                convert,
                "It checks the archive share's mark, takes this machine's settings and certificate, and starts its services.",
            ),
            Some(second),
            join_lines(),
            None,
            joined(second),
        );
    }

    let mut checks = vec![Check {
        text: "this machine answers".into(),
        ok: seen.up.iter().any(|m| Some(m) != convert.second.as_ref()),
    }];
    if let Some(second) = &convert.second {
        checks.push(Check {
            text: format!("{second} answers"),
            ok: seen.up.contains(second),
        });
    }
    checks.push(Check {
        text: "a database machine is the primary, and the other its standby, in step".into(),
        ok: seen.standby_current,
    });
    if let Some(referee) = &convert.referee {
        checks.push(Check {
            text: format!("the referee {referee} answers"),
            ok: seen.referee_answers == Some(true),
        });
    }
    step(
        "final-check",
        "The final check".into(),
        "Each check below must pass. A test switchover swaps the database machines' roles while both are healthy (writes pause for seconds); run it again to swap them back.".into(),
        None,
        Vec::new(),
        Some(Ask {
            kind: Kind::Switchover,
            label: "Run a test switchover".into(),
            question: "Make the standby the primary, and the primary its standby, now? Writes pause for seconds.".into(),
        }),
        checks.iter().all(|check| check.ok),
    );

    let mut done = true;
    let mut out = Vec::with_capacity(steps.len());
    for (mut step, own) in steps {
        step.state = match (done, own) {
            (true, true) => State::Done,
            (true, false) => {
                done = false;
                State::Now
            }
            (false, _) => State::Later,
        };
        out.push(step);
    }
    Guide {
        complete: done,
        steps: out,
        checks,
        back: layout.is_some_and(|l| l.moving).then(|| BACK.to_string()),
    }
}

/// A standby within this distance of the primary is in step.
pub const IN_STEP_BYTES: u64 = 16 * 1024 * 1024;

/// The layout's database machines (`layout::report`'s "database") hold a
/// primary and a standby in step with it.
pub fn standby_current(database: &serde_json::Value) -> bool {
    let nodes = database.as_array().map(Vec::as_slice).unwrap_or_default();
    nodes.iter().any(|node| node["role"] == "primary")
        && nodes.iter().any(|node| {
            node["role"] == "standby"
                && node["behind_bytes"]
                    .as_u64()
                    .is_some_and(|behind| behind < IN_STEP_BYTES)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> Convert {
        Convert {
            nodes: ["db1.example.org".into(), "db2.example.org".into()],
            hub: "192.0.2.10".into(),
            referee: None,
            second: None,
            shared: None,
            archive: None,
        }
    }

    fn layout(joined: &[&str], moving: bool) -> Layout {
        Layout {
            hub: "192.0.2.10".into(),
            nodes: vec!["db1.example.org".into(), "db2.example.org".into()],
            joined: joined.iter().map(|j| j.to_string()).collect(),
            moving,
            ..Layout::default()
        }
    }

    fn states(guide: &Guide) -> Vec<(&str, State)> {
        guide.steps.iter().map(|s| (s.id, s.state)).collect()
    }

    #[test]
    fn by_hand_the_database_opens_then_each_machine_joins_around_the_switch() {
        let plan = plan();
        let guide = guide(&plan, &Seen::default());
        assert_eq!(
            states(&guide),
            [
                ("db-open", State::Now),
                ("db1-join", State::Later),
                ("db-switch", State::Later),
                ("db2-join", State::Later),
                ("db-retire", State::Later),
                ("final-check", State::Later),
            ]
        );
        assert_eq!(guide.steps[0].ask.as_ref().unwrap().kind, Kind::DbMove);
        assert!(!guide.complete);
        // Opened: database machine 1 joins, with the install line.
        let seen = Seen {
            layout: Some(layout(&[], true)),
            ..Seen::default()
        };
        let guide = super::guide(&plan, &seen);
        let now = &guide.steps[1];
        assert_eq!((now.id, now.state), ("db1-join", State::Now));
        // Until the switch, the move can be cancelled.
        assert!(
            guide
                .back
                .as_deref()
                .is_some_and(|b| b.contains("db move --cancel"))
        );
        assert_eq!(now.on.as_deref(), Some("db1.example.org"));
        assert_eq!(now.run[0], INSTALL);
        assert!(
            now.text
                .contains("give this machine's address (192.0.2.10)")
        );
        // Joined and switched: the second database machine joins.
        let seen = Seen {
            layout: Some(layout(&["db1.example.org"], false)),
            ..Seen::default()
        };
        let guide = super::guide(&plan, &seen);
        assert_eq!(guide.steps[3].state, State::Now, "db2-join");
        assert_eq!(guide.back, None, "after the switch, nothing to cancel");
    }

    #[test]
    fn a_move_that_stopped_after_writing_its_plan_is_asked_again() {
        let seen = Seen {
            layout: Some(layout(&[], true)),
            asked: vec![(Kind::DbMove, Some(false))],
            ..Seen::default()
        };
        let guide = guide(&plan(), &seen);
        assert_eq!(guide.steps[0].state, State::Now);
        // Once database machine 1 has joined, the move did open the
        // database: a later stop is another step's.
        let seen = Seen {
            layout: Some(layout(&["db1.example.org"], true)),
            asked: vec![(Kind::DbMove, Some(false))],
            ..Seen::default()
        };
        assert_eq!(super::guide(&plan(), &seen).steps[2].state, State::Now);
    }

    #[test]
    fn a_step_is_done_only_once_those_before_it_are() {
        // The first database machine has joined, but the move is not marked
        // begun for these machines: an old plan of other machines.
        let seen = Seen {
            layout: Some(Layout {
                nodes: vec!["other1".into(), "other2".into()],
                ..layout(&["db1.example.org"], false)
            }),
            ..Seen::default()
        };
        let guide = guide(&plan(), &seen);
        assert_eq!(guide.steps[0].state, State::Now);
        assert!(guide.steps[1..].iter().all(|s| s.state == State::Later));
    }

    #[test]
    fn with_a_referee_a_second_and_a_share_every_step_comes_in_order() {
        let plan = Convert {
            referee: Some("ref.example.org".into()),
            second: Some("web2".into()),
            shared: Some("192.0.2.50/24".into()),
            archive: Some("/srv/share/archive".into()),
            ..plan()
        };
        let ids: Vec<&str> = guide(&plan, &Seen::default())
            .steps
            .iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(
            ids,
            [
                "archive-copy",
                "archive-switch",
                "db-open",
                "referee-join",
                "db-adopt",
                "db1-join",
                "db-switch",
                "db2-join",
                "db-retire",
                "second-add",
                "nodes-rejoin",
                "second-join",
                "final-check",
            ]
        );
        // The copy is shown as the rsync to run here; a check that found it
        // complete opens the switch.
        let seen = Seen {
            archive: Some("/srv/archive".into()),
            ..Seen::default()
        };
        let guide = super::guide(&plan, &seen);
        assert!(
            guide.steps[0].run[0].contains("rsync -a"),
            "{:?}",
            guide.steps[0]
        );
        assert!(guide.steps[0].run[0].contains("/srv/archive/ /srv/share/archive/"));
        let checked = Seen {
            asked: vec![(Kind::ArchiveCheck, Some(true))],
            ..seen
        };
        assert_eq!(super::guide(&plan, &checked).steps[1].state, State::Now);
        // A check that found it incomplete, or still runs, does not.
        for ok in [Some(false), None] {
            let seen = Seen {
                archive: Some("/srv/archive".into()),
                asked: vec![(Kind::ArchiveCheck, ok)],
                ..Seen::default()
            };
            assert_eq!(super::guide(&plan, &seen).steps[0].state, State::Now);
        }
        // Moved, opened, the referee joined: the hub's database goes to it.
        let mut moved = layout(&["ref.example.org"], true);
        moved.referee = plan.referee.clone();
        let seen = Seen {
            archive: plan.archive.clone(),
            layout: Some(moved.clone()),
            ..Seen::default()
        };
        let guide = super::guide(&plan, &seen);
        assert_eq!(guide.steps[4].state, State::Now, "{:?}", states(&guide));
        assert_eq!(guide.steps[4].ask.as_ref().unwrap().kind, Kind::DbMove);
        // Retired after the switch (the hub's mark gone): the second is
        // added, then the database machines join again, then the second.
        moved.joined = vec![
            "ref.example.org".into(),
            "db1.example.org".into(),
            "db2.example.org".into(),
        ];
        moved.moving = false;
        moved.retired = true;
        let seen = Seen {
            archive: plan.archive.clone(),
            layout: Some(moved.clone()),
            ..Seen::default()
        };
        assert_eq!(super::guide(&plan, &seen).steps[9].state, State::Now);
        moved.second = Some("web2".into());
        moved.joined = vec!["ref.example.org".into()];
        let seen = Seen {
            archive: plan.archive.clone(),
            layout: Some(moved.clone()),
            ..Seen::default()
        };
        let guide = super::guide(&plan, &seen);
        assert_eq!(guide.steps[10].state, State::Now);
        assert_eq!(
            guide.steps[10].on.as_deref(),
            Some("db1.example.org, then db2.example.org")
        );
        // Every machine in: the final check waits for its checks.
        moved.joined.extend([
            "db1.example.org".into(),
            "db2.example.org".into(),
            "web2".into(),
        ]);
        let mut seen = Seen {
            archive: plan.archive.clone(),
            layout: Some(moved),
            up: vec!["radio-a".into(), "web2".into()],
            standby_current: true,
            referee_answers: Some(false),
            ..Seen::default()
        };
        let guide = super::guide(&plan, &seen);
        assert_eq!(guide.steps[12].state, State::Now);
        assert_eq!(
            guide.checks,
            [
                Check {
                    text: "this machine answers".into(),
                    ok: true
                },
                Check {
                    text: "web2 answers".into(),
                    ok: true
                },
                Check {
                    text: "a database machine is the primary, and the other its standby, in step"
                        .into(),
                    ok: true
                },
                Check {
                    text: "the referee ref.example.org answers".into(),
                    ok: false
                },
            ]
        );
        assert!(!guide.complete);
        seen.referee_answers = Some(true);
        let guide = super::guide(&plan, &seen);
        assert!(guide.complete);
        assert!(guide.steps.iter().all(|s| s.state == State::Done));
    }

    #[test]
    fn the_standby_is_current_within_16_mib_of_a_primary() {
        let at = |behind: u64| {
            serde_json::json!([
                {"host": "db1", "role": "primary", "at": "0/9000000"},
                {"host": "db2", "role": "standby", "behind_bytes": behind},
            ])
        };
        assert!(standby_current(&at(0)));
        assert!(!standby_current(&at(IN_STEP_BYTES)));
        assert!(!standby_current(&serde_json::json!([
            {"host": "db1", "role": "primary"},
            {"host": "db2", "role": null, "error": "down"},
        ])));
        assert!(!standby_current(&serde_json::Value::Null));
    }

    #[test]
    fn the_plan_is_kept_readable_by_all() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("convert.json");
        let plan = Convert {
            referee: Some("ref.example.org".into()),
            archive: Some("/srv/share/archive".into()),
            ..plan()
        };
        plan.write(&path).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert_eq!(Convert::read(&path), Ok(plan));
        let error = Convert::read(&dir.path().join("none.json")).unwrap_err();
        assert!(error.contains("scannerpulse setup"), "{error}");
    }
}
