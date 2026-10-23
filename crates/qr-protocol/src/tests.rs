use super::*;
use base64::Engine;

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn v1_wire(len: usize) -> Vec<u8> {
    let mut wire = vec![V1_PREFIX];
    wire.resize(len, 7);
    wire
}

/// Every frame of `payload` through a collector, in order.
fn transfer(payload: &[u8]) -> Vec<u8> {
    let mut collector = Collector::new();
    let mut last = None;
    for frame in encode_frames(payload, DEFAULT_CHUNK).unwrap() {
        last = Some(collector.push(&frame.encode()).unwrap());
    }
    let Some(Collected::Complete { payload, .. }) = last else {
        panic!("the transfer completes");
    };
    payload
}

#[test]
fn single_frame_roundtrip() {
    let payload = b"hello signing world";
    let frames = encode_frames(payload, 768).unwrap();
    assert_eq!(frames.len(), 1);
    let parsed = parse_frame(&frames[0].encode()).unwrap();
    let mut asm = Assembler::new();
    let done = asm.push(parsed).unwrap();
    assert_eq!(done.as_deref(), Some(payload.as_slice()));
}

#[test]
fn the_header_is_little_endian_wincode() {
    let frame = encode_frames(b"abc", 64).unwrap().remove(0);
    let bytes = frame.encode();
    assert_eq!(&bytes[..4], MAGIC);
    assert_eq!(bytes[4], VERSION);
    assert_eq!(&bytes[13..15], &0u16.to_le_bytes());
    assert_eq!(&bytes[15..17], &1u16.to_le_bytes());
    assert_eq!(&bytes[17..21], &3u32.to_le_bytes());
    assert_eq!(&bytes[21..25], &crc32(b"abc").to_le_bytes());
    assert_eq!(&bytes[HEADER_LEN..HEADER_LEN + 3], b"abc");
}

#[test]
fn every_frame_is_the_same_size_however_small_the_payload() {
    for payload in [vec![1u8], vec![2; 768], vec![3; 769], vec![4; 5000]] {
        for frame in encode_frames(&payload, DEFAULT_CHUNK).unwrap() {
            assert_eq!(frame.encode().len(), HEADER_LEN + DEFAULT_CHUNK);
        }
        assert_eq!(transfer(&payload), payload);
    }
}

#[test]
fn the_largest_payload_out_of_order() {
    let payload: Vec<u8> = (0..MAX_PAYLOAD).map(|i| (i % 251) as u8).collect();
    let mut frames = encode_frames(&payload, 700).unwrap();
    assert!(frames.len() > 1);
    frames.reverse();

    let mut asm = Assembler::new();
    let mut result = None;
    for frame in frames {
        result = asm.push(parse_frame(&frame.encode()).unwrap()).unwrap();
    }
    assert_eq!(result.unwrap(), payload);
}

#[test]
fn rejects_too_large() {
    let payload = vec![1u8; MAX_PAYLOAD + 1];
    assert!(matches!(
        encode_frames(&payload, 768),
        Err(ProtocolError::TooLarge { .. })
    ));
}

#[test]
fn rejects_truncated() {
    assert_eq!(parse_frame(b"QRS1"), Err(ProtocolError::Truncated));
}

#[test]
fn rejects_bad_magic() {
    let mut bytes = encode_frames(b"abc", 64).unwrap()[0].encode();
    bytes[0] = b'X';
    assert_eq!(parse_frame(&bytes), Err(ProtocolError::BadMagic));
}

#[test]
fn new_transfer_replaces_old() {
    let a = encode_frames(b"aaaa", 64).unwrap();
    let b = encode_frames(b"bbbbbbbbbbbb", 64).unwrap();
    let mut asm = Assembler::new();
    asm.push(a[0].clone()).unwrap();
    let done = asm.push(b.into_iter().next().unwrap()).unwrap();
    assert_eq!(done.as_deref(), Some(b"bbbbbbbbbbbb".as_slice()));
}

#[test]
fn collector_hands_a_transfer_over_once_and_ignores_other_codes() {
    let payload: Vec<u8> = (0..200u8).collect();
    let frames = encode_frames(&payload, 64).unwrap();
    assert!(frames.len() > 1);
    let mut collector = Collector::new();
    assert_eq!(
        collector.push(b"https://example.test").unwrap(),
        Collected::Unrelated
    );

    let last = frames.len() - 1;
    for (i, frame) in frames.iter().enumerate() {
        let step = collector.push(&frame.encode()).unwrap();
        if i < last {
            assert!(matches!(step, Collected::Collecting { received, .. } if received == i + 1));
        } else {
            assert!(
                matches!(step, Collected::Complete { payload: ref got, .. } if *got == payload)
            );
        }
    }
    assert!(collector.have().iter().all(|have| *have));
    assert!(matches!(
        collector.push(&frames[0].encode()).unwrap(),
        Collected::Repeat { .. }
    ));
}

#[test]
fn crc_detects_corruption() {
    let mut frame = encode_frames(b"payload-bytes", 64).unwrap().remove(0);
    frame.chunk[0] ^= 0xff;
    let mut asm = Assembler::new();
    assert_eq!(asm.push(frame), Err(ProtocolError::CorruptPayload));
}

#[test]
fn a_frame_shorter_than_its_stated_length_is_corrupt() {
    let mut frame = encode_frames(b"payload-bytes", 64).unwrap().remove(0);
    frame.chunk.truncate(4);
    let mut asm = Assembler::new();
    assert_eq!(asm.push(frame), Err(ProtocolError::CorruptPayload));
}

#[test]
fn transaction_and_signature_roundtrip() {
    let tx = QrObject::Transaction(v1_wire(24));
    assert_eq!(decode_object(&encode_object(&tx).unwrap()).unwrap(), tx);

    let sig = QrObject::Signature(Signed::Transaction {
        pubkey: [7; 32],
        signature: [9; 64],
    });
    let encoded = encode_object(&sig).unwrap();
    // Count, kind, target, pubkey, signature.
    assert_eq!(encoded.len(), 2 + 1 + 1 + 32 + 64);
    assert_eq!(&encoded[..4], &[1, 0, 2, 0]);
    assert_eq!(decode_object(&transfer(&encoded)).unwrap(), sig);
}

#[test]
fn the_largest_transaction_and_its_note_travel_together() {
    let wire = v1_wire(MAX_TRANSACTION_SIZE);
    let pins: Vec<ContextPin> = (0..MAX_PINS as u8)
        .map(|i| ContextPin {
            program_id: [i; 32],
            idl_sha256: [i; 32],
        })
        .collect();
    let authorizations: Vec<ContextAuthorization> = (0..MAX_AUTHORIZATIONS as u8)
        .map(|i| ContextAuthorization {
            account: [i; 32],
            scheme: 1,
            nonce: [i; 32],
            leaf_floor: u32::from(i),
        })
        .collect();
    let context = TxContext::new([5; 32], &pins, &authorizations).unwrap();
    let objects = vec![QrObject::Transaction(wire), QrObject::Context(context)];
    let payload = encode_payload(&objects).unwrap();
    assert!(payload.len() <= MAX_PAYLOAD);
    assert_eq!(decode_payload(&transfer(&payload)).unwrap(), objects);
    // One object is asked for, and two arrived.
    assert_eq!(decode_object(&payload), Err(ProtocolError::NotOneObject(2)));
}

#[test]
fn a_payload_is_refused_empty_or_with_too_many_objects() {
    assert_eq!(encode_payload(&[]), Err(ProtocolError::Empty));
    assert_eq!(decode_payload(&[0, 0]), Err(ProtocolError::Empty));
    let sig = QrObject::Signature(Signed::Transaction {
        pubkey: [7; 32],
        signature: [9; 64],
    });
    assert_eq!(
        encode_payload(&vec![sig; MAX_OBJECTS + 1]),
        Err(ProtocolError::BadObject)
    );
}

#[test]
fn a_transaction_object_is_a_v1_wire() {
    assert_eq!(
        encode_object(&QrObject::Transaction(vec![1, 0, 0])),
        Err(ProtocolError::BadObject)
    );
    assert_eq!(
        encode_object(&QrObject::Transaction(v1_wire(MAX_TRANSACTION_SIZE + 1))),
        Err(ProtocolError::BadObject)
    );
    // A bare wire is not a payload.
    assert_eq!(decode_payload(&v1_wire(40)), Err(ProtocolError::BadObject));
}

#[test]
fn pasted_text_is_json_or_a_transaction_in_any_format() {
    let sig = QrObject::Signature(Signed::Transaction {
        pubkey: [7; 32],
        signature: [9; 64],
    });
    let json = payload_json(std::slice::from_ref(&sig)).unwrap();
    assert_eq!(
        parse_payload_text(&json, |_| true).unwrap(),
        vec![sig.clone()]
    );
    // The wire form of a payload is not text anyone pastes.
    let payload = encode_object(&sig).unwrap();
    assert_eq!(
        parse_payload_text(&b64(&payload), |_| false),
        Err(ProtocolError::BadObject)
    );

    let legacy = vec![1, 2, 3, 4];
    let is_legacy = |bytes: &[u8]| bytes == [1, 2, 3, 4];
    assert_eq!(
        parse_payload_text(&b64(&legacy), is_legacy).unwrap(),
        vec![QrObject::Transaction(legacy.clone())]
    );
    assert_eq!(
        parse_payload_text(&bs58::encode(&legacy).into_string(), is_legacy).unwrap(),
        vec![QrObject::Transaction(legacy)]
    );
    assert_eq!(
        parse_payload_text("not anything", |_| true),
        Err(ProtocolError::BadObject)
    );
    assert_eq!(
        parse_payload_text("  ", |_| true),
        Err(ProtocolError::Empty)
    );
}

#[test]
fn context_note_round_trips_and_rejects_extra_bytes() {
    let pins = [
        ContextPin {
            program_id: [1; 32],
            idl_sha256: [2; 32],
        },
        ContextPin {
            program_id: [3; 32],
            idl_sha256: [4; 32],
        },
        // The same program again is kept once.
        ContextPin {
            program_id: [1; 32],
            idl_sha256: [9; 32],
        },
    ];
    let context = TxContext::new([5; 32], &pins, &[]).unwrap();
    assert_eq!(context.pins.len(), 2);
    let object = QrObject::Context(context);
    let mut encoded = encode_object(&object).unwrap();
    assert_eq!(decode_object(&encoded).unwrap(), object);
    encoded.push(0);
    assert_eq!(decode_object(&encoded), Err(ProtocolError::BadObject));
}

#[test]
fn a_note_with_more_than_it_holds_is_refused() {
    let too_many: Vec<ContextPin> = (0..=MAX_PINS as u8)
        .map(|i| ContextPin {
            program_id: [i; 32],
            idl_sha256: [i; 32],
        })
        .collect();
    assert_eq!(
        TxContext::new([1; 32], &too_many, &[]),
        Err(ProtocolError::BadObject)
    );
}

#[test]
fn a_signature_says_what_it_signs() {
    let for_transaction = QrObject::Signature(Signed::Transaction {
        pubkey: [7; 32],
        signature: [9; 64],
    });
    let for_authorization = QrObject::Signature(Signed::Authorization {
        authority: [7; 32],
        signature: [9; 64],
    });
    // The same key and the same bytes are two different objects.
    let (a, b) = (
        encode_object(&for_transaction).unwrap(),
        encode_object(&for_authorization).unwrap(),
    );
    assert_ne!(a, b);
    assert_eq!(decode_object(&a).unwrap(), for_transaction);
    assert_eq!(decode_object(&b).unwrap(), for_authorization);

    let mut unknown = a.clone();
    unknown[3] = 3;
    assert_eq!(decode_object(&unknown), Err(ProtocolError::BadObject));
    let mut unknown_kind = a.clone();
    unknown_kind[2] = 0;
    assert_eq!(decode_object(&unknown_kind), Err(ProtocolError::BadObject));
    for cut in [3, 40, a.len() - 1] {
        assert_eq!(decode_object(&a[..cut]), Err(ProtocolError::BadObject));
    }
    let mut longer = a;
    longer.push(0);
    assert_eq!(decode_object(&longer), Err(ProtocolError::BadObject));
}

#[test]
fn the_longest_account_signature_fits_a_transfer() {
    // ML-DSA-44: the longest signature any scheme makes.
    let object = QrObject::Signature(Signed::AccountAuthorization {
        account: [1; 32],
        nonce: [2; 32],
        signature: vec![3; 2420],
    });
    let encoded = encode_object(&object).unwrap();
    assert_eq!(encoded.len(), 2 + 1 + 1 + 32 + 32 + 2 + 2420);
    assert_eq!(decode_object(&transfer(&encoded)).unwrap(), object);

    // The length it states has to be the length it has, and not zero.
    assert_eq!(
        decode_object(&encoded[..encoded.len() - 1]),
        Err(ProtocolError::BadObject)
    );
    let empty = QrObject::Signature(Signed::AccountAuthorization {
        account: [1; 32],
        nonce: [2; 32],
        signature: Vec::new(),
    });
    assert_eq!(encode_object(&empty), Err(ProtocolError::BadObject));
    let huge = QrObject::Signature(Signed::AccountAuthorization {
        account: [1; 32],
        nonce: [2; 32],
        signature: vec![3; 70_000],
    });
    assert!(matches!(
        encode_object(&huge),
        Err(ProtocolError::TooLarge { .. })
    ));
}

#[test]
fn a_public_key_travels_with_its_scheme() {
    let object = QrObject::PublicKey {
        scheme: 5,
        key: vec![4; 1312],
    };
    let encoded = encode_object(&object).unwrap();
    assert_eq!(decode_object(&encoded).unwrap(), object);
    assert_eq!(
        encode_object(&QrObject::PublicKey {
            scheme: 5,
            key: Vec::new(),
        }),
        Err(ProtocolError::BadObject)
    );
}

#[test]
fn wallet_keys_round_trip_with_their_roles_and_labels() {
    let object = QrObject::WalletKeys(vec![
        WalletKey {
            role: KeyRole::Address,
            label: "Cold".into(),
            key: vec![7; PUBKEY_BYTES],
        },
        WalletKey {
            role: KeyRole::Vector { scheme: 5 },
            label: String::new(),
            key: vec![4; 1312],
        },
    ]);
    let encoded = encode_object(&object).unwrap();
    assert_eq!(decode_object(&encoded).unwrap(), object);
}

#[test]
fn a_wallet_key_label_is_cut_on_a_character_boundary() {
    let objects = encode_wallet_keys(&[WalletKey {
        role: KeyRole::Address,
        label: "é".repeat(40),
        key: vec![1; PUBKEY_BYTES],
    }])
    .unwrap();
    let QrObject::WalletKeys(keys) = &objects[0] else {
        panic!("wallet keys");
    };
    assert_eq!(keys[0].label, "é".repeat(MAX_KEY_LABEL / 2));
}

#[test]
fn malformed_wallet_keys_are_refused() {
    let one = |role: KeyRole, label: &str, key: Vec<u8>| {
        encode_object(&QrObject::WalletKeys(vec![WalletKey {
            role,
            label: label.into(),
            key,
        }]))
    };
    // An address is 32 bytes.
    assert_eq!(
        one(KeyRole::Address, "x", vec![1; 31]),
        Err(ProtocolError::BadObject)
    );
    // No key at all.
    assert_eq!(
        one(KeyRole::Vector { scheme: 1 }, "x", Vec::new()),
        Err(ProtocolError::BadObject)
    );
    // A label longer than a label may be.
    assert_eq!(
        one(
            KeyRole::Address,
            &"x".repeat(MAX_KEY_LABEL + 1),
            vec![1; 32]
        ),
        Err(ProtocolError::BadObject)
    );
    // No entries.
    assert_eq!(
        encode_object(&QrObject::WalletKeys(Vec::new())),
        Err(ProtocolError::BadObject)
    );

    let good = one(KeyRole::Address, "x", vec![1; 32]).unwrap();
    // Count, kind, entry count, role, then the label's length and byte.
    let role = 2 + 1 + 2;
    let mut unknown = good.clone();
    unknown[role] = 9;
    assert_eq!(decode_object(&unknown), Err(ProtocolError::BadObject));
    let mut bad_label = good;
    bad_label[role + 3] = 0xff;
    assert_eq!(decode_object(&bad_label), Err(ProtocolError::BadObject));
}

#[test]
fn many_wallet_keys_split_into_payloads_that_each_fit_a_transfer() {
    let keys: Vec<WalletKey> = (0..30u8)
        .map(|n| WalletKey {
            role: KeyRole::Vector { scheme: 5 },
            label: format!("Quantum {n}"),
            key: vec![n; 1312],
        })
        .collect();
    let objects = encode_wallet_keys(&keys).unwrap();
    assert!(objects.len() > 1);
    let mut back = Vec::new();
    for object in &objects {
        let payload = encode_object(object).unwrap();
        assert!(encode_frames(&payload, DEFAULT_CHUNK).is_ok());
        let QrObject::WalletKeys(part) = decode_object(&payload).unwrap() else {
            panic!("wallet keys");
        };
        back.extend(part);
    }
    assert_eq!(back, keys);

    let small: Vec<WalletKey> = (0..40u8)
        .map(|n| WalletKey {
            role: KeyRole::Address,
            label: String::new(),
            key: vec![n; PUBKEY_BYTES],
        })
        .collect();
    assert_eq!(encode_wallet_keys(&small).unwrap().len(), 2);
    assert!(encode_wallet_keys(&[]).unwrap().is_empty());
}

#[test]
fn a_key_rotation_round_trips_and_fits_a_transfer() {
    let rotation = KeyRotation {
        account: [3; PUBKEY_BYTES],
        scheme: 7,
        next_key: vec![9; 41],
        signed: (0..3u8)
            .map(|n| RotationSignature {
                nonce: [n; 32],
                signature: vec![n; 1037],
            })
            .collect(),
    };
    let object = QrObject::KeyRotation(rotation.clone());
    let encoded = encode_object(&object).unwrap();
    assert_eq!(decode_object(&transfer(&encoded)).unwrap(), object);

    assert_eq!(
        encode_object(&QrObject::KeyRotation(KeyRotation {
            signed: Vec::new(),
            ..rotation
        })),
        Err(ProtocolError::BadObject)
    );
}

#[test]
fn an_account_state_round_trips() {
    let object = QrObject::AccountState(AccountState {
        account: [3; PUBKEY_BYTES],
        scheme: 7,
        nonce: [5; 32],
        leaf_floor: 200,
        key: vec![9; 41],
    });
    let encoded = encode_object(&object).unwrap();
    assert_eq!(decode_object(&encoded).unwrap(), object);
}

/// One of every kind, each field set to something it can be told apart by.
fn every_kind() -> Vec<QrObject> {
    vec![
        QrObject::Transaction(v1_wire(40)),
        QrObject::Signature(Signed::Transaction {
            pubkey: [1; 32],
            signature: [2; 64],
        }),
        QrObject::Signature(Signed::Authorization {
            authority: [3; 32],
            signature: [4; 64],
        }),
        QrObject::Signature(Signed::AccountAuthorization {
            account: [5; 32],
            nonce: [6; 32],
            signature: vec![7; 2420],
        }),
        QrObject::Context(TxContext {
            message_sha256: [8; 32],
            pins: vec![ContextPin {
                program_id: [9; 32],
                idl_sha256: [10; 32],
            }],
            authorizations: vec![ContextAuthorization {
                account: [11; 32],
                scheme: 3,
                nonce: [12; 32],
                leaf_floor: 13,
            }],
            accounts: vec![ContextAccount {
                pubkey: [14; 32],
                data: vec![15; 20],
            }],
        }),
        QrObject::PublicKey {
            scheme: 5,
            key: vec![16; 897],
        },
        QrObject::WalletKeys(vec![
            WalletKey {
                role: KeyRole::Address,
                label: "Cold".into(),
                key: vec![17; PUBKEY_BYTES],
            },
            WalletKey {
                role: KeyRole::Vector { scheme: 6 },
                label: String::new(),
                key: vec![18; 41],
            },
        ]),
        QrObject::KeyRotation(KeyRotation {
            account: [19; 32],
            scheme: 6,
            next_key: vec![20; 41],
            signed: vec![RotationSignature {
                nonce: [21; 32],
                signature: vec![22; 100],
            }],
        }),
        QrObject::AccountState(AccountState {
            account: [23; 32],
            scheme: 6,
            nonce: [24; 32],
            leaf_floor: 25,
            key: vec![26; 41],
        }),
    ]
}

#[test]
fn every_kind_round_trips_as_json_alone_and_together() {
    let all = every_kind();
    for object in &all {
        let json = payload_json(std::slice::from_ref(object)).unwrap();
        assert!(json.trim_start().starts_with('{'), "{json}");
        assert_eq!(parse_payload_json(&json).unwrap(), vec![object.clone()]);
    }
    for pair in all.chunks(2) {
        let json = payload_json(pair).unwrap();
        if pair.len() > 1 {
            assert!(json.trim_start().starts_with('['), "{json}");
        }
        assert_eq!(parse_payload_json(&json).unwrap(), pair);
    }
}

#[test]
fn json_names_keys_as_solana_prints_them() {
    let object = QrObject::Signature(Signed::Transaction {
        pubkey: [1; 32],
        signature: [2; 64],
    });
    let json = payload_json(&[object]).unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        value,
        serde_json::json!({
            "type": "signature",
            "signs": "transaction",
            "pubkey": bs58::encode([1u8; 32]).into_string(),
            "signature": bs58::encode([2u8; 64]).into_string(),
        })
    );

    let note = QrObject::Context(TxContext::new([8; 32], &[], &[]).unwrap());
    let value: serde_json::Value = serde_json::from_str(&payload_json(&[note]).unwrap()).unwrap();
    assert_eq!(value["type"], "contextNote");
    assert_eq!(value["message"], hex::encode([8u8; 32]));

    let keys = QrObject::WalletKeys(vec![WalletKey {
        role: KeyRole::Address,
        label: "Cold".into(),
        key: vec![17; PUBKEY_BYTES],
    }]);
    let value: serde_json::Value = serde_json::from_str(&payload_json(&[keys]).unwrap()).unwrap();
    assert_eq!(
        value["keys"][0],
        serde_json::json!({
            "role": "address",
            "label": "Cold",
            "key": bs58::encode([17u8; 32]).into_string(),
        })
    );
}

#[test]
fn json_that_breaks_a_rule_is_refused() {
    let refused = |text: &str| {
        assert_eq!(
            parse_payload_json(text),
            Err(ProtocolError::BadObject),
            "{text}"
        )
    };
    let pubkey = bs58::encode([1u8; 32]).into_string();
    let signature = bs58::encode([2u8; 64]).into_string();
    // A kind this build does not know.
    refused(r#"{"type":"poem"}"#);
    // A field it does not know.
    refused(&format!(
        r#"{{"type":"signature","signs":"transaction","pubkey":"{pubkey}","signature":"{signature}","extra":1}}"#
    ));
    // An address that is not 32 bytes.
    refused(&format!(
        r#"{{"type":"signature","signs":"transaction","pubkey":"{signature}","signature":"{signature}"}}"#
    ));
    // A v1 transaction starts with 0x81.
    refused(r#"{"type":"transaction","wire":"AQID"}"#);
    // An empty list of objects, and an empty key list.
    assert_eq!(parse_payload_json("[]"), Err(ProtocolError::Empty));
    refused(r#"{"type":"walletKeys","keys":[]}"#);
    refused("{ not json");
}
