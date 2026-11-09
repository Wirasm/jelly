//! Runs a jelly's verbs in an embedded QuickJS engine.
//!
//! A jelly's verbs are JavaScript source that declares `const verbs = { ... }`.
//! Each verb is `(state, args, { by, at_ms }) => void | newState`: it may mutate `state` in
//! place or return a replacement. Every call gets a fresh engine, so verbs share
//! nothing between calls except the state the daemon hands them.

use rquickjs::{Context, Runtime};
use serde_json::Value;
use std::time::{Duration, Instant};

/// Agent-written verbs can loop forever; the daemon must not hang with them.
const TIME_LIMIT: Duration = Duration::from_secs(1);
const MEMORY_LIMIT: usize = 64 << 20;

/// Loads the verbs and returns their names, or why they do not load.
pub fn check(src: &str) -> Result<Vec<String>, String> {
    let body = "return JSON.stringify({ok: Object.keys(__v).filter(k => typeof __v[k] === 'function')});";
    let names = eval(&script(src, body))?;
    Ok(serde_json::from_value(names).unwrap_or_default())
}

/// Runs one verb against `state` and returns the new state. The verb gets
/// `{ by, at_ms }` as a third argument so it can record who did what.
pub fn run(src: &str, verb: &str, state: &Value, args: &Value, by: &str, at_ms: u64) -> Result<Value, String> {
    let verb_lit = serde_json::to_string(verb).expect("a string serializes");
    let ctx = serde_json::json!({"by": by, "at_ms": at_ms});
    let body = format!(
        "var f = Object.prototype.hasOwnProperty.call(__v, {verb_lit}) ? __v[{verb_lit}] : undefined;
         if (typeof f !== 'function') return JSON.stringify({{error: 'no verb ' + {verb_lit} + '; this jelly has: ' + Object.keys(__v).join(', ')}});
         var state = {state}, args = {args}, ctx = {ctx};
         try {{ var r = f(state, args, ctx); return JSON.stringify({{ok: r === undefined ? state : r}}); }}
         catch (e) {{ return JSON.stringify({{error: String(e) + (e && e.stack ? '\\n' + e.stack : '')}}); }}",
    );
    eval(&script(src, &body))
}

/// Wraps the verbs source so `__v` holds its `verbs` object, then runs `body`.
fn script(src: &str, body: &str) -> String {
    format!(
        "(function() {{
           var __v;
           try {{ __v = (function() {{
{src}
;return typeof verbs === 'undefined' ? undefined : verbs; }})(); }}
           catch (e) {{ return JSON.stringify({{error: 'verbs did not load: ' + e}}); }}
           if (!__v || typeof __v !== 'object') return JSON.stringify({{error: 'verbs must declare `const verbs = {{ ... }}`'}});
           {body}
         }})()"
    )
}

fn eval(script: &str) -> Result<Value, String> {
    let rt = Runtime::new().map_err(|e| e.to_string())?;
    rt.set_memory_limit(MEMORY_LIMIT);
    let deadline = Instant::now() + TIME_LIMIT;
    rt.set_interrupt_handler(Some(Box::new(move || Instant::now() > deadline)));
    let ctx = Context::full(&rt).map_err(|e| e.to_string())?;
    let out: String = ctx.with(|ctx| match ctx.eval::<String, _>(script) {
        Ok(s) => Ok(s),
        Err(rquickjs::Error::Exception) => {
            // Errors inside verbs are caught in the script itself, so only a
            // syntax error in the verbs source or the time limit lands here.
            let caught = ctx.catch();
            let ex = caught.as_exception();
            let name: String = ex.and_then(|e| e.get("name").ok()).unwrap_or_else(|| "Error".into());
            let msg = ex.and_then(|e| e.message()).unwrap_or_else(|| "unknown error".into());
            Err(if Instant::now() > deadline {
                format!("verb ran longer than {}s and was stopped", TIME_LIMIT.as_secs())
            } else {
                format!("verbs did not load: {name}: {msg}")
            })
        }
        Err(e) => Err(e.to_string()),
    })?;
    let mut v: Value = serde_json::from_str(&out).map_err(|e| e.to_string())?;
    if let Some(err) = v.get("error").and_then(Value::as_str) {
        return Err(err.to_string());
    }
    Ok(v.get_mut("ok").map(Value::take).unwrap_or(Value::Null))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const VERBS: &str = "const verbs = {
        label(state, { id, value }) { state.labels[id] = value; },
        reset() { return { labels: {} }; },
        boom() { throw new Error('nope'); },
        spin() { for (;;) {} },
    };";

    #[test]
    fn mutating_and_returning_verbs_both_change_state() {
        let s = json!({"labels": {}});
        let s = run(VERBS, "label", &s, &json!({"id": "c1", "value": "fail"}), "agent", 0).unwrap();
        assert_eq!(s, json!({"labels": {"c1": "fail"}}));
        assert_eq!(run(VERBS, "reset", &s, &json!({}), "agent", 0).unwrap(), json!({"labels": {}}));
    }

    #[test]
    fn errors_name_the_problem() {
        let s = json!({});
        assert!(run(VERBS, "boom", &s, &json!({}), "agent", 0).unwrap_err().contains("nope"));
        assert!(run(VERBS, "missing", &s, &json!({}), "agent", 0).unwrap_err().contains("this jelly has: label"));
        assert!(run(VERBS, "toString", &s, &json!({}), "agent", 0).unwrap_err().contains("no verb"));
        assert!(check("const nope = 1;").unwrap_err().contains("const verbs"));
        let e = check("const verbs = {").unwrap_err(); assert!(e.contains("SyntaxError"), "{e}");
    }

    #[test]
    fn a_runaway_verb_is_stopped() {
        let err = run(VERBS, "spin", &json!({}), &json!({}), "agent", 0).unwrap_err();
        assert!(err.contains("stopped"), "{err}");
    }

    #[test]
    fn verbs_know_who_called() {
        let src = "const verbs = { sign(s, _a, { by, at_ms }) { s.log.push(by + '@' + at_ms); } };";
        let s = run(src, "sign", &json!({"log": []}), &json!({}), "person", 7).unwrap();
        assert_eq!(s, json!({"log": ["person@7"]}));
    }

    const DECISION: &str = include_str!("../assets/decision.verbs.js");

    #[test]
    fn decision_rounds_keep_history() {
        let src = DECISION;
        let spec = json!({"question": "Which?", "recommend": "b", "options": [{"id": "a", "title": "A"}, {"id": "b", "title": "B"}]});
        let s = run(src, "init", &json!({}), &spec, "agent", 1).unwrap();
        assert_eq!((s["status"].clone(), s["round"].clone()), (json!("open"), json!(1)));
        // One question needs no `q`.
        let s = run(src, "pick", &s, &json!({"option": "b", "note": "but smaller"}), "person", 2).unwrap();
        assert_eq!(s.pointer("/questions/0/answer/option"), Some(&json!("b")));
        assert_eq!(s["status"], "answered");
        assert!(run(src, "pick", &s, &json!({"option": "z"}), "person", 3).unwrap_err().contains("options are a, b"));
        let s = run(src, "revise", &s, &spec, "agent", 4).unwrap();
        assert_eq!((s["round"].clone(), s["status"].clone()), (json!(2), json!("open")));
        assert_eq!(s.pointer("/history/0/questions/0/note/text"), Some(&json!("but smaller")));
        let bad = json!({"question": "Which?", "recommend": "x", "options": [{"id": "a", "title": "A"}]});
        assert!(run(src, "init", &json!({}), &bad, "agent", 5).unwrap_err().contains("recommend"));
    }

    #[test]
    fn a_set_of_questions_is_answered_when_each_has_a_pick_or_a_reason() {
        let src = DECISION;
        let opts = json!([{"id": "a", "title": "A"}, {"id": "b", "title": "B"}]);
        let spec = json!({"title": "Open questions", "questions": [
            {"id": "names", "question": "Names?", "options": opts},
            {"id": "store", "question": "Store?", "options": opts}]});
        let s = run(src, "init", &json!({}), &spec, "agent", 1).unwrap();
        assert!(run(src, "pick", &s, &json!({"option": "a"}), "person", 2).unwrap_err().contains("names, store"));

        let s = run(src, "note", &s, &json!({"q": "store", "text": "  only if it stays local "}), "person", 2).unwrap();
        assert_eq!(s.pointer("/questions/1/note"), Some(&json!({"text": "only if it stays local", "by": "person", "at_ms": 2})));
        assert_eq!(s["status"], "open", "a note alone does not answer");

        let s = run(src, "pick", &s, &json!({"q": "names", "option": "b"}), "person", 3).unwrap();
        assert_eq!(s.pointer("/questions/0/answer"), Some(&json!({"kind": "pick", "option": "b", "by": "person", "at_ms": 3})));
        assert_eq!(s["status"], "open");
        assert!(run(src, "pick", &s, &json!({"q": "store", "option": null, "note": " "}), "person", 4).is_err());
        let s = run(src, "pick", &s, &json!({"q": "store", "option": null}), "person", 4).unwrap();
        assert_eq!(s.pointer("/questions/1/answer/kind"), Some(&json!("none")), "the existing note is the reason");
        assert_eq!(s["status"], "answered");
        assert!(run(src, "note", &s, &json!({"q": "store", "text": ""}), "person", 5).unwrap_err().contains("clear the answer"));

        let s = run(src, "applied", &s, &json!({"note": "done"}), "agent", 5).unwrap();
        assert_eq!(s["status"], "applied");
        let s = run(src, "clear", &s, &json!({"q": "names"}), "person", 6).unwrap();
        assert_eq!((s["status"].clone(), s["applied"].clone()), (json!("open"), json!(null)));
        assert!(run(src, "applied", &s, &json!({}), "agent", 7).is_err());

        let s = run(src, "add", &s, &json!({"text": "What about remote access?"}), "person", 7).unwrap();
        let s = run(src, "add", &s, &json!({"text": "I work on two machines"}), "person", 8).unwrap();
        assert!(run(src, "add", &s, &json!({"text": "  "}), "person", 9).is_err());
        let s = run(src, "drop", &s, &json!({"id": "a1"}), "person", 9).unwrap();
        assert_eq!(s["added"], json!([{"id": "a2", "text": "I work on two machines", "by": "person", "at_ms": 8}]));
        let s = run(src, "add", &s, &json!({"text": "third"}), "person", 10).unwrap();
        assert_eq!(s.pointer("/added/1/id"), Some(&json!("a3")), "ids are not reused");
    }

    #[test]
    fn check_lists_verbs() {
        assert_eq!(check(VERBS).unwrap(), vec!["label", "reset", "boom", "spin"]);
    }
}
