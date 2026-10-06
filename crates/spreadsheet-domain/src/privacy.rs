//! Bounded, opt-in recognition. Matches are heuristics, never proof of identity.
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum SensitiveKind {
    Phone,
    Passport,
    Snils,
    Inn,
    Email,
    Card,
}
impl SensitiveKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Phone => "phone",
            Self::Passport => "passport",
            Self::Snils => "snils",
            Self::Inn => "inn",
            Self::Email => "email",
            Self::Card => "card",
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SensitiveMatch {
    pub kind: SensitiveKind,
    pub start: usize,
    pub end: usize,
}
#[derive(Debug, Error)]
#[error("privacy analysis budget exceeded")]
pub struct PrivacyLimit;

fn nontrivial(digits: &[u8]) -> bool {
    digits
        .first()
        .is_some_and(|first| digits.iter().any(|d| d != first))
}
fn check(digits: &[u8], weights: &[u32]) -> u8 {
    (digits
        .iter()
        .zip(weights)
        .map(|(d, w)| u32::from(*d) * w)
        .sum::<u32>()
        % 11
        % 10) as u8
}
fn inn(d: &[u8]) -> bool {
    nontrivial(d)
        && match d.len() {
            10 => check(d, &[2, 4, 10, 3, 5, 9, 4, 6, 8]) == d[9],
            12 => {
                check(d, &[7, 2, 4, 10, 3, 5, 9, 4, 6, 8]) == d[10]
                    && check(d, &[3, 7, 2, 4, 10, 3, 5, 9, 4, 6, 8]) == d[11]
            }
            _ => false,
        }
}
fn snils(d: &[u8]) -> bool {
    d.len() == 11
        && nontrivial(d)
        && (d
            .iter()
            .take(9)
            .enumerate()
            .map(|(i, d)| u32::from(*d) * (9 - i) as u32)
            .sum::<u32>()
            % 101
            % 100)
            == u32::from(d[9]) * 10 + u32::from(d[10])
}
fn card(d: &[u8]) -> bool {
    (13..=19).contains(&d.len())
        && nontrivial(d)
        && (2..=6).contains(&d[0])
        && d.iter()
            .rev()
            .enumerate()
            .map(|(i, d)| {
                let value = u32::from(*d) * if i % 2 == 0 { 1 } else { 2 };
                if value > 9 { value - 9 } else { value }
            })
            .sum::<u32>()
            % 10
            == 0
}
fn separator(c: char) -> bool {
    matches!(c, ' ' | '\u{a0}' | '-' | '(' | ')' | '.')
}
fn email_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '.' | '_' | '-' | '+' | '%')
}

pub fn recognize(text: &str, kinds: &[SensitiveKind]) -> Result<Vec<SensitiveMatch>, PrivacyLimit> {
    recognize_with_passport_context(text, kinds, false)
}

pub fn passport_context(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("паспорт") || (lower.contains("серия") && lower.contains("номер"))
}

/// Context is supplied only from the same sheet's row/column labels.
pub fn recognize_with_passport_context(
    text: &str,
    kinds: &[SensitiveKind],
    axis_passport_context: bool,
) -> Result<Vec<SensitiveMatch>, PrivacyLimit> {
    if text.len() > 64 * 1024 {
        return Err(PrivacyLimit);
    }
    let chars: Vec<_> = text.char_indices().collect();
    let lower = text.to_lowercase();
    let passport_context = axis_passport_context || passport_context(text);
    let phone_context = lower.contains("тел") || lower.contains("phone");
    let mut matches = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let (start, c) = chars[i];
        if c.is_ascii_digit()
            || (c == '+' && chars.get(i + 1).is_some_and(|(_, c)| c.is_ascii_digit()))
        {
            let previous_is_word = i > 0 && chars[i - 1].1.is_alphanumeric();
            let mut j = i + usize::from(c == '+');
            let mut d = Vec::new();
            let mut end = start;
            while let Some((at, c)) = chars.get(j) {
                if c.is_ascii_digit() {
                    d.push(*c as u8 - b'0');
                    end = at + c.len_utf8();
                } else if !separator(*c) {
                    break;
                }
                j += 1;
            }
            let tail_word = chars
                .get(j)
                .is_some_and(|(at, c)| *at == end && c.is_alphanumeric());
            if !previous_is_word && !tail_word && d.len() <= 19 {
                for kind in kinds {
                    let found = match kind {
                        SensitiveKind::Inn => inn(&d),
                        SensitiveKind::Snils => snils(&d),
                        SensitiveKind::Card => card(&d),
                        SensitiveKind::Phone => {
                            (d.len() == 11
                                && matches!(d[0], 7 | 8)
                                && matches!(d[1], 3 | 4 | 8 | 9))
                                || (d.len() == 10 && matches!(d[0], 3 | 4 | 8 | 9) && phone_context)
                        }
                        SensitiveKind::Passport => {
                            d.len() == 10 && nontrivial(&d) && passport_context
                        }
                        SensitiveKind::Email => false,
                    };
                    if found {
                        matches.push(SensitiveMatch {
                            kind: *kind,
                            start,
                            end,
                        });
                    }
                }
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    if kinds.contains(&SensitiveKind::Email) {
        for (i, (_, c)) in chars.iter().enumerate() {
            if *c != '@' {
                continue;
            }
            let mut left = i;
            while left > 0 && email_char(chars[left - 1].1) {
                left -= 1;
            }
            let mut right = i + 1;
            while right < chars.len()
                && (chars[right].1.is_alphanumeric() || matches!(chars[right].1, '.' | '-'))
            {
                right += 1;
            }
            while right > i + 1 && chars[right - 1].1 == '.' {
                right -= 1;
            }
            let domain: String = chars[i + 1..right].iter().map(|(_, c)| *c).collect();
            if left < i
                && domain.rsplit_once('.').is_some_and(|(host, tld)| {
                    !host.is_empty() && tld.len() >= 2 && tld.chars().all(char::is_alphabetic)
                })
            {
                matches.push(SensitiveMatch {
                    kind: SensitiveKind::Email,
                    start: chars[left].0,
                    end: chars.get(right).map_or(text.len(), |(at, _)| *at),
                });
            }
        }
    }
    matches.sort_by_key(|m| (m.start, m.end, m.kind));
    matches.dedup();
    if matches.len() > 1000 {
        return Err(PrivacyLimit);
    }
    Ok(matches)
}
pub fn mask(text: &str, matches: &[SensitiveMatch]) -> String {
    let mut spans: Vec<_> = matches
        .iter()
        .filter(|m| {
            m.end <= text.len()
                && m.start < m.end
                && text.is_char_boundary(m.start)
                && text.is_char_boundary(m.end)
        })
        .map(|m| (m.start, m.end))
        .collect();
    spans.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in spans {
        if let Some(last) = merged.last_mut()
            && start <= last.1
        {
            last.1 = last.1.max(end);
        } else {
            merged.push((start, end));
        }
    }
    let mut out = String::new();
    let mut position = 0;
    for (start, end) in merged {
        let item = SensitiveMatch {
            kind: SensitiveKind::Email,
            start,
            end,
        };
        if item.start < position
            || item.end > text.len()
            || !text.is_char_boundary(item.start)
            || !text.is_char_boundary(item.end)
        {
            continue;
        }
        out.push_str(&text[position..item.start]);
        out.push_str("[СКРЫТО]");
        position = item.end;
    }
    out.push_str(&text[position..]);
    out
}
pub fn fingerprint(scope: &[u8], text: &str, kind: SensitiveKind) -> String {
    let mut hash = Sha256::new();
    hash.update(b"PXEL privacy exception v1\0");
    hash.update(scope);
    hash.update([0]);
    hash.update(kind.name());
    hash.update([0]);
    hash.update(text);
    hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn passport_axis_context_preserves_spans_and_requires_format() -> Result<(), PrivacyLimit> {
        for text in ["45 01 123456", "4501123456"] {
            assert!(recognize(text, &[SensitiveKind::Passport])?.is_empty());
            let found = recognize_with_passport_context(text, &[SensitiveKind::Passport], true)?;
            assert_eq!(found.len(), 1);
            assert_eq!(&text[found[0].start..found[0].end], text);
            assert_eq!(mask(text, &found), "[СКРЫТО]");
        }
        for text in ["450112345", "45011234567", "0000000000", "item4501123456"] {
            assert!(
                recognize_with_passport_context(text, &[SensitiveKind::Passport], true)?.is_empty()
            );
        }
        assert!(passport_context("ПАСПОРТ"));
        assert!(passport_context("Серия и номер"));
        assert!(!passport_context("Номер заказа"));
        Ok(())
    }
    #[test]
    fn recognizes_each_type_and_rejects_invalid_checksums() -> Result<(), PrivacyLimit> {
        let cases = [
            (SensitiveKind::Phone, "+7 (999) 123-45-67"),
            (SensitiveKind::Passport, "Паспорт: 45 01 123456"),
            (SensitiveKind::Snils, "112-233-445 95"),
            (SensitiveKind::Inn, "7707083893"),
            (SensitiveKind::Inn, "500100732259"),
            (SensitiveKind::Email, "тест+отчёт@example.com"),
            (SensitiveKind::Card, "4242 4242 4242 4242"),
        ];
        for (kind, text) in cases {
            assert_eq!(recognize(text, &[kind])?.len(), 1, "{kind:?}");
            assert!(!mask(text, &recognize(text, &[kind])?).contains(text));
        }
        for (kind, text) in [
            (SensitiveKind::Snils, "112-233-445 94"),
            (SensitiveKind::Inn, "7707083894"),
            (SensitiveKind::Inn, "0000000000"),
            (SensitiveKind::Card, "4242424242424243"),
            (SensitiveKind::Passport, "45 01 123456"),
            (SensitiveKind::Phone, "+7 701 123 45 67"),
            (SensitiveKind::Email, "a@localhost"),
        ] {
            assert!(recognize(text, &[kind])?.is_empty(), "{kind:?}");
        }
        assert!(recognize("test@example.com", &[])?.is_empty());
        Ok(())
    }
    #[test]
    fn unicode_masks_and_exceptions_follow_exact_content() -> Result<(), PrivacyLimit> {
        let text = "📩 Для связи: test@example.com, копия other@example.org.";
        assert_eq!(
            mask(text, &recognize(text, &[SensitiveKind::Email])?),
            "📩 Для связи: [СКРЫТО], копия [СКРЫТО]."
        );
        assert_ne!(
            fingerprint(b"cell", "test@example.com", SensitiveKind::Email),
            fingerprint(b"cell", "other@example.com", SensitiveKind::Email)
        );
        assert!(recognize(&"x".repeat(65537), &[]).is_err());
        let overlapping = "4242424242424242@example.com";
        assert_eq!(
            mask(
                overlapping,
                &recognize(overlapping, &[SensitiveKind::Card, SensitiveKind::Email])?
            ),
            "[СКРЫТО]"
        );
        Ok(())
    }
}
