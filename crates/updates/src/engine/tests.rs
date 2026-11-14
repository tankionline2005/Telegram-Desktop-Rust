use super::*;
use tdr_tl::enums::updates as eu;
use tdr_tl::types::updates as tu;

fn t0() -> Instant {
    Instant::now()
}

fn delete(pts: i32, count: i32) -> enums::Update {
    types::UpdateDeleteMessages {
        messages: vec![pts],
        pts,
        pts_count: count,
    }
    .into()
}

fn channel_delete(channel_id: i64, pts: i32, count: i32) -> enums::Update {
    types::UpdateDeleteChannelMessages {
        channel_id,
        messages: vec![pts],
        pts,
        pts_count: count,
    }
    .into()
}

fn bot_stopped(qts: i32) -> enums::Update {
    types::UpdateBotStopped {
        user_id: 1,
        date: 0,
        stopped: true,
        qts,
    }
    .into()
}

fn short(u: enums::Update) -> enums::Updates {
    types::UpdateShort {
        update: u,
        date: 100,
    }
    .into()
}

fn container(seq_start: i32, seq: i32, updates: Vec<enums::Update>) -> enums::Updates {
    if seq_start == seq {
        types::Updates {
            updates,
            users: vec![],
            chats: vec![],
            date: 100,
            seq,
        }
        .into()
    } else {
        types::UpdatesCombined {
            updates,
            users: vec![],
            chats: vec![],
            date: 100,
            seq_start,
            seq,
        }
        .into()
    }
}

/// The pts values of applied `updateDeleteMessages`, in order.
fn applied(out: &Output) -> Vec<i32> {
    out.items
        .iter()
        .filter_map(|i| match i {
            Item::Update(enums::Update::DeleteMessages(d)) => Some(d.pts),
            Item::Update(enums::Update::DeleteChannelMessages(d)) => Some(d.pts),
            Item::Update(enums::Update::BotStopped(d)) => Some(d.qts),
            _ => None,
        })
        .collect()
}

fn state(pts: i32, qts: i32, seq: i32) -> State {
    State {
        pts,
        qts,
        date: 1,
        seq,
    }
}

fn engine(pts: i32) -> Engine {
    Engine::new(Some(state(pts, 0, 0)), [])
}

#[test]
fn applies_in_order_and_drops_duplicates() {
    let mut e = engine(10);
    let now = t0();
    assert_eq!(applied(&e.feed(short(delete(11, 1)), now)), [11]);
    assert_eq!(applied(&e.feed(short(delete(13, 2)), now)), [13]);
    assert_eq!(
        applied(&e.feed(short(delete(12, 1)), now)),
        Vec::<i32>::new(),
        "duplicate"
    );
    assert_eq!(e.state().pts, 13);
    assert!(e.next_deadline().is_none());
}

#[test]
fn zero_count_updates_apply_at_the_same_pts() {
    let mut e = engine(10);
    assert_eq!(applied(&e.feed(short(delete(10, 0)), t0())), [10]);
    assert_eq!(e.state().pts, 10);
}

#[test]
fn waits_for_a_missing_update_then_reorders() {
    let mut e = engine(10);
    let now = t0();
    // 12 arrives before 11: held back.
    assert!(applied(&e.feed(short(delete(12, 1)), now)).is_empty());
    assert!(e.next_deadline().is_some());
    // 11 arrives within the timeout: both applied, in order.
    assert_eq!(applied(&e.feed(short(delete(11, 1)), now)), [11, 12]);
    assert!(e.next_deadline().is_none());
    assert!(e.poll(now + GAP_TIMEOUT * 2).actions.is_empty());
}

#[test]
fn unresolved_gap_requests_difference_after_timeout() {
    let mut e = engine(10);
    let now = t0();
    e.feed(short(delete(15, 1)), now);
    assert!(
        e.poll(now + GAP_TIMEOUT / 2).actions.is_empty(),
        "too early"
    );
    let out = e.poll(now + GAP_TIMEOUT);
    assert_eq!(out.actions, [Action::GetDifference]);
    // While the difference is in flight, new updates are held back.
    assert!(applied(&e.feed(short(delete(16, 1)), now)).is_empty());

    // The difference brings us to pts 15; the held-back 16 then applies, 15 is a duplicate.
    let diff: eu::Difference = tu::Difference {
        new_messages: vec![],
        new_encrypted_messages: vec![],
        other_updates: vec![delete(14, 4)],
        chats: vec![],
        users: vec![],
        state: tu::State {
            pts: 15,
            qts: 0,
            date: 5,
            seq: 0,
            unread_count: 0,
        }
        .into(),
    }
    .into();
    let out = e.apply_difference(diff, now);
    assert_eq!(applied(&out), [14, 16]);
    assert_eq!(e.state().pts, 16);
    assert!(out.actions.is_empty());
}

#[test]
fn difference_slices_continue() {
    let mut e = engine(10);
    let slice: eu::Difference = tu::DifferenceSlice {
        new_messages: vec![],
        new_encrypted_messages: vec![],
        other_updates: vec![],
        chats: vec![],
        users: vec![],
        intermediate_state: tu::State {
            pts: 50,
            qts: 0,
            date: 5,
            seq: 0,
            unread_count: 0,
        }
        .into(),
    }
    .into();
    let out = e.apply_difference(slice, t0());
    assert_eq!(out.actions, [Action::GetDifference]);
    assert_eq!(e.state().pts, 50);

    let too_long: eu::Difference = tu::DifferenceTooLong { pts: 900 }.into();
    let out = e.apply_difference(too_long, t0());
    assert!(out.items.iter().any(|i| matches!(i, Item::ResetCommon)));
    assert_eq!(e.state().pts, 900);
}

#[test]
fn no_state_holds_everything_until_get_state() {
    let mut e = Engine::new(None, []);
    assert!(applied(&e.feed(short(delete(21, 1)), t0())).is_empty());
    assert!(e.next_deadline().is_none(), "no gap timer without a state");
    let out = e.set_state(state(20, 0, 0));
    assert_eq!(applied(&out), [21]);
}

#[test]
fn channels_have_their_own_pts() {
    let mut e = Engine::new(Some(state(10, 0, 0)), [(777, 100)]);
    let now = t0();
    assert_eq!(
        applied(&e.feed(short(channel_delete(777, 101, 1)), now)),
        [101]
    );
    // Unknown channel: its first update sets the position.
    assert_eq!(applied(&e.feed(short(channel_delete(888, 5, 1)), now)), [5]);
    assert_eq!(e.channel_pts(888), Some(5));
    // A gap in 777 asks for its channel difference, not the common one.
    e.feed(short(channel_delete(777, 104, 1)), now);
    let out = e.poll(now + GAP_TIMEOUT);
    assert_eq!(
        out.actions,
        [Action::GetChannelDifference {
            channel_id: 777,
            pts: 101
        }]
    );
    assert_eq!(e.state().pts, 10, "common pts untouched");

    let diff: eu::ChannelDifference = tu::ChannelDifference {
        r#final: true,
        pts: 103,
        timeout: None,
        new_messages: vec![],
        other_updates: vec![],
        chats: vec![],
        users: vec![],
    }
    .into();
    let out = e.apply_channel_difference(777, diff, now);
    assert_eq!(
        applied(&out),
        [104],
        "the held-back update applies after the difference"
    );
    assert_eq!(e.channel_pts(777), Some(104));
    let dirty = e.take_dirty_channels();
    assert!(dirty.contains(&(777, 104)) && dirty.contains(&(888, 5)));
}

#[test]
fn qts_sequence() {
    let mut e = Engine::new(Some(state(10, 3, 0)), []);
    let now = t0();
    assert_eq!(applied(&e.feed(short(bot_stopped(4)), now)), [4]);
    assert!(applied(&e.feed(short(bot_stopped(6)), now)).is_empty());
    assert_eq!(applied(&e.feed(short(bot_stopped(5)), now)), [5, 6]);
    assert_eq!(e.state().qts, 6);
}

#[test]
fn seq_gaps_lead_to_difference() {
    let mut e = Engine::new(Some(state(10, 0, 7)), []);
    let now = t0();
    e.feed(container(8, 8, vec![]), now);
    assert_eq!(e.state().seq, 8);
    // seq 9 is missing.
    e.feed(container(10, 10, vec![]), now);
    assert_eq!(e.state().seq, 8);
    assert_eq!(e.poll(now + GAP_TIMEOUT).actions, [Action::GetDifference]);
    // A container that was already applied is ignored entirely.
    let mut e = Engine::new(Some(state(10, 0, 7)), []);
    assert!(
        e.feed(container(6, 7, vec![bot_stopped(1)]), now)
            .items
            .is_empty()
    );
}

#[test]
fn reconnect_asks_for_difference_once() {
    let mut e = engine(10);
    assert_eq!(e.connection_restored().actions, [Action::GetDifference]);
    assert!(
        e.connection_restored().actions.is_empty(),
        "already requesting"
    );
}

/// Every `Update` constructor with `pts`/`qts` in the schema must be classified;
/// otherwise a new layer could add a sequenced update we silently treat as unsequenced.
#[test]
fn every_sequenced_update_in_the_schema_is_classified() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../schema/api.tl"))
        .unwrap();
    let schema = tdr_tl_parser::Schema::parse(&src).unwrap();
    let missing: Vec<&str> = schema
        .definitions
        .iter()
        .filter(|d| d.result == "Update")
        .filter(|d| d.params.iter().any(|p| p.name == "pts" || p.name == "qts"))
        .map(|d| d.name.as_str())
        .filter(|n| !SEQUENCED_UPDATES.contains(n))
        .collect();
    assert!(
        missing.is_empty(),
        "classify these updates in engine.rs: {missing:?}"
    );
}
