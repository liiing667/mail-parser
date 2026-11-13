/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: Apache-2.0 OR MIT
 */

//! Transfer decoding up to a byte budget, without expanding a body that
//! does not fit. [`bounded_transfer`] decodes only the prefix of a body
//! whose transfer-decoded length fits `max_bytes`, and reports whether the
//! body was cut and whether the decoded prefix is malformed.

use super::charsets::Charset;
use crate::Encoding;
use encodify::{base64, qp};
use memchr::memrchr;
use std::borrow::Cow;

/// How much slack a chunk may add past the byte budget: base64 emits at
/// most three bytes per quantum, quoted-printable at most one byte per
/// input byte.
const CHUNK_SLACK: usize = 4;

/// A transfer-decoded body prefix bounded by a byte budget.
pub(crate) struct BoundedBytes<'x> {
    /// The decoded prefix, at most a few bytes longer than the budget while
    /// decoding, then cut back to it.
    pub(crate) bytes: Cow<'x, [u8]>,
    /// More bytes were transfer-encoded in the body than fit the budget.
    pub(crate) truncated: bool,
    /// The decoded prefix contains a malformed transfer encoding: a byte
    /// outside the base64 alphabet, or a malformed quoted-printable escape.
    pub(crate) malformed: bool,
}

/// Decodes `raw` in `encoding` up to `max_bytes` of output, keeping the
/// work and the allocation proportional to `max_bytes` rather than to the
/// body size.
pub(crate) fn bounded_transfer(
    raw: &[u8],
    encoding: Encoding,
    max_bytes: usize,
) -> BoundedBytes<'_> {
    if raw.len() <= max_bytes {
        let (bytes, malformed) = encoding.decode_checked(raw);
        return BoundedBytes {
            bytes,
            truncated: false,
            malformed,
        };
    }
    match encoding {
        Encoding::None => BoundedBytes {
            bytes: Cow::Borrowed(raw.get(..max_bytes).unwrap_or(raw)),
            truncated: true,
            malformed: false,
        },
        Encoding::Base64 => bounded_base64(raw, max_bytes),
        Encoding::QuotedPrintable => bounded_qp(raw, max_bytes),
    }
}

fn bounded_base64(raw: &[u8], max_bytes: usize) -> BoundedBytes<'_> {
    let decoded_len = base64::MIME
        .decoded_len(raw)
        .unwrap_or_else(|_| Encoding::Base64.decoded_len(raw));
    if decoded_len <= max_bytes {
        let (bytes, malformed) = Encoding::Base64.decode_checked(raw);
        return BoundedBytes {
            bytes,
            truncated: false,
            malformed,
        };
    }
    let mut out = Vec::with_capacity(max_bytes.saturating_add(CHUNK_SLACK));
    let mut decoder = base64::MIME.decoder();
    let mut malformed = false;
    let mut rest = raw;
    'decode: loop {
        if out.len() >= max_bytes {
            out.truncate(max_bytes);
            return BoundedBytes {
                bytes: Cow::Owned(out),
                truncated: decoded_len > max_bytes,
                malformed,
            };
        }
        let remaining = max_bytes.saturating_sub(out.len());
        let take = (remaining / 3 * 4).saturating_add(4);
        let (chunk, tail) = if rest.len() > take {
            rest.split_at(take)
        } else {
            (rest, &[][..])
        };
        let mut offset = 0;
        loop {
            let consumed = decoder.decode_symbols(chunk.get(offset..).unwrap_or(&[]), &mut out);
            offset += consumed;
            match chunk.get(offset) {
                None => break,
                Some(b'=') => {
                    decoder.pad(&mut out);
                    offset += 1;
                }
                Some(&byte) if byte.is_ascii_whitespace() => offset += 1,
                Some(_) => {
                    malformed = true;
                    offset += 1;
                }
            }
            if out.len() >= max_bytes {
                break 'decode;
            }
        }
        if tail.is_empty() {
            decoder.finish(&mut out);
            break;
        }
        rest = tail;
    }
    out.truncate(max_bytes);
    BoundedBytes {
        bytes: Cow::Owned(out),
        truncated: decoded_len > max_bytes,
        malformed,
    }
}

fn bounded_qp(raw: &[u8], max_bytes: usize) -> BoundedBytes<'_> {
    let decoded_len = qp::BODY
        .strict()
        .decoded_len(raw)
        .unwrap_or_else(|_| Encoding::QuotedPrintable.decoded_len(raw));
    if decoded_len <= max_bytes {
        let (bytes, malformed) = Encoding::QuotedPrintable.decode_checked(raw);
        return BoundedBytes {
            bytes,
            truncated: false,
            malformed,
        };
    }
    let mut window = max_bytes.saturating_add(256).min(raw.len());
    loop {
        let prefix = raw.get(..window).unwrap_or(raw);
        let whole = window >= raw.len();
        let lines = prefix
            .get(..memrchr(b'\n', prefix).map_or(0, |end| end + 1))
            .unwrap_or_default();
        let lines = if lines.is_empty() && !prefix.is_empty() {
            prefix
        } else {
            lines
        };
        let (bytes, malformed) = match qp::BODY.strict().decode(lines) {
            Ok(decoded) => (decoded, false),
            Err(_) => (qp::BODY.decode(lines).unwrap_or(Cow::Borrowed(lines)), true),
        };
        if bytes.len() > max_bytes || whole {
            let bytes = match bytes {
                Cow::Borrowed(slice) => Cow::Borrowed(slice.get(..max_bytes).unwrap_or(slice)),
                Cow::Owned(mut vec) => {
                    vec.truncate(max_bytes);
                    Cow::Owned(vec)
                }
            };
            return BoundedBytes {
                bytes,
                truncated: decoded_len > max_bytes,
                malformed,
            };
        }
        window = window.saturating_mul(2).min(raw.len());
        if window == prefix.len() && !whole {
            window = raw.len();
        }
    }
}

/// A text prefix bounded by a byte budget, cut on a character boundary.
pub(crate) struct BoundedText<'x> {
    /// The longest prefix of the decoded text at most `max_bytes` long,
    /// ending on a character boundary.
    pub(crate) text: Cow<'x, str>,
    /// The body had more bytes (transfer-decoded, then charset-decoded)
    /// than the budget allowed.
    pub(crate) truncated: bool,
    /// The decoded prefix contains a malformed transfer encoding.
    pub(crate) malformed_transfer: bool,
    /// The decoded prefix contains sequences that are not valid in the
    /// charset, replaced by U+FFFD.
    pub(crate) malformed_charset: bool,
}

/// Transfer-decodes a text body up to `max_bytes` and converts the prefix
/// from `charset`, never splitting a character.
pub(crate) fn bounded_text(
    raw: &[u8],
    encoding: Encoding,
    charset: Charset,
    max_bytes: usize,
) -> BoundedText<'_> {
    let BoundedBytes {
        bytes,
        truncated: transfer_truncated,
        malformed: malformed_transfer,
    } = bounded_transfer(raw, encoding, max_bytes);
    let (decoded, malformed_charset) = if transfer_truncated {
        (
            Cow::Owned(charset.decode_prefix(bytes.as_ref()).into_owned()),
            false,
        )
    } else {
        charset.decode_cow_checked(bytes)
    };
    let end = decoded
        .get(..max_bytes)
        .map_or(decoded.len(), str::len)
        .min(decoded.len());
    let truncated = transfer_truncated || end < decoded.len();
    let text = super::truncate(decoded, end);
    BoundedText {
        text,
        truncated,
        malformed_transfer,
        malformed_charset,
    }
}
