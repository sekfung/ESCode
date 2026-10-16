use super::super::service_support::{activated_block, mentioned_names};
use super::*;

#[test]
fn mentions_follow_the_ts_regex() {
    let names = |prompt: &str| mentioned_names(prompt).into_iter().collect::<Vec<_>>();
    assert_eq!(names("use $foo and $bar-baz."), vec!["bar-baz", "foo"]);
    assert_eq!(
        names("$a--b $-x $9 $UP $x_y $ok-"),
        vec!["9", "a", "ok", "x"]
    );
    assert!(names("no skills $ here").is_empty());
}

#[test]
fn prompt_block_escapes_attributes() {
    let skill = json!({"name": "a\"b", "path": "/p/<x>&", "body": "BODY"});
    assert_eq!(
        activated_block(&[&skill]),
        "<available_skills>\n<activated_skill name=\"a&quot;b\" path=\"/p/&lt;x&gt;&amp;\">\nBODY\n</activated_skill>\n</available_skills>"
    );
}
