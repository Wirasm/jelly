//! The jellies and every change to them.
//!
//! A jelly is a view (HTML), verbs (JavaScript) and state (JSON) held together
//! under one name. State changes only through verbs, so a person's click and an
//! agent's call go through the same code and produce the same event. Every
//! change keeps an undo snapshot and is appended to `events.jsonl`.

use crate::verbs;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const UNDO_DEPTH: usize = 100;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Jelly {
    pub name: String,
    pub view: String,
    pub verbs: String,
    pub state: Value,
    /// Bumps on every change of any kind.
    pub version: u64,
    /// Bumps when the view or verbs change; an open view reloads when it does.
    pub view_rev: u64,
    pub pinned: bool,
    pub made_by: String,
    pub updated_ms: u64,
    /// When the agent made it; the surface's stream is in this order.
    #[serde(default)]
    pub created_ms: u64,
    /// One line from the agent on what this jelly is for, shown beside it.
    #[serde(default)]
    pub about: String,
    /// Hidden from the surface after a quiet week; any change brings it back.
    #[serde(default)]
    pub archived: bool,
    /// Comments pinned to elements of the view. Kept apart from state: they are
    /// about the jelly, not part of it, and undo does not touch them.
    #[serde(default)]
    pub feedback: Vec<Feedback>,
    /// What the agent put in front of the person: asks to answer, things to
    /// look at. Beside state like comments; undo does not touch them.
    #[serde(default)]
    pub attention: Vec<Attention>,
    /// Set while an agent says it is changing this jelly, so the person waits.
    #[serde(default)]
    pub working: Option<Value>,
    /// Notes agents leave for the next agent: what this jelly is for, how its
    /// verbs are meant to be used, what the person said. `jelly see` shows them.
    #[serde(default)]
    pub notes: Vec<Value>,
}

/// One item for the person's attention. `ask` wants an answer (an option, or
/// text); `look` wants a glance (seen, or a word back).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Attention {
    pub id: String,
    pub kind: String,
    pub title: String,
    #[serde(default)]
    pub detail: String,
    /// Optional element it is about: `{"selector": "#hero", "quote": "…"}`.
    #[serde(default)]
    pub anchor: Value,
    /// For asks: `[{"id": "a", "label": "…", "does": "what picking it does"}]`.
    #[serde(default)]
    pub options: Vec<Value>,
    #[serde(default)]
    pub recommend: Option<String>,
    #[serde(default)]
    pub why: String,
    pub by: String,
    pub at_ms: u64,
    /// `{"option"?, "text"?, "by", "at_ms"}` once answered; `{"withdrawn": true, …}` if the agent took it back.
    #[serde(default)]
    pub answer: Option<Value>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Feedback {
    pub id: String,
    /// Where in the view: `{"selector": "#opt-b .preview", "quote": "first words"}`.
    pub anchor: Value,
    pub text: String,
    pub by: String,
    pub at_ms: u64,
    pub resolved: bool,
    /// The thread under the comment: the agent's answer, the person's follow-up.
    #[serde(default)]
    pub replies: Vec<Reply>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Reply {
    pub text: String,
    pub by: String,
    pub at_ms: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Snapshot {
    view: String,
    verbs: String,
    state: Value,
    view_rev: u64,
}

#[derive(Serialize, Deserialize, Debug)]
struct Entry {
    jelly: Jelly,
    undo: Vec<Snapshot>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Event {
    pub seq: u64,
    pub at_ms: u64,
    pub jelly: String,
    /// new | call | reshape | undo | pin | project | remove | comment | resolve | reply
    pub kind: String,
    pub by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verb: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Value>,
    /// JSON pointers into the state that this change touched.
    pub changed: Vec<String>,
    /// For a person's verb call: the element they used, `{"selector", "text"}`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source: Option<Value>,
    pub version: u64,
    pub view_rev: u64,
    /// Sent to live listeners, never written to the log.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<Value>,
}

#[derive(Debug, PartialEq)]
pub enum Error {
    NotFound(String),
    Exists(String),
    Invalid(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::NotFound(n) => write!(f, "no jelly named {n}"),
            Error::Exists(n) => write!(f, "a jelly named {n} already exists; reshape it or pass replace"),
            Error::Invalid(m) => f.write_str(m),
        }
    }
}

pub struct Store {
    dir: PathBuf,
    jellies: BTreeMap<String, Entry>,
    seq: u64,
    /// The element behind the verb call being committed; taken by the next event.
    source: Option<Value>,
}

impl Store {
    pub fn open(dir: &Path) -> std::io::Result<Store> {
        fs::create_dir_all(dir.join("jellies"))?;
        let mut jellies = BTreeMap::new();
        for file in fs::read_dir(dir.join("jellies"))? {
            let path = file?.path();
            if path.extension().is_some_and(|e| e == "json") {
                match serde_json::from_slice::<Entry>(&fs::read(&path)?) {
                    Ok(entry) => {
                        jellies.insert(entry.jelly.name.clone(), entry);
                    }
                    Err(e) => eprintln!("skipping {}: {e}", path.display()),
                }
            }
        }
        // Sequence numbers continue across restarts, so an event's seq is unique.
        let seq = fs::read_to_string(dir.join("events.jsonl")).ok()
            .and_then(|t| t.lines().rev().find_map(|l| serde_json::from_str::<Value>(l).ok()?["seq"].as_u64()))
            .unwrap_or(0);
        Ok(Store { dir: dir.to_path_buf(), jellies, seq, source: None })
    }

    pub fn list(&self) -> impl Iterator<Item = &Jelly> {
        self.jellies.values().map(|e| &e.jelly)
    }

    pub fn get(&self, name: &str) -> Result<&Jelly, Error> {
        self.jellies.get(name).map(|e| &e.jelly).ok_or_else(|| Error::NotFound(name.into()))
    }

    #[allow(clippy::too_many_arguments)] // one call site per caller; a struct would only rename the arguments
    pub fn create(
        &mut self,
        name: &str,
        view: String,
        verbs_src: String,
        state: Value,
        by: &str,
        replace: bool,
        about: &str,
        project: &str,
    ) -> Result<Event, Error> {
        // A jelly's id is `project:name`, so names only need to be unique in a project.
        let id = if project.is_empty() || name.contains(':') { name.to_string() } else { format!("{project}:{name}") };
        let name = id.as_str();
        valid_name(name)?;
        if self.jellies.contains_key(name) && !replace {
            return Err(Error::Exists(name.into()));
        }
        verbs::check(&verbs_src).map_err(Error::Invalid)?;
        let pinned = self.jellies.get(name).is_some_and(|e| e.jelly.pinned);
        let jelly = Jelly {
            name: name.into(),
            view,
            verbs: verbs_src,
            state,
            version: 1,
            view_rev: 1,
            pinned,
            made_by: by.into(),
            updated_ms: now_ms(),
            created_ms: now_ms(),
            about: about.trim().into(),
            archived: false,
            feedback: Vec::new(),
            attention: Vec::new(),
            working: None,
            notes: Vec::new(),
        };
        self.jellies.insert(name.into(), Entry { jelly, undo: Vec::new() });
        self.commit(name, "new", by, None, None, vec![String::new()])
    }

    pub fn call(&mut self, name: &str, verb: &str, args: Value, by: &str) -> Result<Event, Error> {
        self.call_from(name, verb, args, by, None)
    }

    /// A verb call that also records the element the person used.
    pub fn call_from(&mut self, name: &str, verb: &str, args: Value, by: &str, source: Option<Value>) -> Result<Event, Error> {
        let entry = self.entry(name)?;
        let next = verbs::run(&entry.jelly.verbs, verb, &entry.jelly.state, &args, by, now_ms()).map_err(Error::Invalid)?;
        let mut changed = Vec::new();
        diff(&entry.jelly.state, &next, &mut String::new(), &mut changed);
        snapshot(entry);
        entry.jelly.state = next;
        self.source = source;
        self.commit(name, "call", by, Some(verb.into()), Some(args), changed)
    }

    /// Replaces the view and/or verbs while keeping state.
    /// Replaces the view and/or verbs while keeping state. `about` alone only
    /// relabels the jelly; open views do not reload for it.
    pub fn reshape(&mut self, name: &str, view: Option<String>, verbs_src: Option<String>, about: Option<String>, by: &str) -> Result<Event, Error> {
        if view.is_none() && verbs_src.is_none() && about.is_none() {
            return Err(Error::Invalid("reshape needs a new view, new verbs or a new about".into()));
        }
        if let Some(src) = &verbs_src {
            verbs::check(src).map_err(|e| Error::Invalid(format!("new verbs rejected, jelly unchanged: {e}")))?;
        }
        let entry = self.entry(name)?;
        if let Some(a) = about {
            entry.jelly.about = a.trim().into();
        }
        if view.is_some() || verbs_src.is_some() {
            snapshot(entry);
            if let Some(v) = view {
                entry.jelly.view = v;
            }
            if let Some(v) = verbs_src {
                entry.jelly.verbs = v;
            }
            entry.jelly.view_rev += 1;
        }
        self.commit(name, "reshape", by, None, None, Vec::new())
    }

    pub fn undo(&mut self, name: &str, by: &str) -> Result<Event, Error> {
        let entry = self.entry(name)?;
        let snap = entry.undo.pop().ok_or_else(|| Error::Invalid(format!("{name} has nothing to undo")))?;
        let mut changed = Vec::new();
        diff(&entry.jelly.state, &snap.state, &mut String::new(), &mut changed);
        entry.jelly.view = snap.view;
        entry.jelly.verbs = snap.verbs;
        entry.jelly.state = snap.state;
        if entry.jelly.view_rev != snap.view_rev {
            // A new number, not the old one: views opened on the undone
            // version must notice and reload.
            entry.jelly.view_rev += 1;
        }
        self.commit(name, "undo", by, None, None, changed)
    }

    /// Pins a comment to part of the view.
    pub fn comment(&mut self, name: &str, anchor: Value, text: &str, by: &str) -> Result<Event, Error> {
        let text = text.trim();
        if text.is_empty() {
            return Err(Error::Invalid("empty comment".into()));
        }
        let entry = self.entry(name)?;
        let id = format!("f{}", entry.jelly.feedback.len() + 1);
        entry.jelly.feedback.push(Feedback { id: id.clone(), anchor: anchor.clone(), text: text.into(), by: by.into(), at_ms: now_ms(), resolved: false, replies: Vec::new() });
        let args = serde_json::json!({"id": id, "anchor": anchor, "text": text});
        self.commit(name, "comment", by, None, Some(args), Vec::new())
    }

    pub fn resolve(&mut self, name: &str, id: &str, by: &str) -> Result<Event, Error> {
        let entry = self.entry(name)?;
        let f = entry.jelly.feedback.iter_mut().find(|f| f.id == id).ok_or_else(|| Error::Invalid(format!("{name} has no comment {id}")))?;
        f.resolved = true;
        self.commit(name, "resolve", by, None, Some(Value::String(id.into())), Vec::new())
    }

    /// Answers on a comment's thread. A reply leaves the thread resolved only
    /// when it says so: the agent's "done" can close it, and the person's
    /// follow-up on a closed thread opens it again.
    pub fn reply(&mut self, name: &str, id: &str, text: &str, by: &str, resolve: bool) -> Result<Event, Error> {
        let text = text.trim();
        if text.is_empty() {
            return Err(Error::Invalid("empty reply".into()));
        }
        let entry = self.entry(name)?;
        let f = entry.jelly.feedback.iter_mut().find(|f| f.id == id).ok_or_else(|| Error::Invalid(format!("{name} has no comment {id}")))?;
        f.replies.push(Reply { text: text.into(), by: by.into(), at_ms: now_ms() });
        f.resolved = resolve;
        let args = serde_json::json!({"id": id, "text": text, "resolved": resolve, "anchor": f.anchor, "comment": f.text});
        self.commit(name, "reply", by, None, Some(args), Vec::new())
    }

    /// Moves a jelly to another project ("" for none). Its id changes from
    /// `old:name` to `new:name`; assets move with it.
    pub fn set_project(&mut self, name: &str, project: &str, by: &str) -> Result<Event, Error> {
        let short = name.split_once(':').map_or(name, |(_, n)| n);
        let id = if project.is_empty() { short.to_string() } else { format!("{project}:{short}") };
        valid_name(&id)?;
        if id == name {
            return Err(Error::Invalid(format!("{name} is already in that project")));
        }
        if self.jellies.contains_key(&id) {
            return Err(Error::Exists(id));
        }
        let mut entry = self.jellies.remove(name).ok_or_else(|| Error::NotFound(name.into()))?;
        entry.jelly.name = id.clone();
        self.jellies.insert(id.clone(), entry);
        let _ = fs::remove_file(self.path(name));
        if self.asset_dir(name).exists() {
            let _ = fs::rename(self.asset_dir(name), self.asset_dir(&id));
        }
        self.commit(&id, "project", by, None, Some(serde_json::json!({"from": name})), Vec::new())
    }

    /// Archives unpinned jellies nobody touched for `idle_ms`, unless they still
    /// wait on the person (an open decision or an open comment).
    pub fn sweep(&mut self, idle_ms: u64) -> Vec<Event> {
        let cutoff = now_ms().saturating_sub(idle_ms);
        let stale: Vec<String> = self.list()
            .filter(|j| !j.pinned && !j.archived && j.name != "surface" && j.updated_ms < cutoff)
            .filter(|j| !(j.state.get("questions").or_else(|| j.state.get("options")).is_some() && j.state.get("status") == Some(&Value::from("open"))))
            .filter(|j| j.feedback.iter().all(|f| f.resolved))
            .filter(|j| j.attention.iter().all(|a| a.answer.is_some()))
            .map(|j| j.name.clone())
            .collect();
        stale.iter().filter_map(|n| self.set_archived(n, true, "jelly").ok()).collect()
    }

    pub fn set_archived(&mut self, name: &str, archived: bool, by: &str) -> Result<Event, Error> {
        self.entry(name)?.jelly.archived = archived;
        self.commit(name, if archived { "archive" } else { "restore" }, by, None, None, Vec::new())
    }

    /// An agent's note for the next agent.
    pub fn note(&mut self, name: &str, text: &str, by: &str) -> Result<Event, Error> {
        let text = text.trim();
        if text.is_empty() {
            return Err(Error::Invalid("empty note".into()));
        }
        self.entry(name)?.jelly.notes.push(serde_json::json!({"text": text, "by": by, "at_ms": now_ms()}));
        self.commit(name, "note", by, None, Some(Value::String(text.into())), Vec::new())
    }

    /// Where a project keeps its DESIGN.md.
    pub fn project_dir(&self, project: &str) -> PathBuf {
        self.dir.join("projects").join(project)
    }

    pub fn home(&self) -> &Path {
        &self.dir
    }

    pub fn asset_dir(&self, name: &str) -> PathBuf {
        self.dir.join("assets").join(name)
    }

    /// Saves a file next to a jelly, outside its state and undo history.
    pub fn put_asset(&mut self, name: &str, file: &str, bytes: &[u8], by: &str) -> Result<Event, Error> {
        valid_file(file)?;
        let dir = self.asset_dir(name);
        self.entry(name)?;
        fs::create_dir_all(&dir).and_then(|_| write_atomic(&dir.join(file), bytes)).map_err(|e| Error::Invalid(format!("could not save {file}: {e}")))?;
        self.commit(name, "asset", by, None, Some(serde_json::json!({"file": file, "bytes": bytes.len()})), Vec::new())
    }

    pub fn assets(&self, name: &str) -> Vec<String> {
        let mut out: Vec<String> = fs::read_dir(self.asset_dir(name)).into_iter().flatten().flatten()
            .filter_map(|e| e.file_name().into_string().ok()).filter(|f| !f.starts_with('.')).collect();
        out.sort();
        out
    }

    /// Puts something in front of the person: `{"kind": "ask"|"look", "title", "detail"?,
    /// "anchor"?, "options"?, "recommend"?, "why"?}`.
    pub fn attend(&mut self, name: &str, spec: Value, by: &str) -> Result<Event, Error> {
        let kind = spec["kind"].as_str().unwrap_or("ask");
        if kind != "ask" && kind != "look" {
            return Err(Error::Invalid(format!("attention kind must be ask or look, not {kind:?}")));
        }
        let title = spec["title"].as_str().unwrap_or("").trim().to_string();
        if title.is_empty() {
            return Err(Error::Invalid("attention needs a title: what you want from the person".into()));
        }
        let options: Vec<Value> = spec["options"].as_array().cloned().unwrap_or_default();
        let mut ids = std::collections::HashSet::new();
        for o in &options {
            let id = o["id"].as_str().filter(|i| !i.is_empty()).ok_or_else(|| Error::Invalid("every option needs an id".into()))?;
            if !ids.insert(id.to_string()) {
                return Err(Error::Invalid(format!("option id {id:?} appears twice")));
            }
        }
        let recommend = spec["recommend"].as_str().map(String::from);
        if let Some(r) = &recommend {
            if !ids.contains(r) {
                return Err(Error::Invalid(format!("recommend {r:?} is not one of the options")));
            }
        }
        let entry = self.entry(name)?;
        let id = format!("a{}", entry.jelly.attention.len() + 1);
        let item = Attention {
            id: id.clone(),
            kind: kind.into(),
            title,
            detail: spec["detail"].as_str().unwrap_or("").into(),
            anchor: spec.get("anchor").cloned().unwrap_or(Value::Null),
            options,
            recommend,
            why: spec["why"].as_str().unwrap_or("").into(),
            by: by.into(),
            at_ms: now_ms(),
            answer: None,
        };
        let args = serde_json::to_value(&item).expect("serializes");
        entry.jelly.attention.push(item);
        self.commit(name, "attention", by, None, Some(args), Vec::new())
    }

    /// The person answers an item: an option id, text, or both. A look can be
    /// answered with nothing, meaning "seen".
    pub fn answer(&mut self, name: &str, id: &str, option: Option<&str>, text: &str, by: &str) -> Result<Event, Error> {
        let entry = self.entry(name)?;
        let item = entry.jelly.attention.iter_mut().find(|a| a.id == id).ok_or_else(|| Error::Invalid(format!("{name} has no attention item {id}")))?;
        if let Some(o) = option {
            if !item.options.iter().any(|x| x["id"] == o) {
                return Err(Error::Invalid(format!("{o:?} is not an option of {id}")));
            }
        }
        let text = text.trim();
        if item.kind == "ask" && option.is_none() && text.is_empty() {
            return Err(Error::Invalid("an ask needs an option or some text".into()));
        }
        item.answer = Some(serde_json::json!({"option": option, "text": text, "by": by, "at_ms": now_ms()}));
        let args = serde_json::json!({"id": id, "kind": item.kind, "title": item.title, "option": option, "text": text});
        self.commit(name, "answer", by, None, Some(args), Vec::new())
    }

    /// The agent takes an item back, for example because it no longer applies.
    pub fn withdraw(&mut self, name: &str, id: &str, by: &str) -> Result<Event, Error> {
        let entry = self.entry(name)?;
        let item = entry.jelly.attention.iter_mut().find(|a| a.id == id).ok_or_else(|| Error::Invalid(format!("{name} has no attention item {id}")))?;
        item.answer = Some(serde_json::json!({"withdrawn": true, "by": by, "at_ms": now_ms()}));
        self.commit(name, "withdraw", by, None, Some(Value::String(id.into())), Vec::new())
    }

    /// `Some("rebuilding the header")` while an agent works on the jelly, `None` when done.
    pub fn set_working(&mut self, name: &str, what: Option<&str>, by: &str) -> Result<Event, Error> {
        self.entry(name)?.jelly.working = what.map(|w| serde_json::json!({"what": w, "by": by, "at_ms": now_ms()}));
        self.commit(name, "working", by, None, what.map(|w| Value::String(w.into())), Vec::new())
    }

    pub fn pin(&mut self, name: &str, pinned: bool, by: &str) -> Result<Event, Error> {
        self.entry(name)?.jelly.pinned = pinned;
        self.commit(name, "pin", by, None, Some(Value::Bool(pinned)), Vec::new())
    }

    pub fn remove(&mut self, name: &str, by: &str) -> Result<Event, Error> {
        let entry = self.jellies.remove(name).ok_or_else(|| Error::NotFound(name.into()))?;
        let _ = fs::remove_file(self.path(name));
        let _ = fs::remove_dir_all(self.asset_dir(name));
        let ev = self.event(&entry.jelly, "remove", by, None, None, Vec::new());
        self.log(&ev);
        Ok(ev)
    }

    fn entry(&mut self, name: &str) -> Result<&mut Entry, Error> {
        self.jellies.get_mut(name).ok_or_else(|| Error::NotFound(name.into()))
    }

    /// Bumps the version, saves the jelly, logs the event and returns it with state attached.
    fn commit(
        &mut self,
        name: &str,
        kind: &str,
        by: &str,
        verb: Option<String>,
        args: Option<Value>,
        changed: Vec<String>,
    ) -> Result<Event, Error> {
        let entry = self.jellies.get_mut(name).expect("caller checked");
        if kind != "new" {
            entry.jelly.version += 1;
        }
        entry.jelly.updated_ms = now_ms();
        if kind != "archive" {
            entry.jelly.archived = false;
        }
        let bytes = serde_json::to_vec(entry).expect("a jelly serializes");
        let jelly = entry.jelly.clone();
        write_atomic(&self.path(name), &bytes).map_err(|e| Error::Invalid(format!("could not save {name}: {e}")))?;
        let mut ev = self.event(&jelly, kind, by, verb, args, changed);
        self.log(&ev);
        ev.state = Some(jelly.state);
        Ok(ev)
    }

    fn event(&mut self, j: &Jelly, kind: &str, by: &str, verb: Option<String>, args: Option<Value>, changed: Vec<String>) -> Event {
        self.seq += 1;
        Event {
            seq: self.seq,
            at_ms: now_ms(),
            jelly: j.name.clone(),
            kind: kind.into(),
            by: by.into(),
            verb,
            args,
            changed,
            source: self.source.take(),
            version: j.version,
            view_rev: j.view_rev,
            state: None,
        }
    }

    fn log(&self, ev: &Event) {
        let line = serde_json::to_string(ev).expect("an event serializes");
        let file = fs::OpenOptions::new().create(true).append(true).open(self.dir.join("events.jsonl"));
        if let Err(e) = file.and_then(|mut f| writeln!(f, "{line}")) {
            eprintln!("could not log event: {e}");
        }
    }

    pub fn events_path(&self) -> PathBuf {
        self.dir.join("events.jsonl")
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join("jellies").join(format!("{name}.json"))
    }
}

fn snapshot(entry: &mut Entry) {
    let j = &entry.jelly;
    entry.undo.push(Snapshot { view: j.view.clone(), verbs: j.verbs.clone(), state: j.state.clone(), view_rev: j.view_rev });
    if entry.undo.len() > UNDO_DEPTH {
        entry.undo.remove(0);
    }
}

/// Names become file names and URL segments.
/// `name` or `project:name`, each part 1-64 of a-z, 0-9, - and _.
fn valid_name(name: &str) -> Result<(), Error> {
    let part = |p: &str| !p.is_empty() && p.len() <= 64 && p.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
    let ok = match name.split_once(':') {
        Some((project, rest)) => part(project) && part(rest),
        None => part(name),
    };
    if ok { Ok(()) } else { Err(Error::Invalid(format!("bad name {name:?}: use 1-64 of a-z, 0-9, - and _, optionally as project:name"))) }
}

fn valid_file(file: &str) -> Result<(), Error> {
    let ok = !file.is_empty() && file.len() <= 128 && !file.starts_with('.')
        && file.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.');
    if ok { Ok(()) } else { Err(Error::Invalid(format!("bad file name {file:?}: use letters, digits, '.', '-' and '_'"))) }
}

impl Jelly {
    /// The project part of `project:name`, or "".
    pub fn project(&self) -> &str {
        self.name.split_once(':').map_or("", |(p, _)| p)
    }
}

/// Collects the JSON pointers (RFC 6901) where `a` and `b` differ.
pub fn diff(a: &Value, b: &Value, path: &mut String, out: &mut Vec<String>) {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let keys: std::collections::BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            for k in keys {
                let len = path.len();
                path.push('/');
                path.push_str(&k.replace('~', "~0").replace('/', "~1"));
                diff(x.get(k).unwrap_or(&Value::Null), y.get(k).unwrap_or(&Value::Null), path, out);
                if x.contains_key(k) != y.contains_key(k) && !out.contains(path) {
                    out.push(path.clone());
                }
                path.truncate(len);
            }
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
            for (i, (p, q)) in x.iter().zip(y).enumerate() {
                let len = path.len();
                path.push_str(&format!("/{i}"));
                diff(p, q, path, out);
                path.truncate(len);
            }
        }
        _ if a != b => out.push(path.clone()),
        _ => {}
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(tmp, path)
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const VERBS: &str = "const verbs = { label(s, { id, value }) { s.labels[id] = value; } };";

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        (dir, s)
    }

    #[test]
    fn diff_names_changed_pointers() {
        let mut out = Vec::new();
        diff(
            &json!({"a": {"x": 1}, "list": [1, 2], "k/v": 1, "gone": true}),
            &json!({"a": {"x": 2}, "list": [1, 2, 3], "k/v": 2, "new": 0}),
            &mut String::new(),
            &mut out,
        );
        assert_eq!(out, vec!["/a/x", "/gone", "/k~1v", "/list", "/new"]);
    }

    #[test]
    fn calls_reshape_and_undo_keep_state_and_history() {
        let (_d, mut s) = store();
        s.create("labeler", "<p>v1</p>".into(), VERBS.into(), json!({"labels": {}}), "agent", false, "", "").unwrap();

        let ev = s.call("labeler", "label", json!({"id": "c14", "value": "fail"}), "person").unwrap();
        assert_eq!((ev.by.as_str(), ev.changed.clone(), ev.version), ("person", vec!["/labels/c14".to_string()], 2));

        let ev = s.reshape("labeler", Some("<p>v2</p>".into()), None, None, "agent").unwrap();
        assert_eq!(ev.view_rev, 2);
        assert_eq!(s.get("labeler").unwrap().state, json!({"labels": {"c14": "fail"}}), "reshape keeps state");

        let ev = s.undo("labeler", "person").unwrap();
        let j = s.get("labeler").unwrap();
        assert_eq!(j.view, "<p>v1</p>");
        assert_eq!(ev.view_rev, 3, "undoing a reshape is a new view revision");
        assert_eq!(j.state, json!({"labels": {"c14": "fail"}}));

        s.undo("labeler", "person").unwrap();
        assert_eq!(s.get("labeler").unwrap().state, json!({"labels": {}}));
        assert!(s.undo("labeler", "person").is_err());
    }

    #[test]
    fn bad_verbs_never_replace_good_ones() {
        let (_d, mut s) = store();
        s.create("j", "".into(), VERBS.into(), json!({"labels": {}}), "agent", false, "", "").unwrap();
        let err = s.reshape("j", None, Some("const verbs = {".into()), None, "agent").unwrap_err();
        assert!(matches!(err, Error::Invalid(m) if m.contains("unchanged")));
        assert_eq!(s.get("j").unwrap().verbs, VERBS);
        assert!(s.create("j", "".into(), VERBS.into(), json!({}), "agent", false, "", "").is_err());
        assert!(s.create("../x", "".into(), VERBS.into(), json!({}), "agent", false, "", "").is_err());
    }

    #[test]
    fn comments_sit_beside_state_and_survive_undo() {
        let (_d, mut s) = store();
        s.create("j", "".into(), VERBS.into(), json!({"labels": {}}), "agent", false, "", "").unwrap();
        s.call("j", "label", json!({"id": "a", "value": "pass"}), "person").unwrap();
        let ev = s.comment("j", json!({"selector": "#a"}), "too small", "person").unwrap();
        assert_eq!((ev.kind.as_str(), ev.args.clone().unwrap()["id"].clone()), ("comment", json!("f1")));
        s.undo("j", "person").unwrap();
        let j = s.get("j").unwrap();
        assert_eq!(j.state, json!({"labels": {}}));
        assert_eq!(j.feedback.len(), 1, "undo is about state, not comments");
        s.resolve("j", "f1", "agent").unwrap();
        assert!(s.get("j").unwrap().feedback[0].resolved);
        assert!(s.comment("j", json!({}), "  ", "person").is_err());
    }

    #[test]
    fn names_are_unique_per_project() {
        let (_d, mut s) = store();
        s.create("plan", "".into(), VERBS.into(), json!({}), "agent", false, "", "archon").unwrap();
        s.create("plan", "".into(), VERBS.into(), json!({}), "agent", false, "", "jelly").unwrap();
        assert_eq!(s.get("archon:plan").unwrap().project(), "archon");
        s.put_asset("archon:plan", "shot.png", b"png", "agent").unwrap();
        let ev = s.set_project("archon:plan", "rex", "person").unwrap();
        assert_eq!(ev.jelly, "rex:plan");
        assert!(s.get("archon:plan").is_err());
        assert_eq!(s.assets("rex:plan"), ["shot.png"], "assets move with the jelly");
        assert!(s.set_project("rex:plan", "jelly", "person").is_err(), "jelly:plan exists");
        assert!(s.put_asset("rex:plan", "../x", b"", "agent").is_err());
        assert!(s.create("k", "".into(), VERBS.into(), json!({}), "agent", false, "", "Bad Project").is_err());
    }

    #[test]
    fn attention_items_are_answered_beside_state() {
        let (_d, mut s) = store();
        s.create("j", "".into(), VERBS.into(), json!({"labels": {}}), "agent", false, "", "").unwrap();
        let spec = json!({"kind": "ask", "title": "Which header?", "options": [{"id": "a", "label": "Bold"}, {"id": "b", "label": "Quiet"}], "recommend": "b"});
        s.attend("j", spec, "claude").unwrap();
        s.attend("j", json!({"kind": "look", "title": "Check the footer", "anchor": {"selector": "#f"}}), "claude").unwrap();
        assert!(s.attend("j", json!({"title": "x", "options": [{"id": "a"}], "recommend": "z"}), "claude").is_err());
        assert!(s.answer("j", "a1", Some("c"), "", "person").is_err(), "not an option");
        assert!(s.answer("j", "a1", None, " ", "person").is_err(), "an ask needs something");
        let ev = s.answer("j", "a1", Some("b"), "but larger", "person").unwrap();
        assert_eq!(ev.args.unwrap()["option"], "b");
        s.answer("j", "a2", None, "", "person").unwrap();
        assert!(s.sweep(0).is_empty() || s.get("j").unwrap().attention.iter().all(|a| a.answer.is_some()));
        s.undo("j", "person").ok();
        assert!(s.get("j").unwrap().attention[0].answer.is_some(), "undo leaves answers alone");
    }

    #[test]
    fn verb_calls_remember_their_source_and_seq_survives_restart() {
        let (dir, mut s) = store();
        s.create("j", "".into(), VERBS.into(), json!({"labels": {}}), "agent", false, "", "").unwrap();
        let ev = s.call_from("j", "label", json!({"id": "a", "value": "x"}), "person", Some(json!({"selector": "#pick-b"}))).unwrap();
        assert_eq!(ev.source.unwrap()["selector"], "#pick-b");
        let seq = s.call("j", "label", json!({"id": "a", "value": "y"}), "agent").unwrap().seq;
        drop(s);
        let mut s = Store::open(dir.path()).unwrap();
        let next = s.call("j", "label", json!({"id": "a", "value": "z"}), "agent").unwrap();
        assert_eq!(next.seq, seq + 1);
        assert!(next.source.is_none());
    }

    #[test]
    fn quiet_jellies_archive_and_come_back_on_change() {
        let (_d, mut s) = store();
        s.create("old", "".into(), VERBS.into(), json!({"labels": {}}), "agent", false, "", "").unwrap();
        s.create("kept", "".into(), VERBS.into(), json!({}), "agent", false, "", "").unwrap();
        s.pin("kept", true, "person").unwrap();
        s.create("asked", "".into(), VERBS.into(), json!({}), "agent", false, "", "").unwrap();
        s.comment("asked", json!({}), "why?", "person").unwrap();
        let swept: Vec<String> = s.sweep(0).into_iter().map(|e| e.jelly).collect();
        assert_eq!(swept, ["old"], "pinned and open-comment jellies stay");
        assert!(s.get("old").unwrap().archived);
        s.call("old", "label", json!({"id": "a", "value": "x"}), "agent").unwrap();
        assert!(!s.get("old").unwrap().archived, "a change brings it back");
    }

    #[test]
    fn replies_thread_under_a_comment() {
        let (_d, mut s) = store();
        s.create("j", "".into(), VERBS.into(), json!({}), "agent", false, "", "").unwrap();
        s.comment("j", json!({"selector": "#a"}), "split this", "person").unwrap();
        s.reply("j", "f1", "Done, reshaped v2", "claude", true).unwrap();
        assert!(s.get("j").unwrap().feedback[0].resolved);
        s.reply("j", "f1", "not quite", "person", false).unwrap();
        let f = &s.get("j").unwrap().feedback[0];
        assert!(!f.resolved, "a follow-up opens the thread again");
        assert_eq!(f.replies.iter().map(|r| r.by.as_str()).collect::<Vec<_>>(), ["claude", "person"]);
        assert!(s.reply("j", "f9", "x", "claude", false).is_err());
    }

    #[test]
    fn jellies_survive_a_restart() {
        let (dir, mut s) = store();
        s.create("j", "".into(), VERBS.into(), json!({"labels": {}}), "agent", false, "", "").unwrap();
        s.call("j", "label", json!({"id": "a", "value": "pass"}), "person").unwrap();
        drop(s);
        let mut s = Store::open(dir.path()).unwrap();
        assert_eq!(s.get("j").unwrap().state, json!({"labels": {"a": "pass"}}));
        s.undo("j", "person").unwrap();
        assert_eq!(s.get("j").unwrap().state, json!({"labels": {}}), "undo history is saved too");
    }
}
