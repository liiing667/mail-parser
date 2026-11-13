/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 * SPDX-License-Identifier: Apache-2.0 OR MIT
 */

mod support;

use mail_parser::{DecodeProblems, Encoding, MessageParser, PartKind};
use support::fixture;

fn parse(name: &str) -> mail_parser::Message<'static> {
    MessageParser::new()
        .parse_owned(fixture(name))
        .expect("the fixture parses")
}

fn part_by_name<'m>(
    message: &'m mail_parser::Message<'_>,
    name: &str,
) -> mail_parser::MessagePart<'m> {
    message
        .parts()
        .find(|part| part.attachment_name() == Some(name))
        .expect("named part")
}

#[test]
fn base64_binary_is_bounded_and_exact() {
    let message = parse("bounded-base64.eml");
    let attachment = part_by_name(&message, "blob.bin");
    assert_eq!(attachment.encoding(), Encoding::Base64);
    let full = attachment.decoded();
    assert_eq!(full.len(), 512);

    for max in [0usize, 1, 2, 3, 4, 7, 63, 64, 100, 511, 512, 1024] {
        let limited = attachment.decoded_with_limit(max);
        if max >= full.len() {
            assert!(!limited.truncated, "max {max}");
            assert!(limited.bytes.len() <= max);
            assert_eq!(limited.bytes.as_ref(), full.as_ref());
            assert!(!limited.problems.contains(DecodeProblems::TRUNCATED));
        } else {
            assert!(limited.truncated, "max {max}");
            assert_eq!(limited.bytes.len(), max, "max {max}");
            assert_eq!(limited.bytes.as_ref(), &full[..max], "max {max}");
            assert!(limited.problems.contains(DecodeProblems::TRUNCATED));
        }
        assert!(
            !limited
                .problems
                .intersects(DecodeProblems::MALFORMED_TRANSFER_ENCODING)
        );
    }
}

#[test]
fn base64_text_keeps_character_boundaries() {
    let message = parse("bounded-base64.eml");
    let part = message
        .parts()
        .find(|part| part.is_text() && part.encoding() == Encoding::Base64)
        .expect("base64 text part");
    let full = part.text().expect("text");
    assert_eq!(full.len(), 180);

    for max in [0usize, 1, 2, 3, 5, 10, 31, 64, 127, 179, 180, 1000] {
        let limited = part.text_with_limit(max).expect("a text part");
        assert!(limited.text.len() <= max, "max {max}");
        assert!(
            full.starts_with(limited.text.as_ref()),
            "max {max}: {:?} not a prefix of {:?}",
            limited.text,
            full
        );
        if max >= full.len() {
            assert!(!limited.truncated, "max {max}");
            assert_eq!(limited.text, full);
        } else {
            assert!(limited.truncated, "max {max}");
            assert!(limited.problems.contains(DecodeProblems::TRUNCATED));
        }
        std::str::from_utf8(limited.text.as_bytes()).expect("valid UTF-8");
    }

    let zero = part.decoded_with_limit(0);
    assert!(zero.bytes.is_empty());
    assert!(zero.truncated);
    let zero_text = part.text_with_limit(0).expect("a text part");
    assert!(zero_text.text.is_empty());
    assert!(zero_text.truncated);
}

#[test]
fn malformed_base64_is_flagged_inside_the_prefix() {
    let message = parse("bounded-base64.eml");
    let part = message
        .parts()
        .find(|part| {
            part.is_text()
                && part
                    .decoded_with_limit(usize::MAX)
                    .problems
                    .contains(DecodeProblems::MALFORMED_TRANSFER_ENCODING)
        })
        .expect("malformed part");

    let checked = part.decoded_checked();
    assert!(
        checked
            .1
            .contains(DecodeProblems::MALFORMED_TRANSFER_ENCODING)
    );

    let limited = part.decoded_with_limit(usize::MAX);
    assert!(!limited.truncated);
    assert!(
        limited
            .problems
            .contains(DecodeProblems::MALFORMED_TRANSFER_ENCODING)
    );
    assert_eq!(limited.bytes, checked.0);

    let small = part.decoded_with_limit(4);
    assert!(small.truncated);
    assert_eq!(small.bytes.as_ref(), &checked.0[..4]);

    for max in 0..=checked.0.len() {
        let limited = part.decoded_with_limit(max);
        assert!(limited.bytes.len() <= max);
        assert_eq!(
            limited.bytes.as_ref(),
            &checked.0[..checked.0.len().min(max)],
            "budget {max}"
        );
    }
}

#[test]
fn quoted_printable_is_bounded_and_checked() {
    let message = parse("bounded-quoted-printable.eml");
    let part = message.root_part();
    assert_eq!(part.encoding(), Encoding::QuotedPrintable);
    let (full, full_problems) = part.decoded_checked();
    assert!(full_problems.contains(DecodeProblems::MALFORMED_TRANSFER_ENCODING));

    for max in [
        0usize,
        1,
        2,
        7,
        16,
        40,
        80,
        full.len() - 1,
        full.len(),
        4096,
    ] {
        let limited = part.decoded_with_limit(max);
        assert!(limited.bytes.len() <= max, "max {max}");
        if max >= full.len() {
            assert!(!limited.truncated, "max {max}");
            assert_eq!(limited.bytes, full);
            assert!(
                limited
                    .problems
                    .contains(DecodeProblems::MALFORMED_TRANSFER_ENCODING)
            );
        } else {
            assert!(limited.truncated, "max {max}");
            assert_eq!(limited.bytes.as_ref(), &full[..limited.bytes.len()]);
            assert!(limited.problems.contains(DecodeProblems::TRUNCATED));
        }
    }

    let limited_text = part.text_with_limit(20).expect("a text part");
    assert!(limited_text.text.len() <= 20);
    assert!(limited_text.truncated);
    assert!(
        part.text()
            .expect("text")
            .starts_with(limited_text.text.as_ref())
    );
    std::str::from_utf8(limited_text.text.as_bytes()).expect("valid UTF-8");

    for max in 0..=full.len() {
        let limited = part.decoded_with_limit(max);
        assert!(limited.bytes.len() <= max, "budget {max}");
        let end = full.len().min(limited.bytes.len());
        assert_eq!(limited.bytes.as_ref(), &full[..end], "budget {max}");
        assert_eq!(limited.truncated, end < full.len(), "budget {max}");
    }
}

#[test]
fn nested_rfc822_bounded_decode() {
    let message = parse("bounded-nested-rfc822.eml");
    let container = message
        .parts()
        .find(|part| part.is_message())
        .expect("message/rfc822 part");
    assert_eq!(container.encoding(), Encoding::Base64);

    let nested = container.nested().expect("nested message");
    assert_eq!(nested.subject(), Some("inner"));
    let inner_text = nested.text_body().next().expect("inner text part");
    let full = inner_text.text().expect("inner text");

    for max in [0usize, 1, 8, 32, 64, full.len() - 1, full.len(), 9999] {
        let limited = inner_text.text_with_limit(max).expect("a text part");
        assert!(limited.text.len() <= max, "max {max}");
        assert!(full.starts_with(limited.text.as_ref()), "max {max}");
        assert_eq!(limited.truncated, max < full.len(), "max {max}");
        assert_eq!(
            limited.problems.contains(DecodeProblems::TRUNCATED),
            max < full.len(),
            "max {max}"
        );
    }

    let limited_container = container.decoded_with_limit(32);
    assert!(limited_container.truncated);
    assert_eq!(limited_container.bytes.len(), 32);
    assert_eq!(limited_container.bytes.as_ref(), &container.decoded()[..32]);
}

#[test]
fn bounded_decode_never_exceeds_budget_on_random_windows() {
    let message = parse("bounded-base64.eml");
    let attachment = part_by_name(&message, "blob.bin");
    let full = attachment.decoded();

    let mut state = 0x1234_5678_9abc_def0u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..200 {
        let max = (next() % (full.len() as u64 + 10)) as usize;
        let limited = attachment.decoded_with_limit(max);
        assert!(limited.bytes.len() <= max, "budget {max}");
        if max < full.len() {
            assert!(limited.truncated, "budget {max}");
            assert_eq!(limited.bytes.as_ref(), &full[..max]);
        }
    }
}

#[test]
fn full_decode_apis_keep_their_behaviour() {
    for name in [
        "bounded-base64.eml",
        "bounded-quoted-printable.eml",
        "bounded-nested-rfc822.eml",
    ] {
        let message = parse(name);
        for part in message.parts() {
            let bounded = part.decoded_with_limit(usize::MAX);
            assert!(!bounded.truncated, "{name} part {}", part.id());
            assert_eq!(bounded.bytes, part.decoded(), "{name}");
            if part.is_text() {
                let text = part.text_with_limit(usize::MAX).expect("a text part");
                assert!(!text.truncated);
                assert_eq!(text.text, part.text().expect("text"), "{name}");
            } else {
                assert!(part.text_with_limit(16).is_none());
            }
        }
    }
}

#[test]
fn unknown_charset_and_encoding_are_reported_with_truncation() {
    let raw = concat!(
        "Content-Type: text/plain; charset=x-unknown\r\n",
        "Content-Transfer-Encoding: base64\r\n",
        "\r\n",
        "Y2Fmw6ktdW5rbm93bi1jaGFyc2V0LWRhdGEtZm9yLWJvdW5kZWQtZGVjb2Rlcg==\r\n"
    );
    let message = MessageParser::new().parse(raw).expect("message");
    let part = message.root_part();
    let limited = part.text_with_limit(8).expect("a text part");
    assert!(limited.truncated);
    assert!(limited.problems.contains(DecodeProblems::UNKNOWN_CHARSET));
    assert!(limited.problems.contains(DecodeProblems::TRUNCATED));
    assert!(limited.text.len() <= 8);
    std::str::from_utf8(limited.text.as_bytes()).expect("valid UTF-8");
}

#[test]
fn non_text_parts_have_no_limited_text() {
    let message = parse("bounded-base64.eml");
    let attachment = part_by_name(&message, "blob.bin");
    assert!(matches!(
        attachment.kind(),
        PartKind::Binary | PartKind::InlineBinary
    ));
    assert!(attachment.text_with_limit(100).is_none());
}

#[test]
fn qp_without_line_breaks_still_decodes_a_prefix() {
    let raw = "Content-Type: text/plain; charset=iso-8859-1\r\n\
Content-Transfer-Encoding: quoted-printable\r\n\r\nCaf=E9 cr=E8me";
    let message = MessageParser::new().parse(raw.as_bytes()).expect("message");
    let part = message.root_part();
    let full = part.decoded();
    let limited = part.decoded_with_limit(4);
    assert_eq!(limited.bytes.as_ref(), &full[..4]);
    assert!(limited.truncated);
    let text = part.text_with_limit(4).expect("a text part");
    assert_eq!(text.text.as_ref(), "Caf\u{e9}");
    assert!(text.truncated);
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

#[test]
fn random_base64_bodies_match_full_decode_prefix() {
    let mut rng = Rng(0xa56d_5f1d_c005_0a9d);
    for _ in 0..300 {
        let len = rng.below(400);
        let symbols = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut body = Vec::with_capacity(len);
        for _ in 0..len {
            body.push(symbols[rng.below(symbols.len())]);
        }
        let mut encoded = Vec::new();
        for (index, chunk) in body.chunks(48).enumerate() {
            if index > 0 {
                encoded.extend_from_slice(b"\r\n");
            }
            encoded.extend_from_slice(chunk);
        }
        let mut message_raw =
            b"Content-Type: application/octet-stream\r\nContent-Transfer-Encoding: base64\r\n\r\n"
                .to_vec();
        message_raw.extend_from_slice(&encoded);
        let message = MessageParser::new().parse(&message_raw).expect("message");
        let part = message.root_part();
        let full = part.decoded();
        for _ in 0..6 {
            let max = rng.below(full.len() + 5);
            let limited = part.decoded_with_limit(max);
            assert!(limited.bytes.len() <= max, "budget {max}");
            let expected = full.len().min(max);
            assert_eq!(limited.bytes.len(), expected, "budget {max}");
            assert_eq!(limited.bytes.as_ref(), &full[..expected]);
            assert_eq!(
                limited.truncated,
                limited.bytes.len() < full.len(),
                "budget {max}"
            );
            assert!(
                !limited
                    .problems
                    .intersects(DecodeProblems::MALFORMED_TRANSFER_ENCODING)
            );
        }
    }
}

#[test]
fn random_qp_bodies_match_full_decode_prefix() {
    let mut rng = Rng(0x1234_df92_6081_e436);
    for _ in 0..300 {
        let len = rng.below(300);
        let alphabet = b"ab \t\r\n=\xc3\xa9";
        let mut body = Vec::with_capacity(len);
        for _ in 0..len {
            body.push(alphabet[rng.below(alphabet.len())]);
        }
        let raw = [
            b"Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n"
                .as_slice(),
            &body,
        ]
        .concat();
        let message = MessageParser::new().parse(&raw).expect("message");
        let part = message.root_part();
        let full = part.decoded();
        for _ in 0..6 {
            let max = rng.below(full.len() + 5);
            let limited = part.decoded_with_limit(max);
            assert!(limited.bytes.len() <= max, "budget {max}");
            let end = full.len().min(limited.bytes.len());
            assert_eq!(limited.bytes.as_ref(), &full[..end]);
            assert_eq!(
                limited.truncated,
                limited.bytes.len() < full.len(),
                "budget {max}"
            );
        }
    }
}

#[test]
fn multibyte_utf8_is_never_split() {
    let payload = "éñ中🎉".repeat(40);
    let encoded = base64_encode(payload.as_bytes());
    let raw = format!(
        "Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n{encoded}\r\n"
    );
    let message = MessageParser::new().parse(raw.as_bytes()).expect("message");
    let part = message.root_part();
    for max in 0..payload.len() + 4 {
        let limited = part.text_with_limit(max).expect("a text part");
        assert!(limited.text.len() <= max, "budget {max}");
        assert!(payload.starts_with(limited.text.as_ref()), "budget {max}");
        assert!(!limited.text.contains('\u{fffd}'), "budget {max}");
        if max < payload.len() {
            assert!(limited.truncated, "budget {max}");
        }
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[((b0 & 0x03) << 4 | b1 >> 4) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((b1 & 0x0f) << 2 | b2 >> 6) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(b2 & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}
