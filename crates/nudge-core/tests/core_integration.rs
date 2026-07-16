//! End-to-end over pure core: rules text -> schedule context -> transitions.

use nudge_core::rules::parse;
use nudge_core::schedule::{context, LocalNow};
use nudge_core::state::{next, Effect, Event, State};

const RULES: &str = include_str!("../../../rules.example.toml");

fn at(weekday: u8, minutes: u32) -> LocalNow {
    LocalNow {
        unix: (weekday as i64) * 86_400 + (minutes as i64) * 60,
        weekday,
        minutes,
    }
}

#[test]
fn example_rules_full_day_cycle() {
    let rules = parse(RULES).expect("rules.example.toml must stay valid");

    // Monday 07:00 — idle, timer armed for 08:30 start.
    let ctx = context(&rules, at(0, 7 * 60));
    let (s, fx) = next(State::Idle, &Event::EdgeTimer(ctx.next_edge.unwrap().at - 5400), &ctx);
    assert_eq!(s, State::Idle);
    assert!(matches!(fx.as_slice(), [Effect::ArmEdgeTimer { .. }]));

    // 08:30 edge fires — window opens, prompting begins with window text.
    let ctx = context(&rules, at(0, 8 * 60 + 30));
    let (s, fx) = next(State::Idle, &Event::EdgeTimer(0), &ctx);
    assert!(matches!(s, State::Prompting { .. }));
    assert!(fx.iter().any(|f| matches!(f, Effect::ShowPrompt { text, .. } if text.contains("Deep work"))));

    // User starts the task — prompt hidden, outcome logged, task Started.
    let (s, fx) = next(s, &Event::Ack(60), &ctx);
    // Sampling and check-ins are both off in rules.example.toml, so Started
    // carries neither of those edges; the §6.4 on-task check-in defaults ON
    // (30-min floor), so its tick is armed one cadence out from the Ack.
    assert_eq!(
        s,
        State::Started {
            checkin_at: None,
            sample_at: None,
            off_task_since: None,
            ontask_at: Some(60 + 1800),
        }
    );
    assert!(fx.contains(&Effect::HidePrompt));

    // 12:00 edge — window closes, back to idle.
    let ctx = context(&rules, at(0, 12 * 60));
    let (s, fx) = next(s, &Event::EdgeTimer(0), &ctx);
    assert_eq!(s, State::Idle);
    assert!(fx.contains(&Effect::HidePrompt));
}
