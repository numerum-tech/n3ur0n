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
//! @peer:alice                    an instance (n3: id; `self` = this one)
//! @peer:alice/summarize          one capability on one instance
//! @lobe:medical                  a lobe
//! @lobe:medical/summarize        that capability, anywhere in the lobe
//! @cap:summarize                 a capability by name, wherever it lives
//! @file:"Cahier des Charges.pdf" a value containing spaces
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
//!
//! A mention states an intention; it does not assert that the thing exists. The
//! user names what they want, and it is the system's job to say whether it has
//! it. So a mention that does not resolve must neither narrow the catalogue to
//! nothing nor be dropped in silence — both decide on the user's behalf without
//! telling them. Resolution therefore reports what it could not find, and the
//! reply says so. See `plan_exec::resolve_scope`.

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
    /// A capability by name, on no particular peer.
    Cap,
}

impl MentionKind {
    fn from_prefix(s: &str) -> Option<Self> {
        match s {
            "file" => Some(Self::File),
            "peer" => Some(Self::Peer),
            "lobe" => Some(Self::Lobe),
            "cap" => Some(Self::Cap),
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
            Self::Cap => "cap",
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
        let value = match &self.capability {
            Some(cap) => format!("{}/{}", self.entity, cap),
            None => self.entity.clone(),
        };
        format!("@{}:{}", self.kind, quote_if_needed(&value))
    }
}

/// Wrap a mention value in quotes when it could not be parsed back bare.
///
/// A value is quoted when it contains whitespace, or when it ends in sentence
/// punctuation that the parser would otherwise strip off.
#[must_use]
pub fn quote_if_needed(value: &str) -> String {
    let needs = value.chars().any(char::is_whitespace)
        || value.ends_with(TRAILING_PUNCTUATION)
        || value.is_empty();
    if needs {
        format!("\"{value}\"")
    } else {
        value.to_string()
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

        // The prefix runs to the `:` that types the mention.
        let rest = &text[at + 1..];
        let Some(colon) = rest.find(':') else {
            continue;
        };
        let Some(kind) = MentionKind::from_prefix(&rest[..colon]) else {
            continue;
        };
        let after_colon = &rest[colon + 1..];

        // A quoted value runs to its closing quote, which is how a file name
        // with spaces stays one token: `@file:"Cahier des Charges.pdf"`. The
        // picker writes the quotes; a user rarely types them. Unquoted values
        // still end at the first whitespace.
        let (value, consumed) = if let Some(body) = after_colon.strip_prefix('"') {
            match body.find('"') {
                // A name containing a `"` cannot be spelled this way; the
                // canonical `@file:sha256:…` form always can.
                Some(close) => (&body[..close], colon + 1 + close + 2),
                None => continue,
            }
        } else {
            let end = after_colon
                .find(char::is_whitespace)
                .unwrap_or(after_colon.len());
            let unquoted = after_colon[..end].trim_end_matches(TRAILING_PUNCTUATION);
            (unquoted, colon + 1 + unquoted.len())
        };
        if value.is_empty() {
            continue;
        }
        let raw_len = consumed;

        // `/` splits entity from capability for peers and lobes. For files it
        // is part of the path, and for a canonical `@file:sha256:…` there is
        // no tail at all.
        let (entity, capability) = match kind {
            // A file path and a capability name are both flat: a `/` inside
            // them is part of the value, not a descent.
            MentionKind::File | MentionKind::Cap => (value.to_string(), None),
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

        let span_end = at + 1 + raw_len;
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

/// The message with every mention removed.
///
/// Used to tell whether a message carries an actual request or is nothing but
/// scoping. `@peer:65s25vdkawys` on its own asks for nothing, and a composer
/// that lets the user send it means the planner must handle it — silently
/// inventing a request is the failure it produces otherwise.
#[must_use]
pub fn strip_mentions(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for m in parse_mentions(text) {
        out.push_str(&text[cursor..m.span.0]);
        cursor = m.span.1;
    }
    out.push_str(&text[cursor..]);
    out.trim().to_string()
}

/// True when a message is made only of mentions (and punctuation), so it
/// scopes without asking anything.
#[must_use]
pub fn is_scope_only(text: &str) -> bool {
    if parse_mentions(text).is_empty() {
        return false;
    }
    !strip_mentions(text).chars().any(|c| c.is_alphanumeric())
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
                MentionKind::Cap => push_unique(&mut scope.capabilities, m.entity),
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

    #[test]
    fn quoted_values_keep_their_spaces() {
        assert_eq!(
            kinds("résume @file:\"Cahier des Charges — GovActu.pdf\" stp"),
            vec![(
                MentionKind::File,
                "Cahier des Charges — GovActu.pdf".into(),
                None
            )]
        );
    }

    #[test]
    fn a_quoted_mention_spans_its_closing_quote() {
        let text = "voici @file:\"deux mots.pdf\" ok";
        let m = &parse_mentions(text)[0];
        assert_eq!(&text[m.span.0..m.span.1], "@file:\"deux mots.pdf\"");
        assert_eq!(m.to_token(), "@file:\"deux mots.pdf\"");
    }

    #[test]
    fn an_unterminated_quote_is_not_a_mention() {
        assert!(parse_mentions("@file:\"jamais fermé").is_empty());
    }

    #[test]
    fn quoting_only_kicks_in_when_needed() {
        assert_eq!(quote_if_needed("rapport.pdf"), "rapport.pdf");
        assert_eq!(quote_if_needed("deux mots.pdf"), "\"deux mots.pdf\"");
        // A name ending in punctuation would lose it to sentence trimming.
        assert_eq!(quote_if_needed("note."), "\"note.\"");
    }

    #[test]
    fn a_quoted_peer_mention_still_splits_its_capability() {
        assert_eq!(
            kinds("@peer:\"alice bis/summarize\""),
            vec![(
                MentionKind::Peer,
                "alice bis".into(),
                Some("summarize".into())
            )]
        );
    }

    #[test]
    fn strips_mentions_from_the_sentence() {
        assert_eq!(
            strip_mentions("résume @file:notes.md pour demain"),
            "résume  pour demain"
        );
        assert_eq!(strip_mentions("@peer:alice"), "");
    }

    #[test]
    fn recognises_a_message_that_only_scopes() {
        assert!(is_scope_only("@peer:vphg3ejn4x4s"));
        assert!(is_scope_only("  @lobe:medical @peer:alice  "));
        assert!(is_scope_only("@peer:alice ,"));
        assert!(!is_scope_only("@peer:alice quelle heure est-il ?"));
        // No mention at all is not "scope only" — it is an ordinary message.
        assert!(!is_scope_only("bonjour"));
        assert!(!is_scope_only(""));
    }

    #[test]
    fn a_capability_can_be_named_on_its_own() {
        assert_eq!(
            kinds("@cap:summarize ce texte"),
            vec![(MentionKind::Cap, "summarize".into(), None)]
        );
        let s = MentionScope::from_text("@cap:summarize");
        assert_eq!(s.capabilities, vec!["summarize"]);
        assert!(s.peers.is_empty() && s.lobes.is_empty());
        assert!(!s.is_empty());
    }

    #[test]
    fn a_capability_name_is_flat() {
        // No descent: a slash belongs to the name, which keeps the grammar
        // unambiguous when a name ever contains one.
        assert_eq!(
            kinds("@cap:docs/list"),
            vec![(MentionKind::Cap, "docs/list".into(), None)]
        );
    }
}
