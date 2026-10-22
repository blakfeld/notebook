//! The tag cases shared with the editor's highlighter tests in `web/`.

use notebook_core::tags::parse;
use serde::Deserialize;

#[derive(Deserialize)]
struct Case {
    body: String,
    /// The source text of each occurrence, in order.
    tags: Vec<String>,
}

#[test]
fn parser_matches_the_shared_fixture() {
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("fixtures/tag_cases.json")).expect("valid fixture");
    assert!(!cases.is_empty());
    for case in cases {
        let found: Vec<&str> = parse(&case.body)
            .into_iter()
            .map(|o| &case.body[o.range])
            .collect();
        assert_eq!(found, case.tags, "{:?}", case.body);
    }
}
