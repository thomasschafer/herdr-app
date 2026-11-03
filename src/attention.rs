//! How an agent's alert reaches the person. In front, a toast; with Herdr in
//! the background, a system notification (once per agent: a newer one
//! replaces it, and it goes once they look or the agent moves on) and, when
//! the agent needs them, one bounce of the Dock icon. The Dock badge counts
//! the agents that need them or finished unseen, whatever is in front.

use crate::model::Alert;
use crate::settings::Notifications;
use gpui::{App, SharedString, SystemNotification};

#[cfg(target_os = "macos")]
mod macos;

/// What one alert does. An alert for an agent the person is looking at does
/// none of these (its sound still plays).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Delivery {
    pub toast: bool,
    pub system: bool,
    pub bounce: bool,
}

/// Whether system notifications can show here, else alerts stay toasts rather
/// than vanish: macOS, from the app bundle (GPUI posts nothing for a bare
/// binary, as `cargo run` starts). GPUI's Linux and Windows notifications need
/// an app identity Herdr doesn't set. Tests post to GPUI's test platform.
pub fn system_notifications() -> bool {
    #[cfg(target_os = "macos")]
    return cfg!(test) || macos::bundled();
    #[cfg(not(target_os = "macos"))]
    return false;
}

pub fn delivery(alert: Alert, looking: bool, foreground: bool, settings: &Notifications) -> Delivery {
    delivery_on(system_notifications(), alert, looking, foreground, settings)
}

fn delivery_on(supported: bool, alert: Alert, looking: bool, foreground: bool, settings: &Notifications) -> Delivery {
    if looking {
        return Delivery::default();
    }
    if foreground {
        return Delivery { toast: true, ..Delivery::default() };
    }
    let system = supported && settings.system;
    Delivery { toast: !system, system, bounce: supported && settings.bounce && alert == Alert::Blocked }
}

/// Whether a posted notification is old news: the person is looking at the
/// agent, or it no longer has the status it was posted for (it went back to
/// work, was seen elsewhere, or is gone).
pub fn stale(alert: Alert, status: Option<&str>, looking: bool) -> bool {
    let posted_for = match alert {
        Alert::Blocked => "blocked",
        Alert::Done => "done",
    };
    looking || status != Some(posted_for)
}

/// The Dock badge for a number of agents wanting attention: none at zero.
pub fn badge_label(count: usize) -> Option<String> {
    (count > 0).then(|| count.to_string())
}

const AGENT_TAG: &str = "herdr.agent:";
const TEST_TAG: &str = "herdr.test";

/// One notification per agent: posting again replaces it.
pub fn agent_tag(address: &str) -> String {
    format!("{AGENT_TAG}{address}")
}

pub fn agent_notification(address: &str, title: &str, body: &str) -> SystemNotification {
    SystemNotification { tag: agent_tag(address).into(), title: title.to_owned().into(), body: body.to_owned().into(), actions: vec![] }
}

/// `app.test_notification`: posted whatever is in front, so the person sees
/// that notifications arrive (macOS asks for permission on the first one).
pub fn test_notification() -> SystemNotification {
    SystemNotification {
        tag: SharedString::new_static(TEST_TAG),
        title: "Herdr notifications work".into(),
        body: "Agents that need you or finish while Herdr is in the background show up here.".into(),
        actions: vec![],
    }
}

/// Clicking a notification brings Herdr forward; an agent's goes to that agent.
pub fn init(cx: &mut App) {
    cx.on_system_notification_response(|response, cx| {
        cx.activate(true);
        if let Some(address) = response.tag.strip_prefix(AGENT_TAG) {
            let args = serde_json::json!({ "terminal": address });
            crate::mods::request(crate::mods::Request::Run { name: "agent.focus".into(), args });
        } else if response.tag != TEST_TAG {
            tracing::warn!("a notification Herdr didn't post was clicked: {}", response.tag);
        }
    });
}

/// Shows `label` on the Dock icon, or nothing. Only macOS has a Dock badge.
pub fn set_badge(label: Option<&str>) {
    #[cfg(target_os = "macos")]
    macos::set_badge(label);
    #[cfg(not(target_os = "macos"))]
    let _ = label;
}

#[cfg(test)]
mod tests {
    use super::*;

    const ON: Notifications = Notifications { system: true, bounce: true };

    #[test]
    fn in_front_a_toast_behind_a_notification_and_a_bounce_only_for_needing_you() {
        let d = |alert, looking, foreground, settings: &Notifications| delivery_on(true, alert, looking, foreground, settings);
        let toast = Delivery { toast: true, ..Delivery::default() };
        for alert in [Alert::Blocked, Alert::Done] {
            assert_eq!(d(alert, true, true, &ON), Delivery::default(), "looking at it");
            assert_eq!(d(alert, false, true, &ON), toast, "in front, elsewhere");
        }
        assert_eq!(d(Alert::Blocked, false, false, &ON), Delivery { system: true, bounce: true, toast: false });
        assert_eq!(d(Alert::Done, false, false, &ON), Delivery { system: true, ..Delivery::default() });
        let quiet = Notifications { system: false, bounce: false };
        assert_eq!(d(Alert::Blocked, false, false, &quiet), toast, "without notifications, the toast waits in the window");
        let bounce_only = Notifications { system: false, bounce: true };
        assert_eq!(d(Alert::Blocked, false, false, &bounce_only), Delivery { toast: true, bounce: true, system: false });
        assert_eq!(delivery_on(false, Alert::Blocked, false, false, &ON), toast, "where the system can't show them");
    }

    #[test]
    fn a_notification_goes_once_looked_at_or_moved_on() {
        assert!(!stale(Alert::Blocked, Some("blocked"), false));
        assert!(stale(Alert::Blocked, Some("blocked"), true), "looked at, still blocked");
        assert!(stale(Alert::Blocked, Some("working"), false));
        assert!(!stale(Alert::Done, Some("done"), false));
        assert!(stale(Alert::Done, Some("idle"), false), "seen on another client");
        assert!(stale(Alert::Done, Some("blocked"), false), "a newer alert posts its own");
        assert!(stale(Alert::Done, None, false), "gone");
    }

    #[test]
    fn the_badge_counts_and_clears_at_zero() {
        assert_eq!(badge_label(0), None);
        assert_eq!(badge_label(3).as_deref(), Some("3"));
    }

    #[test]
    fn agent_tags_round_trip_remote_addresses() {
        let tag = agent_tag("vps/t7");
        assert_eq!(tag.strip_prefix(AGENT_TAG), Some("vps/t7"));
        assert_ne!(tag, TEST_TAG);
    }
}
