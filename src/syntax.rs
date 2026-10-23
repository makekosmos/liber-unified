//! Tree-sitter display spans, prepared on the PDF worker and cached on disk.
//! Parsing and query compilation never run in the reader's render loop.
use std::{cell::RefCell, sync::OnceLock};
use serde::{Deserialize, Serialize};
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Token { Comment, String, Number, Keyword, Type, Function, Operator }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span { pub start: usize, pub end: usize, pub token: Token }

const NAMES: [&str; 15] = ["comment", "string", "number", "keyword", "type",
    "constructor", "function", "operator", "constant", "constant.builtin",
    "variable.builtin", "string.special", "function.method", "function.builtin", "property"];
const TOKENS: [Option<Token>; 15] = [Some(Token::Comment), Some(Token::String),
    Some(Token::Number), Some(Token::Keyword), Some(Token::Type), Some(Token::Type),
    Some(Token::Function), Some(Token::Operator), Some(Token::Number), Some(Token::Keyword),
    Some(Token::Keyword), Some(Token::String), Some(Token::Function), Some(Token::Function), None];

fn configuration() -> &'static HighlightConfiguration {
    static CONFIG: OnceLock<HighlightConfiguration> = OnceLock::new();
    CONFIG.get_or_init(|| {
        let mut config = HighlightConfiguration::new(tree_sitter_javascript::LANGUAGE.into(),
            "javascript", tree_sitter_javascript::HIGHLIGHT_QUERY, "",
            tree_sitter_javascript::LOCALS_QUERY).expect("bundled JavaScript highlight query");
        config.configure(&NAMES);
        config
    })
}

pub fn lex(text: &str) -> Vec<Span> {
    thread_local! { static ENGINE: RefCell<Highlighter> = RefCell::new(Highlighter::new()); }
    ENGINE.with(|engine| {
        let mut engine = engine.borrow_mut();
        let Ok(events) = engine.highlight(configuration(), text.as_bytes(), None, |_| None) else {
            return Vec::new();
        };
        let mut stack = Vec::new();
        let mut spans: Vec<Span> = Vec::new();
        for event in events {
            match event {
                Ok(HighlightEvent::HighlightStart(highlight)) => stack.push(highlight.0),
                Ok(HighlightEvent::HighlightEnd) => { stack.pop(); }
                Ok(HighlightEvent::Source { start, end }) => {
                    let token = stack.iter().rev().find_map(|i| TOKENS.get(*i).copied().flatten());
                    if let Some(token) = token.filter(|_| start < end) {
                        if let Some(last) = spans.last_mut().filter(|s| s.end == start && s.token == token) {
                            last.end = end;
                        } else { spans.push(Span { start, end, token }); }
                    }
                }
                Err(_) => break, // Broken OCR still renders as plain text.
            }
        }
        spans
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn javascript_unicode_and_comments_keep_exact_source_offsets() {
        let text = "const ответ = fn(0xff, 3.2e-4); // число 3\nreturn 'Юникод';";
        let spans = lex(text);
        for span in &spans { assert!(text.get(span.start..span.end).is_some()); }
        assert!(spans.windows(2).all(|p| p[0].end <= p[1].start));
        for (expected, token) in [("const", Token::Keyword), ("0xff", Token::Number),
            ("3.2e-4", Token::Number), ("// число 3", Token::Comment), ("'Юникод'", Token::String)] {
            assert!(spans.iter().any(|s| s.token == token && &text[s.start..s.end] == expected), "{expected}: {spans:?}");
        }
    }

    #[test]
    fn numeric_examples_and_incomplete_code_are_safe() {
        let text = "0\n3\n10000000\n'незакрытая строка";
        let spans = lex(text);
        assert_eq!(spans.iter().filter(|s| s.token == Token::Number).count(), 3);
        assert!(spans.iter().all(|s| text.get(s.start..s.end).is_some()));
        let _ = lex("/* unfinished");
    }
}
