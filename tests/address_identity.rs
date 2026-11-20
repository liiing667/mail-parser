/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: Apache-2.0 OR MIT
 */

use mail_parser::{Address, AddressList, Message, MessageParser};

fn parse_to(header: &str) -> Message<'static> {
    let raw = format!("To: {header}\r\n\r\n").into_bytes();
    MessageParser::new().parse_owned(raw).expect("a message")
}

fn mailboxes<'m>(list: &AddressList<'m>) -> Vec<(Option<&'m str>, Option<&'m str>)> {
    list.mailboxes()
        .map(|mailbox| (mailbox.name(), mailbox.address()))
        .collect()
}

fn addresses<'m>(list: &AddressList<'m>) -> Vec<&'m str> {
    list.mailboxes()
        .filter_map(|mailbox| mailbox.address())
        .collect()
}

#[test]
fn plain_to_cc_bcc_are_unchanged() {
    let message = MessageParser::new()
        .parse_owned(
            b"To: Ann <ann@example.com>, bob@example.com\r\n\
               Cc: \"Smith, John\" <john@example.com>\r\n\
               Bcc: cy@example.com (Cy)\r\n\
               \r\n"
                .to_vec(),
        )
        .expect("a message");
    assert_eq!(
        mailboxes(&message.to().expect("to")),
        [
            (Some("Ann"), Some("ann@example.com")),
            (None, Some("bob@example.com"))
        ]
    );
    assert_eq!(
        mailboxes(&message.cc().expect("cc")),
        [(Some("Smith, John"), Some("john@example.com"))]
    );
    assert_eq!(
        mailboxes(&message.bcc().expect("bcc")),
        [(Some("Cy"), Some("cy@example.com"))]
    );
}

#[test]
fn legal_groups_are_preserved() {
    let message = parse_to("Friends: Ann <ann@example.com>, bob@example.com;, cy@example.com");
    let list = message.to().expect("to");
    let items: Vec<_> = list.iter().collect();
    assert_eq!(items.len(), 2);
    let Address::Group(group) = items[0] else {
        panic!("expected a group");
    };
    assert_eq!(group.name(), Some("Friends"));
    assert_eq!(
        group
            .mailboxes()
            .map(|mailbox| (mailbox.name(), mailbox.address()))
            .collect::<Vec<_>>(),
        [
            (Some("Ann"), Some("ann@example.com")),
            (None, Some("bob@example.com"))
        ]
    );
    assert!(matches!(items[1], Address::Mailbox(_)));
    assert_eq!(
        addresses(&list),
        ["ann@example.com", "bob@example.com", "cy@example.com"]
    );
    assert!(list.has_groups());
}

#[test]
fn local_domains_are_preserved() {
    let message = parse_to("root@localhost, postmaster@mail, ops@[127.0.0.1]");
    let list = message.to().expect("to");
    assert_eq!(
        addresses(&list),
        ["root@localhost", "postmaster@mail", "ops@[127.0.0.1]"]
    );
    assert!(list.contains("root@localhost"));
    assert!(list.contains("ROOT@LOCALHOST"));
    // `localhost.` is the absolute form of the local domain.
    assert!(list.contains("root@localhost."));
}

#[test]
fn comments_cannot_move_the_address_into_the_name() {
    for header in [
        "a(comment)@b.c",
        "a@(comment)b.c",
        "a@b.c(comment)",
        "(comment)a@b.c",
        "<a(comment)@b.c>",
        "<a@b.c>(comment)",
    ] {
        let message = parse_to(header);
        let list = message.to().expect("to");
        assert_eq!(
            addresses(&list),
            ["a@b.c"],
            "{header:?} must keep the address identity"
        );
        assert!(list.contains("a@b.c"), "{header:?}");
    }
    // The comment text is still available as the display name.
    assert_eq!(
        mailboxes(&parse_to("a(comment)@b.c").to().expect("to")),
        [(Some("comment"), Some("a@b.c"))]
    );
    // A comment between a display name and the address is untouched.
    assert_eq!(
        mailboxes(&parse_to("Bob(comment)a@b.c").to().expect("to")),
        [(Some("Bob (comment)"), Some("a@b.c"))]
    );
    // Inside a group the same rule applies.
    let message = parse_to("g: a(comment)@b.c;");
    let list = message.to().expect("to");
    assert!(list.contains("a@b.c"));
    assert_eq!(addresses(&list), ["a@b.c"]);
}

#[test]
fn quotes_cannot_split_the_address() {
    // Quoted local parts keep their special characters inside angle brackets.
    for (header, address) in [
        ("<\"a>b\"@x.y>", "\"a>b\"@x.y"),
        ("<\"a;b\"@x.y>", "\"a;b\"@x.y"),
        ("<\"a(b\"@x.y>", "\"a(b\"@x.y"),
        ("<\"a\\\"b\"@x.y>", "\"a\\\"b\"@x.y"),
        ("\"john smith\"@x.y", "\"john smith\"@x.y"),
    ] {
        let message = parse_to(header);
        let list = message.to().expect("to");
        assert_eq!(addresses(&list), [address], "{header:?}");
        assert!(list.contains(address), "{header:?}");
    }
}

#[test]
fn group_syntax_cannot_hide_an_address() {
    // A bare address before `:` is not a legal group name: both
    // addresses stay mailboxes instead of one becoming a group name.
    let message = parse_to("alice@x.test: bob@y.test;");
    let list = message.to().expect("to");
    assert_eq!(addresses(&list), ["alice@x.test", "bob@y.test"]);
    assert!(list.contains("alice@x.test"));
    assert!(list.contains("bob@y.test"));

    // A group tail inside angle brackets is not part of the address.
    for header in ["<a@b.c;>", "<a@b.c; >"] {
        let message = parse_to(header);
        let list = message.to().expect("to");
        assert_eq!(addresses(&list), ["a@b.c"], "{header:?}");
        assert!(list.contains("a@b.c"), "{header:?}");
    }

    // A `;` inside a URL or a quoted local part is left alone.
    for (header, address) in [
        ("<mailto:a@b.c?x=1;y=2>", "mailto:a@b.c?x=1;y=2"),
        ("<http://example.com/a;b>", "http://example.com/a;b"),
    ] {
        let message = parse_to(header);
        assert_eq!(
            addresses(&message.to().expect("to")),
            [address],
            "{header:?}"
        );
    }
}

#[test]
fn trailing_dot_is_the_same_identity() {
    let message = parse_to("alice@example.com.");
    let list = message.to().expect("to");
    // The parsed mailbox keeps the address as written...
    assert_eq!(addresses(&list), ["alice@example.com."]);
    // ...but the comparison identity ignores one trailing dot.
    assert!(list.contains("alice@example.com"));
    assert!(list.contains("alice@example.com."));
    assert!(list.contains("ALICE@EXAMPLE.COM"));
    assert!(!list.contains("alice@example.com.."));
    let message = parse_to("alice@example.com");
    let list = message.to().expect("to");
    assert!(list.contains("alice@example.com."));
}

#[test]
fn duplicates_in_a_group_compare_item_by_item() {
    let message = parse_to("g: a@b.c, a@b.c, B <a@b.c>;, a@b.c");
    let list = message.to().expect("to");
    let Address::Group(group) = list.iter().next().expect("a group") else {
        panic!("expected a group");
    };
    assert_eq!(
        group
            .mailboxes()
            .map(|mailbox| (mailbox.name(), mailbox.address()))
            .collect::<Vec<_>>(),
        [
            (None, Some("a@b.c")),
            (None, Some("a@b.c")),
            (Some("B"), Some("a@b.c"))
        ]
    );
    assert_eq!(group.len(), 3);
    // Flattening keeps the duplicates and the mailbox after the group.
    assert_eq!(addresses(&list), ["a@b.c", "a@b.c", "a@b.c", "a@b.c"]);
    assert!(list.contains("a@b.c"));
}

#[test]
fn unicode_variants_compare_item_by_item() {
    let nfc = "b\u{f6}b@example.com";
    let nfd = "bo\u{308}b@example.com";
    let message = parse_to(&format!("{nfc}, {nfd}, user@ex\u{e1}mple.com"));
    let list = message.to().expect("to");
    // Each variant is a distinct mailbox, in document order.
    assert_eq!(addresses(&list), [nfc, nfd, "user@ex\u{e1}mple.com"]);
    // Comparison is exact (plus ASCII case): variants do not merge.
    assert!(list.contains(nfc));
    assert!(list.contains(nfd));
    assert!(list.contains("B\u{f6}B@EXAMPLE.COM"));
    assert!(!list.contains("bob@example.com"));
    assert!(!list.contains("user@example.com"));
    // The trailing-dot rule applies to internationalized domains too.
    let message = parse_to("user@ex\u{e1}mple.com.");
    assert!(message.to().expect("to").contains("user@ex\u{e1}mple.com"));
}
