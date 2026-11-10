//! Tests of the `mod ui` block of `examples/quotables.rs`, against a response shaped like the real one.

mod support;

use host_tests::gfx::{Color6, EpdBuffer, new_frame};
use host_tests::quotables::*;
use support::count;

const RANDOM: &[u8] = include_bytes!("../fixtures/quotable_random.json");

fn body(content: &str, author: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "quote": { "content": content, "author": { "name": author } }
    }))
    .unwrap()
}

#[test]
fn parses_the_quote_and_the_author() {
    let quote = parse(RANDOM).unwrap();
    assert_eq!(quote.text, "The only thing we have to fear is fear itself.");
    assert_eq!(quote.author, "Franklin D. Roosevelt");
}

#[test]
fn folds_typography_to_ascii() {
    let quote = parse(&body(
        "\u{201C}Be\u{201D} \u{2014} now\u{2026}",
        "Jos\u{e9}",
    ))
    .unwrap();
    assert_eq!(quote.text, "\"Be\" - now...");
    assert_eq!(quote.author, "Jos");
}

#[test]
fn a_quote_of_exactly_the_limit_is_kept_and_one_byte_more_is_not() {
    assert!(parse(&body(&"a".repeat(MAX_QUOTE_LEN), "A")).is_some());
    assert!(parse(&body(&"a".repeat(MAX_QUOTE_LEN + 1), "A")).is_none());
}

#[test]
fn bad_or_incomplete_json_is_none() {
    assert!(parse(b"").is_none());
    assert!(parse(b"{").is_none());
    assert!(parse(br#"{"quote":{"content":"x"}}"#).is_none());
    assert!(parse(br#"{"quote":{"author":{"name":"x"}}}"#).is_none());
    assert!(parse(br#"{"quote":{"content":null,"author":{"name":"x"}}}"#).is_none());
}

#[test]
fn the_screen_has_a_red_mark_black_text_and_a_blue_author() {
    let quote = parse(RANDOM).unwrap();
    let mut frame = new_frame();
    draw_quote(&mut EpdBuffer::new(&mut frame), &quote);
    assert!(count(&frame, Color6::Red) > 20, "the opening mark");
    assert!(count(&frame, Color6::Black) > 500, "the quote");
    assert!(count(&frame, Color6::Blue) > 100, "the author");
    assert_eq!(count(&frame, Color6::Green), 0);
}

#[test]
fn the_longest_quote_still_draws() {
    let quote = parse(&body(
        &"wide words ".repeat(11).trim().to_owned(),
        "A. Author",
    ))
    .unwrap();
    let mut frame = new_frame();
    draw_quote(&mut EpdBuffer::new(&mut frame), &quote);
    assert!(count(&frame, Color6::Black) > 500);
}
