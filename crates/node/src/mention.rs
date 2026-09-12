//! Explicit scoping mentions typed in the chat composer.
//!
//! When the user already knows where a request should go, there is nothing for
//! the planner to guess. A mention narrows the catalogue *before* retrieval and
//! compile, which is the cheapest precision available.
//!
//! The shape is `@type:value`, where `/` descends a hierarchy:
//!
//! ```text
//! @file:contrats/2026/bail.pdf   a blob in the local store
//! @peer:alice                    an instance (local petname or n3: id)
//! @peer:alice/summarize          one capability on one instance
//! @lobe:medical                  a lobe
//! @lobe:medical/summarize        that capability, anywhere in the lobe
//! ```
//!
//! Three rules make this safe to parse out of prose:
//!
//! - the `@` must open a token (start of input or preceded by whitespace), so
//!   `bob@example.com` is never a mention;
//! - the type prefix is mandatory, so a bare `@alice` stays literal text — an
//!   alias carries no cryptographic weight and must never be resolved by
//!   guesswork;
//! - the mention stays in the text handed to the planner. It carries meaning
//!   ("… la météo à Lomé"); only the *scope* is decided ahead of time.

use std::fmt;

/// What a mention points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MentionKind {
    /// A blob in the local store.
    File,
    /// A peer instance.
    Peer,
    /// A lobe.
    Lobe,
}

impl MentionKind {
    fn from_prefix(s: &str) -> Option<Self> {
        match s {
            "file" => Some(Self::File),
            "peer" => Some(Self::Peer),
            "lobe" => Some(Self::Lobe),
            _ => None,
        }
    }

    /// The wire spelling used in the composer.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Peer => "peer",
            Self::Lobe => "lobe",
        }
    }
}

impl fmt::Display for MentionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One `@type:value[/tail]` occurrence found in a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mention {
    /// Which namespace the mention addresses.
    pub kind: MentionKind,
    /// The entity: a store path, a peer petname or `n3:` id, a lobe id.
    pub entity: String,
    /// Capability name after the first `/`, when present. Files never have
    /// one — a `/` in a file mention is part of its path.
    pub capability: Option<String>,
    /// Byte range of the whole mention in the source text.
    pub span: (usize, usize),
}

impl Mention {
    /// Re-render the mention exactly as it should appear in the composer.
    #[must_use]
    pub fn to_token(&self) -> String {
        match &self.capability {
            Some(cap) => format!("@{}:{}/{}", self.kind, self.entity, cap),
            None => format!("@{}:{}", self.kind, self.entity),
        }
    }
}

/// Trailing characters that belong to the sentence, not to the mention.
const TRAILING_PUNCTUATION: &[char] = &['.', ',', ';', ':', '!', '?', ')', ']', '}', '"', '\''];

/// Extract every mention from `text`, in order of appearance.
///
/// Anything that does not match the strict shape is left alone: this function
/// never guesses, so an unresolved `@foo` simply stays part of the sentence.
#[must_use]
pub fn parse_mentions(text: &str) -> Vec<Mention> {
    let mut out = Vec::new();
    let mut idx = 0;

    while let Some(rel) = text[idx..].find('@') {
        let at = idx + rel;
        idx = at + 1;

        // The `@` must open a token, otherwise `bob@example.com` matches.
        if at > 0 {
            let prev = text[..at].chars().next_back().unwrap_or(' ');
            if !prev.is_whitespace() {
                continue;
            }
        }

        // Token runs to the next whitespace.
        let rest = &text[at + 1..];
        let end_rel = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let raw = &rest[..end_rel];
        let raw = raw.trim_end_matches(TRAILING_PUNCTUATION);
        if raw.is_empty() {
            continue;
        }

        let Some((prefix, value)) = raw.split_once(':') else {
            continue;
        };
        let Some(kind) = MentionKind::from_prefix(prefix) else {
            continue;
        };
        if value.is_empty() {
            continue;
        }

        // `/` splits entity from capability for peers and lobes. For files it
        // is part of the path, and for a canonical `@file:sha256:…` there is
        // no tail at all.
        let (entity, capability) = match kind {
            MentionKind::File => (value.to_string(), None),
            MentionKind::Peer | MentionKind::Lobe => match value.split_once('/') {
                Some((e, cap)) if !e.is_empty() && !cap.is_empty() => {
                    (e.to_string(), Some(cap.to_string()))
                }
                // A trailing or leading slash is a typo, not a capability.
                Some((e, _)) if !e.is_empty() => (e.to_string(), None),
                Some(_) => continue,
                None => (value.to_string(), None),
            },
        };

        let span_end = at + 1 + raw.len();
        out.push(Mention {
            kind,
            entity,
            capability,
            span: (at, span_end),
        });
        idx = span_end;
    }

    out
}

/// The catalogue narrowing a message asks for.
///
/// Empty means "no explicit scope" — the planner sees the whole catalogue, as
/// before. Several mentions of the same kind union rather than intersect: two
/// peers means both, which is how a comparison request reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MentionScope {
    /// Peer petnames or `n3:` ids the message restricts to.
    pub peers: Vec<String>,
    /// Lobe ids the message restricts to.
    pub lobes: Vec<String>,
    /// Capability names named explicitly (`@peer:alice/summarize`).
    pub capabilities: Vec<String>,
    /// Store paths mentioned as data.
    pub files: Vec<String>,
}

impl MentionScope {
    /// True when nothing was scoped and the catalogue must be left alone.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.peers.is_empty() && self.lobes.is_empty() && self.capabilities.is_empty()
    }

    /// Collect the scope expressed by a message.
    #[must_use]
    pub fn from_text(text: &str) -> Self {
        let mut scope = Self::default();
        for m in parse_mentions(text) {
            match m.kind {
                MentionKind::File => push_unique(&mut scope.files, m.entity),
                MentionKind::Peer => push_unique(&mut scope.peers, m.entity),
                MentionKind::Lobe => push_unique(&mut scope.lobes, m.entity),
            }
            if let Some(cap) = m.capability {
                push_unique(&mut scope.capabilities, cap);
            }
        }
        scope
    }
}

fn push_unique(v: &mut Vec<String>, item: String) {
    if !v.contains(&item) {
        v.push(item);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(MentionKind, String, Option<String>)> {
        parse_mentions(text)
            .into_iter()
            .map(|m| (m.kind, m.entity, m.capability))
            .collect()
    }

    #[test]
    fn parses_each_kind() {
        assert_eq!(
            kinds("@peer:alice @lobe:medical @file:rapport.pdf"),
            vec![
                (MentionKind::Peer, "alice".into(), None),
                (MentionKind::Lobe, "medical".into(), None),
                (MentionKind::File, "rapport.pdf".into(), None),
            ]
        );
    }

    #[test]
    fn splits_capability_for_peer_and_lobe() {
        assert_eq!(
            kinds("@peer:alice/summarize"),
            vec![(MentionKind::Peer, "alice".into(), Some("summarize".into()))]
        );
        assert_eq!(
            kinds("@lobe:medical/summarize"),
            vec![(
                MentionKind::Lobe,
                "medical".into(),
                Some("summarize".into())
            )]
        );
    }

    #[test]
    fn slash_in_a_file_mention_is_part_of_the_path() {
        assert_eq!(
            kinds("@file:contrats/2026/bail.pdf"),
            vec![(MentionKind::File, "contrats/2026/bail.pdf".into(), None)]
        );
    }

    #[test]
    fn canonical_forms_survive_their_inner_colon() {
        assert_eq!(
            kinds("@file:sha256:abc123"),
            vec![(MentionKind::File, "sha256:abc123".into(), None)]
        );
        assert_eq!(
            kinds("@peer:n3:65s25vdkawysmx4cca3nujvwqahx3z7s"),
            vec![(
                MentionKind::Peer,
                "n3:65s25vdkawysmx4cca3nujvwqahx3z7s".into(),
                None
            )]
        );
    }

    #[test]
    fn email_addresses_are_not_mentions() {
        assert!(parse_mentions("envoie à bob@example.com stp").is_empty());
        assert!(parse_mentions("bob@peer:alice").is_empty());
    }

    #[test]
    fn bare_mentions_stay_literal() {
        assert!(parse_mentions("salut @alice").is_empty());
        assert!(parse_mentions("@unknown:thing").is_empty());
        assert!(parse_mentions("@peer:").is_empty());
        assert!(parse_mentions("@ peer:alice").is_empty());
    }

    #[test]
    fn trailing_punctuation_is_not_part_of_the_mention() {
        assert_eq!(
            kinds("demande à @peer:alice, puis @lobe:medical."),
            vec![
                (MentionKind::Peer, "alice".into(), None),
                (MentionKind::Lobe, "medical".into(), None),
            ]
        );
    }

    #[test]
    fn mention_keeps_its_span_and_round_trips() {
        let text = "résume @file:rapport.pdf vite";
        let m = &parse_mentions(text)[0];
        assert_eq!(&text[m.span.0..m.span.1], "@file:rapport.pdf");
        assert_eq!(m.to_token(), "@file:rapport.pdf");
    }

    #[test]
    fn scope_unions_and_dedupes() {
        let s = MentionScope::from_text(
            "@peer:alice @peer:bob @peer:alice compare, @lobe:medical/summarize",
        );
        assert_eq!(s.peers, vec!["alice", "bob"]);
        assert_eq!(s.lobes, vec!["medical"]);
        assert_eq!(s.capabilities, vec!["summarize"]);
        assert!(!s.is_empty());
    }

    #[test]
    fn files_alone_do_not_scope_the_catalogue() {
        let s = MentionScope::from_text("résume @file:a.pdf et @file:b.pdf");
        assert_eq!(s.files, vec!["a.pdf", "b.pdf"]);
        assert!(s.is_empty(), "a file is data, not a tool scope");
    }
}
