//! The changes a shared jelly's relay can make too. A `Change` is made in the
//! store here (`Change::apply`), or, while the jelly is shared, sent as it is
//! to the relay (`share::forward`), whose `relay/src/ops.ts` makes the same
//! change. `tests/ops.json` holds what each must do; both sides run it.

use crate::store::{Ask, Error, Event, Store};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One change and who makes it. On the wire to the relay it is one object:
/// `{"op": "do", "verb": "pick", "args": {...}, "by": "agent"}`.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Change {
    pub by: String,
    #[serde(flatten)]
    pub op: Op,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Op {
    /// A verb call. `source` is the element the person used; a `quiet` call
    /// does not count toward Send.
    Do {
        verb: String,
        #[serde(default)]
        args: Value,
        #[serde(default)]
        source: Option<Value>,
        #[serde(default)]
        quiet: bool,
    },
    /// Puts `value` at a JSON pointer in the state, or removes what is there.
    Set {
        #[serde(default)]
        pointer: String,
        #[serde(default)]
        value: Value,
        #[serde(default)]
        remove: bool,
    },
    Shape {
        #[serde(default)]
        view: Option<String>,
        #[serde(default)]
        verbs: Option<String>,
        #[serde(default)]
        about: Option<String>,
    },
    /// An ask checked here (`store::ask_item`); the store or the relay numbers it.
    Ask { item: Box<Ask> },
    Unask { id: String },
    Answer {
        id: String,
        #[serde(default)]
        option: Option<String>,
        #[serde(default)]
        text: String,
    },
    Comment {
        #[serde(default)]
        anchor: Value,
        #[serde(default)]
        text: String,
    },
    Reply {
        id: String,
        #[serde(default)]
        text: String,
        #[serde(default)]
        resolve: bool,
    },
    Resolve { id: String },
    Send,
}

impl Change {
    /// Makes the change in the store.
    pub fn apply(self, store: &mut Store, name: &str) -> Result<Event, Error> {
        let by = self.by.as_str();
        match self.op {
            Op::Do { verb, args, source, quiet } => store.do_verb_from(name, &verb, args, by, source, quiet),
            Op::Set { pointer, value, remove } => store.set_state(name, &pointer, (!remove).then_some(value), by),
            Op::Shape { view, verbs, about } => store.shape(name, view, verbs, about, by),
            Op::Ask { item } => store.put_ask(name, *item, by),
            Op::Unask { id } => store.unask(name, &id, by),
            Op::Answer { id, option, text } => store.answer(name, &id, option.as_deref(), &text, by),
            Op::Comment { anchor, text } => store.comment(name, anchor, &text, by),
            Op::Reply { id, text, resolve } => store.reply(name, &id, &text, by, resolve),
            Op::Resolve { id } => store.resolve(name, &id, by),
            Op::Send => store.send(name, by),
        }
    }

    /// What the relay trusts the daemon to have checked, as the store checks
    /// it: new verbs load.
    pub fn check(&self) -> Result<(), Error> {
        match &self.op {
            Op::Shape { verbs: Some(src), .. } => crate::verbs::check(src).map(drop).map_err(|e| Error::Invalid(format!("new verbs rejected, jelly unchanged: {e}"))),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Runs every case in `tests/ops.json` against the store; `relay/test/ops.test.mjs`
    /// runs the same cases against the relay.
    #[test]
    fn the_store_makes_every_change_as_tests_ops_json_says() {
        let fixtures: Value = serde_json::from_str(include_str!("../tests/ops.json")).unwrap();
        for case in fixtures["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let dir = tempfile::tempdir().unwrap();
            let mut store = Store::open(dir.path()).unwrap();
            let j = &case["jelly"];
            let text = |k: &str, or: &str| j[k].as_str().unwrap_or(or).to_string();
            let state = j.get("state").cloned().unwrap_or(json!({}));
            store.make("t:j", text("view", ""), text("verbs", "const verbs = {}"), state, "agent", false, &text("about", ""), "").unwrap();
            for (i, step) in case["steps"].as_array().unwrap().iter().enumerate() {
                let at = format!("{name}, step {}", i + 1);
                let change: Change = serde_json::from_value(step["change"].clone()).unwrap_or_else(|e| panic!("{at}: {e}"));
                match change.apply(&mut store, "t:j") {
                    Ok(ev) => {
                        assert!(step.get("refused").is_none(), "{at}: made the change, but should refuse it with {}", step["refused"]);
                        assert_eq!(made(&serde_json::to_value(&ev).unwrap()), made(&step["event"]), "{at}: event");
                    }
                    Err(e) => assert_eq!(Some(e.to_string().as_str()), step["refused"].as_str(), "{at}: refused"),
                }
                let jelly = serde_json::to_value(store.get("t:j").unwrap()).unwrap();
                for (k, want) in step["jelly"].as_object().into_iter().flatten() {
                    let mut got = jelly[k].clone();
                    if k == "comments" || k == "asks" { untimed(&mut got); }
                    assert_eq!(&got, want, "{at}: jelly {k}");
                }
            }
        }
    }

    /// The relay runs whatever verbs it is sent, so a shared jelly's new verbs
    /// are checked before they go, with the store's own words.
    #[test]
    fn verbs_that_do_not_load_never_reach_the_relay() {
        let shape = |verbs: &str| Change { by: "agent".into(), op: Op::Shape { view: None, verbs: Some(verbs.into()), about: None } };
        let err = shape("const verbs = {").check().unwrap_err().to_string();
        assert!(err.starts_with("new verbs rejected, jelly unchanged: verbs did not load: SyntaxError"), "{err}");
        assert!(shape("const verbs = { a() {} }").check().is_ok());
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        store.make("j", "".into(), "const verbs = {}".into(), json!({}), "agent", false, "", "").unwrap();
        assert_eq!(shape("const verbs = {").apply(&mut store, "j").unwrap_err().to_string(), err);
    }

    /// Undo is not a change a shared jelly takes (the relay refuses it), so it
    /// is not in tests/ops.json; it counts toward Send at home as `countsTowardSend` says.
    #[test]
    fn a_persons_undo_counts_toward_send() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        store.make("j", "".into(), "const verbs = { inc(s) { s.n = 1; } }".into(), json!({}), "agent", false, "", "").unwrap();
        store.do_verb("j", "inc", Value::Null, "agent").unwrap();
        store.undo("j", "person:ana").unwrap();
        assert_eq!(store.get("j").unwrap().unsent, 1);
        store.do_verb("j", "inc", Value::Null, "agent").unwrap();
        store.undo("j", "agent").unwrap();
        assert_eq!(store.get("j").unwrap().unsent, 1);
    }

    /// What both sides put in an event: absent `verb`, `args` and `source` are
    /// null, and an absent `quiet` is false.
    fn made(ev: &Value) -> Value {
        json!({"kind": ev["kind"], "by": ev["by"], "verb": ev["verb"], "args": ev["args"], "source": ev["source"],
            "quiet": ev["quiet"].as_bool().unwrap_or(false), "version": ev["version"], "view_rev": ev["view_rev"]})
    }

    /// Drops every `at_ms` from comments and asks: when a change happened is not part of what it does.
    fn untimed(v: &mut Value) {
        match v {
            Value::Object(m) => { m.shift_remove("at_ms"); m.values_mut().for_each(untimed); }
            Value::Array(a) => a.iter_mut().for_each(untimed),
            _ => {}
        }
    }
}
