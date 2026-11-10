//! Tests of the `mod ui` block of `examples/news.rs`, against a response shaped like the real one.

mod support;

use host_tests::gfx::{Clock, Color6, EpdBuffer, new_frame};
use host_tests::news::*;
use support::count;

const HEADLINES: &[u8] = include_bytes!("../fixtures/news_headlines.json");

#[test]
fn takes_the_first_articles_that_have_a_description() {
    let items = parse(HEADLINES).unwrap();
    assert_eq!(items.len(), MAX_ITEMS);
    assert_eq!(items[0].title, "City council approves new bike lanes");
    // The two articles without a usable description are skipped, not counted.
    assert!(items.iter().all(|i| !i.description.is_empty()));
    assert!(items.iter().all(|i| !i.title.contains("without")));
}

#[test]
fn folds_titles_and_descriptions_to_ascii() {
    for item in parse(HEADLINES).unwrap() {
        assert!(item.title.is_ascii(), "{}", item.title);
        assert!(item.description.is_ascii(), "{}", item.description);
    }
}

#[test]
fn no_articles_or_bad_json_is_none() {
    assert!(parse(b"").is_none());
    assert!(parse(br#"{"status":"ok"}"#).is_none());
    assert!(parse(br#"{"articles":[]}"#).is_none());
    assert!(parse(br#"{"articles":[{"title":"t","description":""}]}"#).is_none());
    assert!(parse(br#"{"articles":[{"title":"t","description":null}]}"#).is_none());
}

#[test]
fn the_request_url_carries_the_key_and_the_country() {
    let url = url("KEY123");
    assert!(url.starts_with("https://newsapi.org/v2/top-headlines?"));
    assert!(url.contains("country=us"));
    assert!(url.ends_with("apiKey=KEY123"));
}

#[test]
fn the_date_and_update_lines_match_the_sketch() {
    let now = Clock {
        year: 2026,
        month: 11,
        day: 9,
        hour: 7,
        minute: 5,
    };
    assert_eq!(date_line(Some(now)), "Date: 09.11.2026");
    assert_eq!(update_line(Some(now)), "Last update: 07:05");
    assert_eq!(date_line(None), "Date: unknown");
    assert_eq!(update_line(None), "Last update: unknown");
}

#[test]
fn the_screen_has_a_red_rule_blue_headlines_and_black_text() {
    let items = parse(HEADLINES).unwrap();
    let mut frame = new_frame();
    draw_news(
        &mut EpdBuffer::new(&mut frame),
        &items,
        "Date: 09.11.2026",
        "Last update: 07:05",
    );
    // The rule alone is 740 x 3 pixels.
    assert!(count(&frame, Color6::Red) >= 740 * 3);
    assert!(count(&frame, Color6::Blue) > 500, "the headlines");
    assert!(
        count(&frame, Color6::Black) > 2000,
        "the text and the rules"
    );
}

#[test]
fn a_single_article_draws_without_separators() {
    let items = parse(HEADLINES).unwrap();
    let mut one = new_frame();
    draw_news(&mut EpdBuffer::new(&mut one), &items[..1], "d", "u");
    let mut three = new_frame();
    draw_news(&mut EpdBuffer::new(&mut three), &items, "d", "u");
    assert!(count(&one, Color6::Black) < count(&three, Color6::Black));
}
