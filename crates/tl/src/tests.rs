use crate::*;

fn roundtrip<T: Serializable + Deserializable + PartialEq + std::fmt::Debug>(value: T) {
    let bytes = Bytes::from(value.to_bytes());
    let mut r = Reader::new(&bytes);
    let back = T::deserialize(&mut r).unwrap();
    assert_eq!(back, value);
    assert_eq!(r.remaining(), 0);
}

#[test]
fn layer_matches_schema() {
    const { assert!(LAYER >= 233) };
}

#[test]
fn tl_bytes_framing() {
    for len in [0usize, 1, 3, 4, 253, 254, 255, 1000, 70_000] {
        let data = Bytes::from(vec![7u8; len]);
        let mut out = Vec::new();
        write_tl_bytes(&mut out, &data);
        assert_eq!(out.len() % 4, 0, "len {len}");
        let buf = Bytes::from(out);
        let mut r = Reader::new(&buf);
        assert_eq!(r.read_bytes().unwrap(), data);
        assert_eq!(r.remaining(), 0);
    }
}

#[test]
fn bytes_are_zero_copy() {
    let mut out = Vec::new();
    write_tl_bytes(&mut out, b"hello world");
    let buf = Bytes::from(out);
    let got = Reader::new(&buf).read_bytes().unwrap();
    // The slice points into the original allocation.
    assert!(buf.as_ptr_range().contains(&got.as_ptr()));
}

#[test]
fn known_encoding() {
    // inputPeerUser#dde8a54c user_id:long access_hash:long = InputPeer;
    let peer: enums::InputPeer = types::InputPeerUser {
        user_id: 1,
        access_hash: 2,
    }
    .into();
    let b = peer.to_bytes();
    assert_eq!(&b[..4], &0xdde8a54cu32.to_le_bytes());
    assert_eq!(b.len(), 4 + 8 + 8);
    roundtrip(peer);
    roundtrip(enums::InputPeer::Empty);
}

#[test]
fn flags_roundtrip() {
    let req = functions::messages::SendMessage {
        no_webpage: true,
        silent: false,
        background: true,
        clear_draft: false,
        noforwards: false,
        update_stickersets_order: false,
        invert_media: false,
        allow_paid_floodskip: false,
        peer: enums::InputPeer::InputPeerSelf,
        reply_to: None,
        message: "привет".into(),
        random_id: 42,
        reply_markup: None,
        entities: None,
        schedule_date: Some(5),
        schedule_repeat_period: None,
        send_as: None,
        quick_reply_shortcut: None,
        effect: None,
        allow_paid_stars: None,
        suggested_post: None,
        rich_message: None,
    };
    let b = req.to_bytes();
    assert_eq!(&b[..4], &0xfef48f62u32.to_le_bytes());
    let flags = u32::from_le_bytes(b[4..8].try_into().unwrap());
    assert_eq!(flags & (1 << 1), 1 << 1, "no_webpage");
    assert_eq!(flags & (1 << 6), 1 << 6, "background");
    assert_eq!(flags & (1 << 10), 1 << 10, "schedule_date");
    assert_eq!(flags & (1 << 5), 0, "silent");
}

#[test]
fn enums_stay_small() {
    // Big constructors are boxed, so moving updates/messages around is cheap.
    assert!(std::mem::size_of::<enums::Message>() <= 24);
    assert!(std::mem::size_of::<enums::Update>() <= 24);
    assert!(std::mem::size_of::<enums::InputPeer>() <= 24);
}

#[test]
fn vectors_and_bool() {
    roundtrip(vec![1i64, 2, 3]);
    roundtrip(true);
    roundtrip(false);
    roundtrip(vec![true, false]);
    roundtrip(String::from("text"));
}

#[test]
fn rejects_wrong_constructor() {
    let buf = Bytes::from(0xdeadbeefu32.to_le_bytes().to_vec());
    let err = enums::InputPeer::from_bytes(&buf).unwrap_err();
    assert_eq!(
        err,
        DeserializeError::UnexpectedConstructor { id: 0xdeadbeef }
    );
}

#[test]
fn huge_vector_length_is_rejected() {
    let mut out = VECTOR_ID.to_le_bytes().to_vec();
    out.extend_from_slice(&u32::MAX.to_le_bytes());
    let buf = Bytes::from(out);
    assert!(Vec::<i64>::from_bytes(&buf).is_err());
}

#[test]
fn mtproto_types_exist() {
    let req = mtproto::functions::ReqPqMulti { nonce: [1; 16] };
    let b = req.to_bytes();
    assert_eq!(&b[..4], &0xbe7e8ef1u32.to_le_bytes());
    assert_eq!(b.len(), 20);
}
