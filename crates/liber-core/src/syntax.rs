//! JavaScript syntax spans for reader code blocks.
use std::{cell::RefCell, sync::OnceLock};

use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

use crate::{CodeSpan, CodeToken};

const NAMES: [&str; 15] = [
    "comment",
    "string",
    "number",
    "keyword",
    "type",
    "constructor",
    "function",
    "operator",
    "constant",
    "constant.builtin",
    "variable.builtin",
    "string.special",
    "function.method",
    "function.builtin",
    "property",
];
const TOKENS: [Option<CodeToken>; 15] = [
    Some(CodeToken::Comment),
    Some(CodeToken::String),
    Some(CodeToken::Number),
    Some(CodeToken::Keyword),
    Some(CodeToken::Type),
    Some(CodeToken::Type),
    Some(CodeToken::Function),
    Some(CodeToken::Operator),
    Some(CodeToken::Number),
    Some(CodeToken::Keyword),
    Some(CodeToken::Keyword),
    Some(CodeToken::String),
    Some(CodeToken::Function),
    Some(CodeToken::Function),
    None,
];

fn configuration() -> &'static HighlightConfiguration {
    static CONFIG: OnceLock<HighlightConfiguration> = OnceLock::new();
    CONFIG.get_or_init(|| {
        let mut config = HighlightConfiguration::new(
            tree_sitter_javascript::LANGUAGE.into(),
            "javascript",
            tree_sitter_javascript::HIGHLIGHT_QUERY,
            "",
            tree_sitter_javascript::LOCALS_QUERY,
        )
        .expect("bundled JavaScript highlight query");
        config.configure(&NAMES);
        config
    })
}

pub(crate) fn lex(text: &str) -> Vec<CodeSpan> {
    thread_local! {
        static ENGINE: RefCell<Highlighter> = RefCell::new(Highlighter::new());
    }
    ENGINE.with(|engine| {
        let mut engine = engine.borrow_mut();
        let Ok(events) = engine.highlight(configuration(), text.as_bytes(), None, |_| None) else {
            return Vec::new();
        };
        let mut stack = Vec::new();
        let mut spans: Vec<CodeSpan> = Vec::new();
        for event in events {
            match event {
                Ok(HighlightEvent::HighlightStart(highlight)) => stack.push(highlight.0),
                Ok(HighlightEvent::HighlightEnd) => {
                    stack.pop();
                }
                Ok(HighlightEvent::Source { start, end }) => {
                    let token = stack
                        .iter()
                        .rev()
                        .find_map(|index| TOKENS.get(*index).copied().flatten());
                    if let Some(token) = token.filter(|_| start < end) {
                        if let Some(last) = spans
                            .last_mut()
                            .filter(|span| span.end == start && span.token == token)
                        {
                            last.end = end;
                        } else {
                            spans.push(CodeSpan { start, end, token });
                        }
                    }
                }
                Err(_) => break,
            }
        }
        spans
    })
}
