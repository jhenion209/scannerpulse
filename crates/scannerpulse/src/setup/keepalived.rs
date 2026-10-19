//! The address two ScannerPulse machines share: keepalived holds it on the
//! healthy one (VRRP, sent straight to the other machine), checking the
//! site's `/healthz`. A failing check lowers the machine's priority below
//! the other's, so the address moves to the machine that answers; when
//! neither does, it stays where it is.

use super::steps::{Step, run};
use super::system::{Family, System};

/// keepalived's settings.
pub const CONF: &str = "/etc/keepalived/keepalived.conf";

/// The first machine's priority; the second's is lower, and a failing check
/// takes `WEIGHT` off.
const FIRST: u8 = 150;
const SECOND: u8 = 100;
const WEIGHT: u8 = 60;
// A failing first machine falls below a healthy second; both failing, the
// first keeps the address.
const _: () = assert!(FIRST - WEIGHT < SECOND && FIRST - WEIGHT > SECOND - WEIGHT);

/// The first line of the settings setup writes, by which uninstall knows them.
const HEADER: &str =
    "# Written by `scannerpulse setup`: the address the two ScannerPulse machines share.\n";

/// keepalived's settings are setup's.
pub fn ours(text: &str) -> bool {
    text.starts_with(HEADER)
}

/// keepalived's settings for one of the two machines. `@IFACE@`, `@SRC@`
/// and `@PEER@` (this machine's interface and address towards the other,
/// and the other's address) are filled in when it is set up.
pub fn conf(shared: &str, first: bool, router_id: u8, port: u16) -> String {
    let priority = if first { FIRST } else { SECOND };
    // The second takes the address as soon as the first fails its check;
    // the first takes it back only once healthy for 30 s.
    let delay = if first { "preempt_delay 30\n    " } else { "" };
    format!(
        "{HEADER}global_defs {{\n    script_user root\n    enable_script_security\n}}\n\
         \n\
         vrrp_script scannerpulse_healthy {{\n    \
             script \"/usr/bin/curl -fsk -o /dev/null --max-time 3 https://127.0.0.1:{port}/healthz\"\n    \
             interval 5\n    fall 2\n    rise 2\n    weight -{WEIGHT}\n}}\n\
         \n\
         vrrp_instance scannerpulse {{\n    \
             state BACKUP\n    interface @IFACE@\n    virtual_router_id {router_id}\n    \
             priority {priority}\n    advert_int 1\n    {delay}\
             unicast_src_ip @SRC@\n    unicast_peer {{\n        @PEER@\n    }}\n    \
             virtual_ipaddress {{\n        {shared} dev @IFACE@\n    }}\n    \
             track_script {{\n        scannerpulse_healthy\n    }}\n}}\n"
    )
}

/// The steps that make this machine hold the shared address with the other
/// (`peer`, a name or an address).
pub fn steps(
    system: &System,
    shared: &str,
    peer: &str,
    first: bool,
    router_id: u8,
    port: u16,
) -> Vec<Step> {
    let mut steps = vec![match system.family {
        Family::Apt => run(
            "Install keepalived, for the shared address",
            "apt-get",
            &[
                "-o",
                "DPkg::Lock::Timeout=300",
                "install",
                "-y",
                "keepalived",
                "curl",
            ],
        ),
        Family::Dnf => run(
            "Install keepalived, for the shared address",
            "dnf",
            &["install", "-y", "keepalived", "/usr/bin/curl"],
        ),
    }];
    steps.push(super::node::script(
        "Write keepalived's settings for the shared address",
        r#"peer=$(getent ahostsv4 "$PEER" | awk 'NR == 1 { print $1 }')
[ -n "$peer" ] || { echo "$PEER does not resolve to an IPv4 address here" >&2; exit 1; }
route=$(ip -o route get "$peer") || exit 1
dev=$(printf '%s\n' "$route" | awk '{ for (i = 1; i < NF; i++) if ($i == "dev") print $(i + 1) }')
src=$(printf '%s\n' "$route" | awk '{ for (i = 1; i < NF; i++) if ($i == "src") print $(i + 1) }')
[ -n "$dev" ] && [ -n "$src" ] || { echo "no route to $PEER" >&2; exit 1; }
install -d -m 0755 "$(dirname "$CONF")"
sed -e "s|@IFACE@|$dev|g" -e "s|@SRC@|$src|g" -e "s|@PEER@|$peer|g" > "$CONF.new" && mv "$CONF.new" "$CONF""#,
        &[("PEER", peer), ("CONF", CONF)],
        Some(conf(shared, first, router_id, port)),
    ));
    // firewalld lets VRRP through only when told.
    steps.push(super::node::script(
        "Let VRRP through the firewall, if firewalld runs",
        "systemctl is-active -q firewalld 2>/dev/null || exit 0\nfirewall-cmd -q --permanent --add-rich-rule='rule protocol value=\"vrrp\" accept' && firewall-cmd -q --reload",
        &[],
        None,
    ));
    steps.push(run(
        "Start keepalived, now and at boot",
        "systemctl",
        &["enable", "keepalived"],
    ));
    steps.push(run(
        "Start keepalived with the new settings",
        "systemctl",
        &["restart", "keepalived"],
    ));
    steps
}

/// The steps that let the shared address go to the other machine and take
/// setup's keepalived settings off.
pub fn undo_steps() -> [Step; 2] {
    [
        run(
            "Stop keepalived: the shared address moves to the other machine",
            "systemctl",
            &["disable", "--now", "keepalived"],
        ),
        super::node::script(
            "Remove keepalived's settings and its firewall rule",
            "rm -f \"$CONF\"\nsystemctl is-active -q firewalld 2>/dev/null || exit 0\nfirewall-cmd -q --permanent --remove-rich-rule='rule protocol value=\"vrrp\" accept' && firewall-cmd -q --reload",
            &[("CONF", CONF)],
            None,
        ),
    ]
}

/// The step that stops keepalived and removes its settings and its
/// firewall rule when the settings are setup's: the shared address was
/// dropped. Settings of anyone else's are left alone.
pub fn drop_step() -> Step {
    super::node::script(
        "Stop keepalived and remove setup's settings for it, if any (no shared address)",
        "head -n 1 \"$CONF\" 2>/dev/null | grep -qF \"$HEADER\" || exit 0\nsystemctl disable --now keepalived || exit 1\nrm -f \"$CONF\"\nsystemctl is-active -q firewalld 2>/dev/null || exit 0\nfirewall-cmd -q --permanent --remove-rich-rule='rule protocol value=\"vrrp\" accept' && firewall-cmd -q --reload",
        &[("CONF", CONF), ("HEADER", HEADER.trim_end())],
        None,
    )
}

/// The VRRP router number for a pair, from the two machines' names, which
/// both know.
pub fn router_id(first: &str, second: &str) -> u8 {
    let sum = format!("{first} {second}")
        .bytes()
        .fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(u32::from(b)));
    (sum % 254) as u8 + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup::steps::Action;

    #[test]
    fn the_healthy_machine_holds_the_shared_address() {
        let first = conf("192.0.2.50/24", true, 7, 443);
        assert!(first.contains("priority 150\n"), "{first}");
        assert!(first.contains("weight -60\n"), "{first}");
        assert!(first.contains("https://127.0.0.1:443/healthz"), "{first}");
        assert!(first.contains("virtual_router_id 7\n"), "{first}");
        assert!(first.contains("192.0.2.50/24 dev @IFACE@"), "{first}");
        assert!(
            first.contains("unicast_peer {\n        @PEER@\n"),
            "{first}"
        );
        let second = conf("192.0.2.50/24", false, 7, 8443);
        assert!(second.contains("priority 100\n"), "{second}");
        // The second takes the address as soon as the first fails; the
        // first takes it back only after 30 s of health.
        assert!(first.contains("preempt_delay 30\n"), "{first}");
        assert!(!second.contains("preempt_delay"), "{second}");
        let id = router_id("192.0.2.10", "web2.example.org");
        assert!((1..=254).contains(&id));
        assert_eq!(id, router_id("192.0.2.10", "web2.example.org"));
    }

    #[test]
    fn the_settings_are_filled_in_from_the_route_to_the_other_machine() {
        let system =
            crate::setup::system::detect("ID=debian\nVERSION_ID=\"13\"\n", "x86_64").unwrap();
        let all = steps(&system, "192.0.2.50/24", "web2.example.org", true, 7, 443);
        let titles: Vec<&str> = all.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "Install keepalived, for the shared address",
                "Write keepalived's settings for the shared address",
                "Let VRRP through the firewall, if firewalld runs",
                "Start keepalived, now and at boot",
                "Start keepalived with the new settings",
            ]
        );
    }

    /// Runs a step's script with `stubs` (name, script) first on the path,
    /// returning the calls the stubs logged.
    fn run_script(
        step: &Step,
        stubs: &[(&str, &str)],
        dir: &std::path::Path,
        input: &str,
    ) -> String {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let Action::Run { args, env, .. } = &step.action else {
            panic!()
        };
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        for (name, body) in stubs {
            let path = bin.join(name);
            std::fs::write(
                &path,
                format!("#!/bin/sh\necho \"{name} $*\" >> \"$LOG\"\n{body}\n"),
            )
            .unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let log = dir.join("calls");
        let mut child = std::process::Command::new("/bin/sh")
            .args(&args[..])
            .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("LOG", &log)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        assert!(child.wait().unwrap().success(), "{}", step.title);
        std::fs::read_to_string(&log).unwrap_or_default()
    }

    #[test]
    fn the_other_machine_is_reached_over_ipv4_even_when_its_name_gives_ipv6_first() {
        let dir = tempfile::tempdir().unwrap();
        let conf_path = dir.path().join("keepalived.conf");
        let system =
            crate::setup::system::detect("ID=debian\nVERSION_ID=\"13\"\n", "x86_64").unwrap();
        let mut step = steps(&system, "192.0.2.50/24", "web2.example.org", true, 7, 443).remove(1);
        let Action::Run { env, input, .. } = &mut step.action else {
            panic!()
        };
        let text = input.take().unwrap();
        for (k, v) in env.iter_mut() {
            if k == "CONF" {
                *v = conf_path.display().to_string();
            }
        }
        let stubs = [
            (
                "getent",
                "case \"$1\" in ahostsv4) echo '192.0.2.11 STREAM web2.example.org' ;; *) echo '2001:db8::11 STREAM web2.example.org'; echo '192.0.2.11 STREAM web2.example.org' ;; esac",
            ),
            (
                "ip",
                "case \"$4\" in *:*) echo \"$4 from :: dev eth0 src 2001:db8::10\" ;; *) echo \"$4 dev eth0 src 192.0.2.10 uid 0\" ;; esac",
            ),
        ];
        run_script(&step, &stubs, dir.path(), &text);
        let written = std::fs::read_to_string(&conf_path).unwrap();
        assert!(written.contains("unicast_src_ip 192.0.2.10\n"), "{written}");
        assert!(written.contains("        192.0.2.11\n"), "{written}");
        assert!(written.contains("192.0.2.50/24 dev eth0"), "{written}");
    }

    #[test]
    fn a_dropped_shared_address_stops_setups_keepalived_and_no_other() {
        let stubs = [("systemctl", "exit 0"), ("firewall-cmd", "exit 0")];
        for (ours, text) in [
            (true, conf("192.0.2.50/24", true, 7, 443)),
            (false, "global_defs {\n}\n".to_string()),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let conf_path = dir.path().join("keepalived.conf");
            std::fs::write(&conf_path, &text).unwrap();
            let mut step = drop_step();
            let Action::Run { env, .. } = &mut step.action else {
                panic!()
            };
            for (k, v) in env.iter_mut() {
                if k == "CONF" {
                    *v = conf_path.display().to_string();
                }
            }
            let calls = run_script(&step, &stubs, dir.path(), "");
            assert_eq!(
                calls.contains("systemctl disable --now keepalived"),
                ours,
                "{calls}"
            );
            assert_eq!(conf_path.exists(), !ours);
            assert_eq!(calls.contains("--remove-rich-rule"), ours, "{calls}");
        }
    }

    #[test]
    fn on_the_red_hat_family_curl_is_asked_for_by_its_file() {
        // curl-minimal, in Rocky's and Alma's minimal images, conflicts with
        // the package curl but gives /usr/bin/curl, all the check needs.
        let rocky =
            crate::setup::system::detect("ID=\"rocky\"\nVERSION_ID=\"9.6\"\n", "x86_64").unwrap();
        let first = steps(&rocky, "192.0.2.50/24", "web2.example.org", true, 7, 443).remove(0);
        let second = crate::setup::second::steps(
            &crate::setup::second::tests::plan(),
            &crate::setup::second::tests::files(),
            &rocky,
            std::path::Path::new("/b"),
            "web2.example.org",
        )
        .unwrap();
        for step in [&first, &second[0]] {
            let Action::Run { program, args, .. } = &step.action else {
                panic!()
            };
            assert_eq!(program, "dnf");
            assert!(args.iter().any(|a| a == "/usr/bin/curl"), "{args:?}");
            assert!(!args.iter().any(|a| a == "curl"), "{args:?}");
        }
    }
}
