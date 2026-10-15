use std::time::{Duration, Instant};

use scannerpulse::servers::{self, Servers};
use scannerpulse::settings::Server;

fn server(url: &str, weight: u32) -> Server {
    Server {
        url: url.to_string(),
        weight,
    }
}

#[test]
fn the_cooldown_doubles_for_each_round_up_to_ten_minutes() {
    let secs: Vec<u64> = (1..=7).map(|n| servers::cooldown(n).as_secs()).collect();
    assert_eq!(secs, [30, 60, 120, 240, 480, 600, 600]);
    assert_eq!(servers::cooldown(1000).as_secs(), 600);
}

#[test]
fn each_server_takes_as_many_requests_as_its_weight_in_turn() {
    let mut s = Servers::new(&[server("a", 2), server("b", 1)]);
    let now = Instant::now();
    assert_eq!(s.free(now), 3);
    let taken: Vec<String> = (0..4).filter_map(|_| s.take(now)).collect();
    assert_eq!(taken, ["a", "b", "a"]);
    assert_eq!(s.free(now), 0);
    s.release("b");
    assert_eq!(s.take(now).as_deref(), Some("b"));
}

#[test]
fn a_failed_server_cools_down_and_its_first_success_ends_it() {
    let mut s = Servers::new(&[server("a", 2), server("b", 1)]);
    let now = Instant::now();
    assert!(s.failed("a", "refused", now));
    // A second failure of the same round does not start another.
    assert!(!s.failed("a", "refused again", now + Duration::from_secs(1)));
    assert_eq!(s.free(now), 1);
    assert_eq!(s.take(now).as_deref(), Some("b"));
    assert_eq!(s.take(now), None);
    let state = &s.states(now)[0];
    assert_eq!(state.failed_rounds, 1);
    assert_eq!(state.cooling_for, Some(Duration::from_secs(30)));
    assert_eq!(state.last_error.as_deref(), Some("refused again"));
    // After the cooldown a new failure is the next round: 60 s.
    let later = now + Duration::from_secs(31);
    assert!(s.failed("a", "refused", later));
    assert_eq!(
        s.states(later)[0].cooling_for,
        Some(Duration::from_secs(60))
    );
    assert!(s.succeeded("a"));
    assert_eq!(s.states(later)[0].cooling_for, None);
    assert_eq!(s.states(later)[0].failed_rounds, 0);
    assert!(!s.succeeded("a"));
}

#[test]
fn a_new_list_keeps_the_state_of_the_servers_that_stay() {
    let mut s = Servers::new(&[server("a", 1), server("b", 1)]);
    let now = Instant::now();
    s.failed("a", "refused", now);
    s.update(&[server("a", 4), server("c", 1)]);
    let states = s.states(now);
    let urls: Vec<&str> = states.iter().map(|x| x.url.as_str()).collect();
    assert_eq!(urls, ["a", "c"]);
    assert_eq!(states[0].failed_rounds, 1);
    assert_eq!(s.free(now), 1);
}
