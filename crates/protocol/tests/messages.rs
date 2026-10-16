//! Roundtrip and strictness tests for every message type.

mod common;

use std::collections::BTreeSet;

use bytes::{Bytes, BytesMut};
use protocol::FrameDecoder;
use protocol::frame::{Endpoint, Flags, Frame, FrameMeta, MsgType};
use protocol::messages::*;

#[test]
fn every_message_type_is_covered_by_the_samples() {
    let covered: BTreeSet<MsgType> = common::all_messages()
        .iter()
        .map(Message::msg_type)
        .collect();
    let all: BTreeSet<MsgType> = MsgType::ALL.into_iter().collect();
    assert_eq!(covered, all);
}

#[test]
fn every_message_roundtrips_through_payload_encoding() {
    for msg in common::all_messages() {
        let mut buf = Vec::new();
        msg.encode_payload(&mut buf).expect("encode");
        let back = Message::decode(msg.msg_type(), Bytes::from(buf)).expect("decode");
        assert_eq!(back, msg);
    }
}

#[test]
fn every_message_roundtrips_through_frames_and_the_stream_decoder() {
    let messages = common::all_messages();
    let frames = common::all_frames();
    let mut stream = BytesMut::new();
    for f in &frames {
        stream.extend_from_slice(f.as_bytes());
    }
    let mut dec = FrameDecoder::new();
    let mut decoded = Vec::new();
    while let Some(frame) = dec.decode_frame(&mut stream).expect("decode") {
        assert_eq!(frame.check_rules(), Ok(frame.msg_type().expect("known")));
        decoded.push(Message::from_frame(&frame).expect("message"));
    }
    assert_eq!(decoded, messages);
}

#[test]
fn telemetry_payload_decodes_to_records() {
    let p = common::tlv_payload(150);
    let (header, records) = p.decode().expect("tlv");
    assert_eq!(
        header.schema_version,
        telemetry_model::TELEMETRY_SCHEMA_VERSION
    );
    assert_eq!(records.len(), 150);
    // Unavailable channels stay unavailable (never a fabricated 0).
    let unavailable = records
        .iter()
        .filter(|r| r.sample.quality == telemetry_model::Quality::Unavailable)
        .count();
    assert_eq!(unavailable, 15);
    assert!(TelemetryPayload::new(Bytes::from_static(&[0u8; 3])).is_err());
}

#[test]
fn telemetry_payload_detach_does_not_pin_the_read_buffer() {
    // Review finding: a TelemetryPayload from Message::from_frame shares the codec's read buffer,
    // so a kept payload pinned the whole allocation.
    let msg = Message::TelemetrySnapshot(common::tlv_payload(20));
    let frame = msg
        .to_frame(&FrameMeta::stream(
            protocol::frame::FramePriority::P0,
            7,
            1,
            0,
        ))
        .expect("frame");
    let mut stream = BytesMut::new();
    stream.extend_from_slice(frame.as_bytes());
    stream.extend_from_slice(&[0u8; 4096]); // more buffered bytes behind the frame
    let mut dec = FrameDecoder::new();
    let decoded = dec
        .decode_frame(&mut stream)
        .expect("decode")
        .expect("frame");
    let Message::TelemetrySnapshot(payload) = Message::from_frame(&decoded).expect("message")
    else {
        panic!("snapshot expected");
    };
    let view = decoded.as_bytes().as_ptr_range();
    assert!(
        view.contains(&payload.as_bytes().as_ptr()),
        "from_frame is zero-copy"
    );
    let detached = payload.detach();
    assert_eq!(detached, payload);
    assert!(
        !view.contains(&detached.as_bytes().as_ptr()),
        "detached payload must not share the read buffer"
    );
    assert_eq!(detached.decode().expect("tlv").1.len(), 20);
}

#[test]
fn trailing_bytes_are_rejected() {
    let msg = Message::Ping(Ping { id: 1, t0: 2 });
    let mut buf = Vec::new();
    msg.encode_payload(&mut buf).expect("encode");
    buf.push(0);
    let err = Message::decode(MsgType::Ping, Bytes::from(buf)).expect_err("strict");
    assert!(matches!(err, MessageError::TrailingBytes { count: 1, .. }));
    assert_eq!(err.error_code(), ErrorCode::Malformed);
}

/// Re-stamps a frame's header minor (and CRC) as a peer of another minor would have sent it.
fn with_minor(frame: &Frame, minor: u8) -> Frame {
    let mut raw = frame.as_bytes().to_vec();
    raw[3] = minor;
    let n = raw.len() - 4;
    let crc = crc32c::crc32c(&raw[..n]);
    raw[n..].copy_from_slice(&crc.to_le_bytes());
    Frame::decode(Bytes::from(raw)).expect("valid frame")
}

#[test]
fn fields_appended_by_a_newer_minor_are_ignored() {
    // A hypothetical 1.(n+1) HELLO with an appended `os_version: String` field.
    let hello = common::all_messages()
        .into_iter()
        .find(|m| matches!(m, Message::Hello(_)))
        .expect("sample hello");
    let mut payload = Vec::new();
    hello.encode_payload(&mut payload).expect("encode");
    let appended = b"\x0aWindows 11";
    payload.extend_from_slice(appended);
    let frame = Frame::build(MsgType::Hello, &FrameMeta::control(0, 0), &payload).expect("frame");

    // Same minor: strict.
    assert!(matches!(
        Message::from_frame(&frame),
        Err(MessageError::TrailingBytes { count, .. }) if count == appended.len()
    ));
    // Newer sender minor: the 1.0 receiver negotiates instead of rejecting the HELLO.
    let newer = with_minor(&frame, protocol::PROTOCOL_MINOR + 1);
    assert_eq!(Message::from_frame(&newer).expect("decode"), hello);
    assert_eq!(
        Message::decode_from_minor(
            MsgType::Hello,
            Bytes::from(payload),
            protocol::PROTOCOL_MINOR + 1
        )
        .expect("decode"),
        hello
    );
    // Appended bytes never mask a broken known part.
    let mut broken = Vec::new();
    hello.encode_payload(&mut broken).expect("encode");
    broken.truncate(broken.len() - 1);
    assert!(Message::decode_from_minor(MsgType::Hello, Bytes::from(broken), u8::MAX).is_err());
}

#[test]
fn truncated_payloads_are_rejected() {
    for msg in common::all_messages() {
        let mut buf = Vec::new();
        msg.encode_payload(&mut buf).expect("encode");
        if buf.is_empty() {
            continue;
        }
        let cut = Bytes::from(buf[..buf.len() - 1].to_vec());
        // TLV payloads are checked lazily (header only), the TLV decoder then rejects them.
        match Message::decode(msg.msg_type(), cut) {
            Err(_) => {}
            Ok(
                Message::TelemetryDelta(p)
                | Message::TelemetrySnapshot(p)
                | Message::WeatherUpdate(p),
            ) => {
                assert!(p.decode().is_err());
            }
            Ok(other) => panic!("truncated {} decoded as {other:?}", msg.msg_type()),
        }
    }
}

#[test]
fn compressed_and_unknown_frames_are_refused() {
    let meta = FrameMeta::control(0, 0);
    // A 1.0 sender cannot even build a compressed frame.
    let refused: Result<Frame, MessageError> =
        Frame::encode_with(MsgType::Ping, &meta, Flags::PAYLOAD_ZSTD, 0, |b| {
            b.extend_from_slice(&[1, 2]);
            Ok(())
        });
    assert!(matches!(
        refused,
        Err(MessageError::Frame(
            protocol::FrameError::InvalidSendFlags { .. }
        ))
    ));
    // A foreign sender's compressed frame is refused by the receiver.
    let mut raw = Frame::build(MsgType::Ping, &meta, &[1, 2])
        .expect("frame")
        .as_bytes()
        .to_vec();
    raw[5] = Flags::PAYLOAD_ZSTD.bits();
    let n = raw.len() - 4;
    let crc = crc32c::crc32c(&raw[..n]);
    raw[n..].copy_from_slice(&crc.to_le_bytes());
    let zstd = Frame::decode(Bytes::from(raw)).expect("valid frame");
    assert!(matches!(
        Message::from_frame(&zstd),
        Err(MessageError::CompressedPayload { .. })
    ));
}

#[test]
fn secrets_are_redacted_in_debug_output() {
    let auth = Message::Auth(Auth::Password {
        username: "drv".into(),
        password: Secret::new("hunter2"),
    });
    let dbg = format!("{auth:?}");
    assert!(!dbg.contains("hunter2"), "{dbg}");
    assert!(dbg.contains("redacted"));
    for msg in common::all_messages() {
        let dbg = format!("{msg:?}");
        for secret in [
            "hunter2",
            "\"resume\"",
            "\"ticket\"",
            "\"at\"",
            "\"rt\"",
            common::CODE,
        ] {
            assert!(!dbg.contains(secret), "{secret} leaked in {dbg}");
        }
    }
    // Connection codes stay redacted from user input on.
    let code = normalize_connect_code("7kq4-mz9t").expect("valid");
    assert!(!format!("{code:?}").contains("7KQ4"));
}

#[test]
fn connect_codes_normalize_to_the_canonical_form() {
    let canonical = |s: &str| normalize_connect_code(s).map(|c| c.expose().to_owned());
    for input in ["7KQ4MZ9T", "7kq4-mz9t", " 7KQ4 - MZ9T\t", "7-K-Q-4-M-Z-9-T"] {
        assert_eq!(canonical(input).as_deref(), Ok("7KQ4MZ9T"), "{input:?}");
    }
    // Crockford look-alikes: I/L read as 1, O as 0.
    assert_eq!(canonical("IL0O-ilo0").as_deref(), Ok("11001100"));
    // Every alphabet character is accepted as is.
    let alphabet = std::str::from_utf8(CONNECT_CODE_ALPHABET).expect("ascii");
    for chunk in alphabet.as_bytes().chunks(CONNECT_CODE_LEN) {
        let s = std::str::from_utf8(chunk).expect("ascii");
        assert_eq!(canonical(s).as_deref(), Ok(s));
    }
    use ConnectCodeError::*;
    assert_eq!(canonical("7KQ4-MZ9"), Err(WrongLength { len: 7 }));
    assert_eq!(canonical("7KQ4-MZ9TX"), Err(WrongLength { len: 9 }));
    assert_eq!(canonical(""), Err(WrongLength { len: 0 }));
    assert_eq!(canonical("--  --"), Err(WrongLength { len: 0 }));
    for bad in [
        "7KQ4-MZ9U",
        "7KQ4_MZ9T",
        "7KQ4-MZ9É",
        "7KQ4-MZ9\u{0}",
        "7KQ4.MZ9T",
    ] {
        assert_eq!(canonical(bad), Err(InvalidCharacter), "{bad:?}");
    }
    // Long input is refused without being buffered.
    assert_eq!(
        canonical(&"A".repeat(10_000)),
        Err(WrongLength { len: 10_000 })
    );
    // Error messages never echo code characters.
    for err in [WrongLength { len: 9 }, InvalidCharacter] {
        assert!(!err.to_string().contains("7KQ4"));
    }
}

#[test]
fn d022_error_codes_have_stable_wire_indices() {
    // ErrorCode is frozen for major 1 (§10): the D-022 codes are appended after Internal = 12.
    for (code, index) in [
        (ErrorCode::Internal, 12u8),
        (ErrorCode::CodeInvalid, 13),
        (ErrorCode::CodeExpired, 14),
        (ErrorCode::SessionFull, 15),
    ] {
        let mut buf = Vec::new();
        Message::Error(ErrorMsg {
            code,
            message: String::new(),
            fatal: false,
        })
        .encode_payload(&mut buf)
        .expect("encode");
        assert_eq!(buf, vec![index, 0x00, 0x00], "{code:?}");
    }
}

#[test]
fn connect_code_request_has_an_empty_payload() {
    let mut buf = Vec::new();
    Message::ConnectCodeRequest(ConnectCodeRequest)
        .encode_payload(&mut buf)
        .expect("encode");
    assert!(buf.is_empty());
    assert_eq!(
        Message::decode(MsgType::ConnectCodeRequest, Bytes::new()).expect("decode"),
        Message::ConnectCodeRequest(ConnectCodeRequest)
    );
    assert!(matches!(
        Message::decode(MsgType::ConnectCodeRequest, Bytes::from_static(&[0])),
        Err(MessageError::TrailingBytes { count: 1, .. })
    ));
}

#[test]
fn session_resume_carries_either_the_driver_token_or_the_engineer_ticket() {
    // postcard variant index 0 = driver, 1 = engineer; a ticket can never be read as a driver token.
    let driver = Message::SessionResume(SessionResume::Driver {
        session_id: 1,
        resume_token: Secret::new("t"),
        new_sequence_space: false,
    });
    let engineer = Message::SessionResume(SessionResume::Engineer {
        session_id: 1,
        ticket: Secret::new("t"),
    });
    let mut d = Vec::new();
    driver.encode_payload(&mut d).expect("encode");
    let mut e = Vec::new();
    engineer.encode_payload(&mut e).expect("encode");
    assert_eq!(d, vec![0, 1, 1, b't', 0]);
    assert_eq!(e, vec![1, 1, 1, b't']);
    assert_eq!(
        Message::decode(MsgType::SessionResume, Bytes::from(e)).expect("decode"),
        engineer
    );
    // SESSION_INFO hands each role its own credential kind.
    let infos: Vec<ResumeCredential> = common::all_messages()
        .into_iter()
        .filter_map(|m| match m {
            Message::SessionInfo(i) => Some(i.resume),
            _ => None,
        })
        .collect();
    assert!(matches!(
        infos.as_slice(),
        [
            ResumeCredential::DriverToken(_),
            ResumeCredential::EngineerTicket(_)
        ]
    ));
}

#[test]
fn client_roles_map_to_endpoints() {
    assert_eq!(Endpoint::from(ClientRole::Driver), Endpoint::Driver);
    assert_eq!(Endpoint::from(ClientRole::Engineer), Endpoint::Engineer);
}

#[test]
fn hello_wire_form_is_self_describing() {
    let msgs = common::all_messages();
    let mut client = Vec::new();
    msgs[0].encode_payload(&mut client).expect("encode");
    let mut server = Vec::new();
    msgs[1].encode_payload(&mut server).expect("encode");
    // postcard variant index: 0 = client form, 1 = server form.
    assert_eq!(client[0], 0);
    assert_eq!(server[0], 1);
}

#[test]
fn postcard_layout_of_ping_is_stable() {
    // Varint encoding: id = 1, t0 = 300 (0xAC 0x02).
    let mut buf = Vec::new();
    Message::Ping(Ping { id: 1, t0: 300 })
        .encode_payload(&mut buf)
        .expect("encode");
    assert_eq!(buf, vec![0x01, 0xAC, 0x02]);
    let mut buf = Vec::new();
    Message::Error(ErrorMsg {
        code: ErrorCode::CrcMismatch,
        message: "x".into(),
        fatal: true,
    })
    .encode_payload(&mut buf)
    .expect("encode");
    assert_eq!(buf, vec![0x04, 0x01, b'x', 0x01]);
}

#[test]
fn engineer_direction_whitelist() {
    let accepted: Vec<MsgType> = common::all_messages()
        .iter()
        .filter(|m| m.driver_accepts())
        .map(Message::msg_type)
        .collect();
    for t in &accepted {
        assert!(
            matches!(
                t,
                MsgType::Hello
                    | MsgType::AuthOk
                    | MsgType::SessionInfo
                    | MsgType::ConnectCode
                    | MsgType::EngineerList
                    | MsgType::SnapshotRequest
                    | MsgType::Pong
                    | MsgType::Disconnect
                    | MsgType::Error
            ),
            "{t} must not be accepted by the driver"
        );
    }
    assert!(accepted.contains(&MsgType::SnapshotRequest));
    assert!(accepted.contains(&MsgType::ConnectCode));
    assert!(accepted.contains(&MsgType::EngineerList));
    // The driver never processes its own session-control requests coming back.
    for m in common::all_messages() {
        if matches!(
            m,
            Message::ConnectCodeRequest(_) | Message::EngineerKick(_) | Message::SessionJoin(_)
        ) {
            assert!(!m.driver_accepts(), "{}", m.msg_type());
        }
    }
    // The client HELLO form is never meaningful to a driver.
    assert!(!common::all_messages()[0].driver_accepts());
    // Telemetry never flows engineer → driver.
    assert!(!MsgType::TelemetryDelta.may_send(Endpoint::Relay, Endpoint::Driver));
}

#[test]
fn stream_messages_refuse_control_metadata() {
    let msg = Message::TelemetryDelta(common::tlv_payload(3));
    assert!(matches!(
        msg.to_frame(&FrameMeta::control(1, 0)),
        Err(MessageError::Frame(protocol::FrameError::HeaderRule(_)))
    ));
}
