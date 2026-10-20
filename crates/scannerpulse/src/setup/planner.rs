//! Setup run again on an install on one machine: the planner of its
//! conversion to high availability. It asks the choices a new install's
//! Advanced setup asks (the database machines, the referee, the second
//! ScannerPulse machine and the address the two share) and the archive's
//! share, and saves them (`convert::PATH`); nothing else changes. The admin
//! page's guide takes the install there, step by step.

use std::path::Path;

use crate::convert::Convert;

/// This machine runs a site on its own, its database setup's own here: an
/// install whose conversion setup plans.
pub fn standalone() -> bool {
    super::web_running()
        && !Path::new(super::plan::PLAN).exists()
        && std::fs::read_to_string(super::steps::env_path())
            .is_ok_and(|settings| super::db_move::database_here(&settings).is_ok())
}

/// `plan` checked before it is saved: its machines each a machine of their
/// own, this one (`plan.hub`) among them, and the archive's new folder a
/// share apart from the archive now (`archive`), or none when it is that
/// archive. With a second ScannerPulse machine, the archive must end on a
/// share; with Let's Encrypt over the web (`le_web`), no second machine.
pub fn checked(
    mut plan: Convert,
    archive: &Path,
    le_web: bool,
    resolve: impl Fn(&str) -> Vec<std::net::IpAddr> + Copy,
) -> Result<Convert, String> {
    super::db_move::machines_apart(&plan.nodes, &plan.hub, plan.referee.as_deref(), resolve)?;
    super::ask::check_machines(
        &plan.nodes,
        plan.referee.as_deref(),
        &plan.hub,
        plan.second.as_deref(),
        plan.shared.as_deref(),
        le_web,
        resolve,
    )?;
    plan.archive = match plan.archive.take() {
        Some(new) => {
            super::ask::check_share(&new)?;
            let new = Path::new(&new)
                .canonicalize()
                .map_err(|error| format!("{new}: {error}"))?;
            if new == archive {
                None
            } else if new.starts_with(archive) || archive.starts_with(&new) {
                return Err(
                    "the archive's new folder may not be inside the archive, nor the archive inside it".into(),
                );
            } else {
                Some(new.display().to_string())
            }
        }
        None => {
            if plan.second.is_some()
                && super::ask::check_share(&archive.display().to_string()).is_err()
            {
                return Err(format!(
                    "a second ScannerPulse machine reaches the archive on a network share, and {} is not one: give its new folder on a share (ARCHIVE_SHARE)",
                    archive.display()
                ));
            }
            None
        }
    };
    Ok(plan)
}

/// What the planner saves, shown before it does.
pub fn summary(plan: &Convert, archive: &Path) -> String {
    let [db1, db2] = &plan.nodes;
    format!(
        "\nThe conversion's plan:\n\
         \n  Database machines        {db1}, the primary once the database has moved; {db2}, its standby\
         \n  This machine's address   {}\
         \n  Failover                 {}\
         \n  Second machine           {}\
         \n  Archive                  {}\
         \n\nSetup saves it in {} and changes nothing else. The admin page's Layout area then shows each step: what it does, what to run on which machine, and the changes on this machine, which its root helper makes once an admin approves them.\n",
        plan.hub,
        match &plan.referee {
            Some(referee) => format!("by the referee {referee}"),
            None => "by hand (`scannerpulse db promote` on the standby)".into(),
        },
        match (&plan.second, &plan.shared) {
            (Some(second), Some(shared)) => format!("{second}, sharing {shared} with this one"),
            (Some(second), None) => second.clone(),
            (None, _) => "none".into(),
        },
        match &plan.archive {
            Some(new) => format!("moves from {} to {new}", archive.display()),
            None => format!("stays at {}", archive.display()),
        },
        crate::convert::PATH,
    )
}

/// Plans this machine's conversion, from the questions or `answers_file`
/// (CONVERT=yes); false when the file is not a conversion's, which setup
/// then refuses as on any machine set up already.
pub async fn run(
    answers_file: Option<&Path>,
    defaults: &super::ask::Defaults,
) -> Result<bool, String> {
    let path = Path::new(crate::convert::PATH);
    let from_file = match answers_file {
        Some(file) => match super::ask::convert_from_file(file, defaults)? {
            Some(plan) => Some(plan),
            None => return Ok(false),
        },
        None => None,
    };
    let (_, archive, _) = super::uninstall::folders().await.unwrap_or_default();
    let archive = archive.ok_or("the database names no archive, or does not answer")?;
    let settings = std::fs::read_to_string(super::steps::env_path())
        .map_err(|error| format!("{}: {error}", super::steps::env_path().display()))?;
    let le_web = crate::hostfile::parse(&settings)?
        .iter()
        .any(|(name, value)| name == "SCANNERPULSE_ACME" && value == "web");
    if let Some(plan) = from_file {
        let plan = checked(plan, &archive, le_web, super::plan::resolve)?;
        print!("{}", summary(&plan, &archive));
        plan.write(path)?;
        println!("Saved.");
        return Ok(true);
    }
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        return Err("setup asks its questions on a terminal: run it in one, or give the answers with --answers FILE (CONVERT=yes)".into());
    }
    let had = Convert::read(path).ok();
    println!(
        "\nThis machine runs a ScannerPulse site on its own. Setup can plan its conversion to high availability: two database machines (a primary and its standby), a referee if you like, a second ScannerPulse machine with an address the two share, and the archive on a network share. Setup only saves the plan; nothing changes here until an admin approves each step in the admin page."
    );
    const PLAN: &str = "Plan the conversion to high availability";
    const CHANGE: &str = "Change the conversion's plan";
    const DROP: &str = "Drop the conversion's plan";
    const LEAVE: &str = "Leave setup";
    let choices = if had.is_some() {
        vec![CHANGE, DROP, LEAVE]
    } else {
        vec![PLAN, LEAVE]
    };
    let chosen = inquire::Select::new("This machine:", choices)
        .prompt()
        .unwrap_or(LEAVE);
    if chosen == LEAVE {
        println!("Setup left; nothing was changed.");
        return Ok(true);
    }
    if chosen == DROP {
        std::fs::remove_file(path).map_err(|error| format!("{}: {error}", path.display()))?;
        println!("The conversion's plan is dropped: the admin page shows no guide.");
        return Ok(true);
    }
    let mut previous = had;
    loop {
        let Some(plan) = super::ask::ask_convert(previous.as_ref(), defaults, &archive)? else {
            println!("Setup left; nothing was changed.");
            return Ok(true);
        };
        previous = Some(plan.clone());
        let plan = match checked(plan, &archive, le_web, super::plan::resolve) {
            Ok(plan) => plan,
            Err(why) => {
                println!("\n{why}\nChange the answers, or quit.");
                continue;
            }
        };
        print!("{}", summary(&plan, &archive));
        match super::ask::next()? {
            super::ask::Next::Go => {
                plan.write(path)?;
                println!(
                    "Saved. Open the admin page's Layout area: it shows the first step. Run setup again here to change the plan before the database starts to move."
                );
                return Ok(true);
            }
            super::ask::Next::Quit => {
                println!("Nothing was changed.");
                return Ok(true);
            }
            super::ask::Next::Change => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

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

    fn resolve(name: &str) -> Vec<IpAddr> {
        match name {
            "db1.example.org" => vec!["192.0.2.21".parse().unwrap()],
            "db2.example.org" => vec!["192.0.2.22".parse().unwrap()],
            "web2.example.org" => vec!["192.0.2.12".parse().unwrap()],
            other => other.parse().into_iter().collect(),
        }
    }

    #[test]
    fn the_machines_are_each_their_own_and_this_one_too() {
        let archive = Path::new("/srv/archive");
        assert_eq!(checked(plan(), archive, false, resolve), Ok(plan()));
        let same = Convert {
            nodes: ["db1.example.org".into(), "192.0.2.21".into()],
            ..plan()
        };
        let error = checked(same, archive, false, resolve).unwrap_err();
        assert!(error.contains("the same machine"), "{error}");
        let here = Convert {
            hub: "192.0.2.22".into(),
            ..plan()
        };
        let error = checked(here, archive, false, resolve).unwrap_err();
        assert!(error.contains("the same machine"), "{error}");
        let web = Convert {
            second: Some("web2.example.org".into()),
            archive: Some("/srv/share".into()),
            ..plan()
        };
        let error = checked(web, archive, true, resolve).unwrap_err();
        assert!(error.contains("Let's Encrypt"), "{error}");
    }

    #[test]
    fn a_second_machine_needs_the_archive_on_a_share() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().canonicalize().unwrap();
        let second = Convert {
            second: Some("web2.example.org".into()),
            ..plan()
        };
        let error = checked(second.clone(), &archive, false, resolve).unwrap_err();
        assert!(error.contains("ARCHIVE_SHARE"), "{error}");
        // A folder of this machine's own is no share.
        let local = dir.path().join("local");
        std::fs::create_dir(&local).unwrap();
        let error = checked(
            Convert {
                archive: Some(local.display().to_string()),
                ..second
            },
            Path::new("/srv/archive"),
            false,
            resolve,
        )
        .unwrap_err();
        assert!(error.contains("not a network share"), "{error}");
    }

    #[test]
    fn the_summary_says_what_is_saved_and_that_nothing_else_changes() {
        let text = summary(
            &Convert {
                referee: Some("ref.example.org".into()),
                second: Some("web2.example.org".into()),
                shared: Some("192.0.2.50/24".into()),
                archive: Some("/srv/share/archive".into()),
                ..plan()
            },
            Path::new("/srv/archive"),
        );
        assert!(text.contains("db1.example.org, the primary once the database has moved; db2.example.org, its standby"), "{text}");
        assert!(text.contains("by the referee ref.example.org"), "{text}");
        assert!(
            text.contains("web2.example.org, sharing 192.0.2.50/24 with this one"),
            "{text}"
        );
        assert!(
            text.contains("moves from /srv/archive to /srv/share/archive"),
            "{text}"
        );
        assert!(
            text.contains("/etc/scannerpulse/convert.json and changes nothing else"),
            "{text}"
        );
        let text = summary(&plan(), Path::new("/srv/archive"));
        assert!(text.contains("by hand"), "{text}");
        assert!(text.contains("stays at /srv/archive"), "{text}");
    }
}
