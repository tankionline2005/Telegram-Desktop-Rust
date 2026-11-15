//! Texts the UI shows for times, media, service messages and statuses, in tdesktop's
//! wording (`Telegram/Resources/langs/lang.strings`, English) and with its rules
//! (`Ui::FormatDialogsDate`, `Data::OnlineText`, `HistoryItem::notificationText`).
//! Localization comes later; until then these are the English strings.

use chrono::{DateTime, Datelike, Local, TimeZone};
use tdr_data::describe::{Lastseen, Media, Service};

fn local(date: i32) -> Option<DateTime<Local>> {
    Local.timestamp_opt(i64::from(date), 0).single()
}

/// `Ui::FormatDialogsDate`: the time for anything from today or the last 20 hours, the
/// weekday within a week, the date otherwise.
pub fn dialogs_date(date: i32, now: DateTime<Local>) -> String {
    let Some(t) = local(date) else {
        return String::new();
    };
    let recent = (now - t).num_seconds().abs() < 20 * 3600;
    if t.date_naive() == now.date_naive() || recent {
        t.format("%H:%M").to_string()
    } else if (now.date_naive() - t.date_naive()).num_days().abs() < 7 {
        t.format("%a").to_string()
    } else {
        t.format("%d.%m.%y").to_string()
    }
}

/// Message time in a bubble.
pub fn message_time(date: i32) -> String {
    local(date)
        .map(|t| t.format("%H:%M").to_string())
        .unwrap_or_default()
}

/// The local calendar day of a message (for date dividers).
pub fn day_of(date: i32) -> Option<chrono::NaiveDate> {
    local(date).map(|t| t.date_naive())
}

/// Date divider text (`langDayOfMonthFull`): "November 13", with the year when it is
/// not the current one.
pub fn day_divider(day: chrono::NaiveDate, today: chrono::NaiveDate) -> String {
    if day.year() == today.year() {
        day.format("%B %-d").to_string()
    } else {
        day.format("%B %-d, %Y").to_string()
    }
}

/// `lng_in_dlg_*`: how the chat list (and a bubble, until media is drawn) names media.
pub fn media_label(media: &Media) -> String {
    let or = |text: &str, fallback: &str| {
        if text.is_empty() {
            fallback.to_string()
        } else {
            text.to_string()
        }
    };
    match media {
        Media::Photo => "Photo".into(),
        Media::Video => "Video".into(),
        Media::Gif => "GIF".into(),
        Media::VoiceMessage => "Voice message".into(),
        Media::VideoMessage => "Video message".into(),
        Media::Audio(title) => or(title, "Audio file"),
        Media::Sticker(emoji) => {
            if emoji.is_empty() {
                "Sticker".into()
            } else {
                format!("{emoji} Sticker")
            }
        }
        Media::File(name) => or(name, "File"),
        Media::Contact => "Contact".into(),
        Media::Location => "Location".into(),
        Media::LiveLocation => "Live location".into(),
        Media::Venue(title) => or(title, "Location"),
        Media::Poll(question) => or(question, "Poll"),
        Media::Dice(emoji) => emoji.clone(),
        Media::Game(title) => or(title, "Game"),
        Media::Invoice(title) => or(title, "Invoice"),
        Media::Story => "Story".into(),
        Media::Giveaway => "Giveaway".into(),
        Media::Checklist => "Checklist".into(),
        Media::Other => "Unsupported message".into(),
    }
}

/// "A", "A and B", "A, B and C" (`lng_action_add_users_and_last`).
fn join_names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// `lng_action_*`: the text of a service message. `name(id)` resolves user names.
pub fn service_text(
    service: &Service,
    from: &str,
    from_id: Option<i64>,
    out: bool,
    is_channel: bool,
    name: impl Fn(i64) -> String,
) -> String {
    match service {
        Service::ChatCreate { title } => format!("{from} created the group «{title}»"),
        Service::ChannelCreate => "Channel created".into(),
        Service::EditTitle { title } if is_channel => format!("Channel renamed to «{title}»"),
        Service::EditTitle { title } => format!("{from} changed group name to «{title}»"),
        Service::EditPhoto if is_channel => "Channel photo updated".into(),
        Service::EditPhoto => format!("{from} updated group photo"),
        Service::DeletePhoto if is_channel => "Channel photo removed".into(),
        Service::DeletePhoto => format!("{from} removed group photo"),
        Service::AddUsers { users } if users.len() == 1 && Some(users[0]) == from_id => {
            format!("{from} joined the group")
        }
        Service::AddUsers { users } => {
            let names: Vec<String> = users.iter().map(|&id| name(id)).collect();
            format!("{from} added {}", join_names(&names))
        }
        Service::DeleteUser { user } if Some(*user) == from_id => format!("{from} left the group"),
        Service::DeleteUser { user } => format!("{from} removed {}", name(*user)),
        Service::JoinedByLink => format!("{from} joined the group via invite link"),
        Service::JoinedByRequest => format!("{from} was accepted to the group"),
        Service::Migrated => "The group was upgraded to a supergroup".into(),
        Service::PinMessage => format!("{from} pinned a message"),
        Service::HistoryClear => "History was cleared".into(),
        Service::Call {
            video,
            missed,
            duration,
        } => {
            let kind = match (*missed, out, *video) {
                (true, true, false) => "Cancelled call",
                (true, true, true) => "Cancelled video call",
                (true, false, false) => "Missed call",
                (true, false, true) => "Missed video call",
                (false, true, false) => "Outgoing call",
                (false, true, true) => "Outgoing video call",
                (false, false, false) => "Incoming call",
                (false, false, true) => "Incoming video call",
            };
            match duration.filter(|d| *d > 0 && !*missed) {
                Some(d) if d >= 60 => format!("{kind} ({} min)", d / 60),
                Some(d) => format!("{kind} ({d} sec)"),
                None => kind.into(),
            }
        }
        Service::GroupCall { ended: true } => "Video chat ended".into(),
        Service::GroupCall { ended: false } if is_channel => "Live stream started".into(),
        Service::GroupCall { ended: false } => format!("{from} started a video chat"),
        Service::ScreenshotTaken => format!("{from} took a screenshot!"),
        Service::ContactSignUp => format!("{from} joined Telegram"),
        Service::Other => "Service message".into(),
    }
}

/// Calls are bubbles in tdesktop, not service lines.
pub fn is_call(service: &Service) -> bool {
    matches!(service, Service::Call { .. })
}

/// `Data::OnlineText`.
pub fn lastseen_text(status: Lastseen, now: DateTime<Local>) -> String {
    let was = match status {
        Lastseen::Online { until } if i64::from(until) > now.timestamp() => return "online".into(),
        Lastseen::Online { until } => until,
        Lastseen::Offline { was } => was,
        Lastseen::Recently => return "last seen recently".into(),
        Lastseen::WithinWeek => return "last seen within a week".into(),
        Lastseen::WithinMonth => return "last seen within a month".into(),
        Lastseen::LongAgo => return "last seen a long time ago".into(),
    };
    let Some(t) = local(was) else {
        return "last seen a long time ago".into();
    };
    let ago = (now - t).num_seconds().max(0);
    if ago < 60 {
        "last seen just now".into()
    } else if ago < 3600 {
        let m = ago / 60;
        format!("last seen {m} minute{} ago", if m == 1 { "" } else { "s" })
    } else if ago < 12 * 3600 {
        let h = ago / 3600;
        format!("last seen {h} hour{} ago", if h == 1 { "" } else { "s" })
    } else if t.date_naive() == now.date_naive() {
        format!("last seen today at {}", t.format("%H:%M"))
    } else if now.date_naive().pred_opt() == Some(t.date_naive()) {
        format!("last seen yesterday at {}", t.format("%H:%M"))
    } else {
        format!("last seen {}", t.format("%d.%m.%y"))
    }
}

/// "1 member", "1,234 members" (`lng_chat_status_members` with `count_decimal`:
/// grouped digits, `%L1`).
pub fn count_text(count: i32, one: &str, many: &str) -> String {
    let digits = count.to_string();
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("{grouped} {}", if count == 1 { one } else { many })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, mo, d, h, mi, 0).single().unwrap()
    }

    #[test]
    fn dialogs_date_like_tdesktop() {
        let now = at(2026, 11, 15, 2, 11);
        // Yesterday 23:27 is within 20 hours: a time, not "Sat".
        assert_eq!(
            dialogs_date(at(2026, 11, 14, 23, 27).timestamp() as i32, now),
            "23:27"
        );
        // 21 hours ago, yesterday: the weekday.
        assert_eq!(
            dialogs_date(at(2026, 11, 14, 5, 0).timestamp() as i32, now),
            "Sat"
        );
        assert_eq!(
            dialogs_date(at(2026, 11, 1, 5, 0).timestamp() as i32, now),
            "01.11.26"
        );
    }

    #[test]
    fn dividers_and_counts() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 11, 15).unwrap();
        let day = chrono::NaiveDate::from_ymd_opt(2026, 11, 3).unwrap();
        assert_eq!(day_divider(day, today), "November 3");
        let old = chrono::NaiveDate::from_ymd_opt(2025, 1, 9).unwrap();
        assert_eq!(day_divider(old, today), "January 9, 2025");
        assert_eq!(count_text(906, "member", "members"), "906 members");
        assert_eq!(count_text(1, "member", "members"), "1 member");
        assert_eq!(count_text(1234, "member", "members"), "1,234 members");
        assert_eq!(
            count_text(25_400, "subscriber", "subscribers"),
            "25,400 subscribers"
        );
    }

    #[test]
    fn services_and_statuses() {
        let names = |id: i64| format!("U{id}");
        let add = Service::AddUsers {
            users: vec![2, 3, 4],
        };
        assert_eq!(
            service_text(&add, "Ann", Some(1), false, false, names),
            "Ann added U2, U3 and U4"
        );
        let join = Service::AddUsers { users: vec![1] };
        assert_eq!(
            service_text(&join, "Ann", Some(1), false, false, names),
            "Ann joined the group"
        );
        let now = at(2026, 11, 15, 12, 0);
        let five_min = now.timestamp() as i32 - 300;
        assert_eq!(
            lastseen_text(Lastseen::Offline { was: five_min }, now),
            "last seen 5 minutes ago"
        );
        assert_eq!(lastseen_text(Lastseen::Recently, now), "last seen recently");
        let future = now.timestamp() as i32 + 60;
        assert_eq!(
            lastseen_text(Lastseen::Online { until: future }, now),
            "online"
        );
    }
}
