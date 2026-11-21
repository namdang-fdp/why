//! Editing a user's JSON config WITHOUT rewriting it.
//!
//! The obvious way to add a hook is: parse the file into a `serde_json::Value`,
//! insert, serialize back. That reformats the whole file (indentation, key
//! order, escapes, spacing), so the preview would show a diff full of changes
//! the user never asked for, and a file kept in a dotfiles repo would churn.
//!
//! Instead we parse once to learn where every value sits (byte spans), then
//! SPLICE text: insert our block at one place, or cut our block out. Everything
//! outside the splice stays byte-for-byte as it was: formatting, unicode
//! escapes, a trailing newline or the lack of one.
//!
//! Safety first, repair never: [`serde_json`] validates the input (strict JSON,
//! no comments) and a file that is invalid, whose root is not an object, whose
//! `hooks` is not an object, or whose keys we would have to guess between
//! (duplicates) is refused, never "fixed".

use std::collections::HashSet;

use serde_json::Value;

/// Parse failures use plain strings: they are shown to the user as is.
pub type Result<T> = std::result::Result<T, String>;

// ───────────────────────── span parser ─────────────────────────

#[derive(Debug, Clone)]
struct Node {
    start: usize,
    end: usize,
    kind: Kind,
}

#[derive(Debug, Clone)]
enum Kind {
    Object(Vec<Member>),
    Array(Vec<Node>),
    Str,
    Other,
}

#[derive(Debug, Clone)]
struct Member {
    key: String,
    /// Where the key's opening quote is.
    start: usize,
    value: Node,
}

impl Node {
    fn members(&self) -> &[Member] {
        match &self.kind {
            Kind::Object(m) => m,
            _ => &[],
        }
    }
    fn elements(&self) -> &[Node] {
        match &self.kind {
            Kind::Array(e) => e,
            _ => &[],
        }
    }
    fn get(&self, key: &str) -> Option<&Member> {
        self.members().iter().find(|m| m.key == key)
    }
    fn is_object(&self) -> bool {
        matches!(self.kind, Kind::Object(_))
    }
    fn is_array(&self) -> bool {
        matches!(self.kind, Kind::Array(_))
    }
    fn text<'a>(&self, text: &'a str) -> &'a str {
        &text[self.start..self.end]
    }
    /// The decoded value if this is a JSON string.
    fn as_string(&self, text: &str) -> Option<String> {
        match self.kind {
            Kind::Str => serde_json::from_str(self.text(text)).ok(),
            _ => None,
        }
    }
    /// Spans of the children as the removal/insertion helpers see them: for an
    /// object a member runs from its key to the end of its value.
    fn child_spans(&self) -> Vec<(usize, usize)> {
        match &self.kind {
            Kind::Object(m) => m.iter().map(|m| (m.start, m.value.end)).collect(),
            Kind::Array(e) => e.iter().map(|n| (n.start, n.end)).collect(),
            _ => Vec::new(),
        }
    }
}

struct Parser<'a> {
    text: &'a str,
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.i).copied()
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.i += 1;
        }
    }

    fn string(&mut self) -> Option<String> {
        let start = self.i;
        if self.peek()? != b'"' {
            return None;
        }
        self.i += 1;
        loop {
            match self.peek()? {
                b'\\' => self.i += 2,
                b'"' => {
                    self.i += 1;
                    break;
                }
                _ => self.i += 1,
            }
        }
        serde_json::from_str(self.text.get(start..self.i)?).ok()
    }

    fn value(&mut self, depth: usize) -> Option<Node> {
        // serde_json refuses deeper nesting too; this keeps our recursion bounded.
        if depth > 128 {
            return None;
        }
        self.ws();
        let start = self.i;
        let kind = match self.peek()? {
            b'{' => {
                self.i += 1;
                let mut members = Vec::new();
                self.ws();
                if self.peek()? == b'}' {
                    self.i += 1;
                } else {
                    loop {
                        self.ws();
                        let key_start = self.i;
                        let key = self.string()?;
                        self.ws();
                        if self.peek()? != b':' {
                            return None;
                        }
                        self.i += 1;
                        let value = self.value(depth + 1)?;
                        members.push(Member {
                            key,
                            start: key_start,
                            value,
                        });
                        self.ws();
                        match self.peek()? {
                            b',' => self.i += 1,
                            b'}' => {
                                self.i += 1;
                                break;
                            }
                            _ => return None,
                        }
                    }
                }
                Kind::Object(members)
            }
            b'[' => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.peek()? == b']' {
                    self.i += 1;
                } else {
                    loop {
                        items.push(self.value(depth + 1)?);
                        self.ws();
                        match self.peek()? {
                            b',' => self.i += 1,
                            b']' => {
                                self.i += 1;
                                break;
                            }
                            _ => return None,
                        }
                    }
                }
                Kind::Array(items)
            }
            b'"' => {
                self.string()?;
                Kind::Str
            }
            _ => {
                while !matches!(
                    self.peek(),
                    None | Some(b',' | b']' | b'}' | b' ' | b'\t' | b'\r' | b'\n')
                ) {
                    self.i += 1;
                }
                Kind::Other
            }
        };
        Some(Node {
            start,
            end: self.i,
            kind,
        })
    }
}

/// The validated document: the root object and where `hooks` is in it.
struct Doc {
    root: Node,
}

impl Doc {
    fn hooks(&self) -> Option<&Member> {
        self.root.get("hooks")
    }
}

fn parse_doc(text: &str) -> Result<Doc> {
    serde_json::from_str::<Value>(text).map_err(|e| format!("not valid JSON ({e})"))?;
    let mut p = Parser { text, i: 0 };
    let root = p
        .value(0)
        .filter(|_| {
            p.ws();
            p.i == text.len()
        })
        .ok_or("cannot read the JSON structure")?;
    if !root.is_object() {
        return Err("the top level is not a JSON object".into());
    }
    let doc = Doc { root };
    // Two `hooks` keys: which one wins is up to the reader; we will not guess.
    if doc
        .root
        .members()
        .iter()
        .filter(|m| m.key == "hooks")
        .count()
        > 1
    {
        return Err("the file has more than one `hooks` key".into());
    }
    if let Some(hooks) = doc.hooks() {
        if !hooks.value.is_object() {
            return Err("`hooks` is not an object".into());
        }
        let mut seen = HashSet::new();
        if let Some(dup) = hooks.value.members().iter().find(|m| !seen.insert(&m.key)) {
            return Err(format!("`hooks` has more than one `{}` key", dup.key));
        }
    }
    Ok(doc)
}

/// Check that `text` is a config we are willing to edit.
pub fn validate(text: &str) -> Result<()> {
    parse_doc(text).map(|_| ())
}

// ───────────────────────── finding our entries ─────────────────────────

/// One handler object that [`is_own`](collect) matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnHandler {
    pub event: String,
    pub command: String,
    pub timeout: Option<u32>,
    pub has_matcher: bool,
    /// How many handlers share the matcher group (1 = we own the whole group).
    pub group_size: usize,
    event_idx: usize,
    group_idx: usize,
    handler_idx: usize,
}

fn collect(text: &str, doc: &Doc, is_own: &dyn Fn(&str) -> bool) -> Vec<OwnHandler> {
    let mut out = Vec::new();
    let Some(hooks) = doc.hooks() else {
        return out;
    };
    for (event_idx, event) in hooks.value.members().iter().enumerate() {
        for (group_idx, group) in event.value.elements().iter().enumerate() {
            let Some(handlers) = group.get("hooks").map(|m| &m.value) else {
                continue;
            };
            for (handler_idx, handler) in handlers.elements().iter().enumerate() {
                let command = handler.get("command").and_then(|m| m.value.as_string(text));
                let is_command_type = handler
                    .get("type")
                    .and_then(|m| m.value.as_string(text))
                    .is_some_and(|t| t == "command");
                let Some(command) = command.filter(|c| is_command_type && is_own(c)) else {
                    continue;
                };
                out.push(OwnHandler {
                    event: event.key.clone(),
                    command,
                    timeout: handler
                        .get("timeout")
                        .and_then(|m| m.value.text(text).parse().ok()),
                    has_matcher: group.get("matcher").is_some(),
                    group_size: handlers.elements().len(),
                    event_idx,
                    group_idx,
                    handler_idx,
                });
            }
        }
    }
    out
}

/// Every handler in `text` that `is_own` recognises.
pub fn own_handlers(text: &str, is_own: &dyn Fn(&str) -> bool) -> Result<Vec<OwnHandler>> {
    let doc = parse_doc(text)?;
    Ok(collect(text, &doc, is_own))
}

// ───────────────────────── splicing primitives ─────────────────────────

fn splice(text: &str, start: usize, end: usize, with: &str) -> String {
    format!("{}{}{}", &text[..start], with, &text[end..])
}

/// Cut child `idx` out of `container` (leaving valid JSON and the neighbours'
/// own formatting alone). Removing the ONLY child leaves `{}` / `[]`.
fn remove_child(text: &str, container: &Node, idx: usize) -> String {
    let spans = container.child_spans();
    if spans.len() == 1 {
        let empty = if container.is_object() { "{}" } else { "[]" };
        return splice(text, container.start, container.end, empty);
    }
    if idx + 1 < spans.len() {
        // Delete from this child's start to the next child's start: takes the
        // comma and the whitespace between, so the next child slides into place.
        splice(text, spans[idx].0, spans[idx + 1].0, "")
    } else {
        // Last child: delete from the previous child's end (its comma goes too).
        splice(text, spans[idx - 1].1, spans[idx].1, "")
    }
}

/// Remove the first own handler that `keep` does not want, cascading upwards
/// through containers that become empty because of it. `None` = nothing to do.
fn remove_one(
    text: &str,
    is_own: &dyn Fn(&str) -> bool,
    keep: &dyn Fn(&OwnHandler) -> bool,
) -> Result<Option<String>> {
    let doc = parse_doc(text)?;
    let hits = collect(text, &doc, is_own);
    let mut seen_good_events = HashSet::new();
    let Some(hit) = hits.into_iter().find(|h| {
        // `keep` is asked per hit, but only the first kept hit per event counts:
        // a second identical entry is a duplicate and must go.
        !(keep(h) && seen_good_events.insert(h.event.clone()))
    }) else {
        return Ok(None);
    };
    let hooks = doc.hooks().ok_or("internal: hooks vanished")?;
    let event = &hooks.value.members()[hit.event_idx];
    let group = &event.value.elements()[hit.group_idx];
    let handlers = &group.get("hooks").ok_or("internal: group vanished")?.value;
    let out = if handlers.elements().len() > 1 {
        remove_child(text, handlers, hit.handler_idx)
    } else if event.value.elements().len() > 1 {
        // We owned the whole group; the event still has the user's other groups.
        remove_child(text, &event.value, hit.group_idx)
    } else if hooks.value.members().len() > 1 {
        remove_child(text, &hooks.value, hit.event_idx)
    } else {
        remove_child(text, &doc.root, root_index(&doc, "hooks"))
    };
    Ok(Some(out))
}

fn root_index(doc: &Doc, key: &str) -> usize {
    doc.root
        .members()
        .iter()
        .position(|m| m.key == key)
        .unwrap_or(0)
}

/// Remove every handler `is_own` recognises.
pub fn remove_own(text: &str, is_own: &dyn Fn(&str) -> bool) -> Result<String> {
    let mut cur = text.to_owned();
    // One removal per pass; every pass shrinks the file, the bound is a guard.
    for _ in 0..10_000 {
        match remove_one(&cur, is_own, &|_| false)? {
            Some(next) => cur = next,
            None => return Ok(cur),
        }
    }
    Err("too many entries to remove".into())
}

// ───────────────────────── rendering new JSON ─────────────────────────

/// How the file is laid out, so our block looks like it belongs.
struct Style {
    multiline: bool,
    /// One indentation step (as found at the first member of the root).
    unit: String,
    nl: &'static str,
}

impl Style {
    fn of(text: &str, doc: &Doc) -> Self {
        let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let (multiline, unit) = match doc.root.members().first() {
            Some(first) => {
                let gap = &text[doc.root.start + 1..first.start];
                match gap.rfind('\n') {
                    Some(i) if !gap[i + 1..].is_empty() => (true, gap[i + 1..].to_owned()),
                    Some(_) => (true, "  ".to_owned()),
                    None => (false, String::new()),
                }
            }
            None => (true, "  ".to_owned()),
        };
        Style {
            multiline,
            unit,
            nl,
        }
    }
}

enum J {
    Str(String),
    Num(u32),
    Obj(Vec<(String, J)>),
    Arr(Vec<J>),
}

fn json_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_owned())
}

impl J {
    fn render(&self, base: &str, st: &Style) -> String {
        match self {
            J::Str(s) => json_string(s),
            J::Num(n) => n.to_string(),
            J::Obj(members) if members.is_empty() => "{}".to_owned(),
            J::Arr(items) if items.is_empty() => "[]".to_owned(),
            J::Obj(members) => {
                let inner = format!("{base}{}", st.unit);
                let parts: Vec<String> = members
                    .iter()
                    .map(|(k, v)| {
                        let sep = if st.multiline { ": " } else { ":" };
                        format!("{}{sep}{}", json_string(k), v.render(&inner, st))
                    })
                    .collect();
                wrap('{', '}', &parts, base, &inner, st)
            }
            J::Arr(items) => {
                let inner = format!("{base}{}", st.unit);
                let parts: Vec<String> = items.iter().map(|v| v.render(&inner, st)).collect();
                wrap('[', ']', &parts, base, &inner, st)
            }
        }
    }
}

fn wrap(open: char, close: char, parts: &[String], base: &str, inner: &str, st: &Style) -> String {
    if st.multiline {
        let body = parts
            .iter()
            .map(|p| format!("{inner}{p}"))
            .collect::<Vec<_>>()
            .join(&format!(",{}", st.nl));
        format!("{open}{nl}{body}{nl}{base}{close}", nl = st.nl)
    } else {
        format!("{open}{}{close}", parts.join(","))
    }
}

/// The leading whitespace of the line `pos` is on.
fn line_indent(text: &str, pos: usize) -> &str {
    let line = text[..pos].rfind('\n').map_or(0, |i| i + 1);
    let rest = &text[line..];
    let len = rest
        .find(|c: char| c != ' ' && c != '\t')
        .unwrap_or(rest.len());
    &rest[..len]
}

/// Append a new child to `container`. `render` gets the indentation the child
/// will sit at and returns its text (`"key": value` for an object member).
fn append(text: &str, container: &Node, st: &Style, render: &dyn Fn(&str) -> String) -> String {
    let spans = container.child_spans();
    match spans.last() {
        Some(&(start, end)) => {
            // Reuse the whitespace that precedes the last sibling, so the new
            // one lines up with it (or follows it on the same line).
            let before = &text[..start];
            let sep = &before[before.trim_end_matches([' ', '\t', '\r', '\n']).len()..];
            let base = sep.rfind('\n').map_or("", |i| &sep[i + 1..]);
            splice(text, end, end, &format!(",{sep}{}", render(base)))
        }
        None => {
            let (open, close) = if container.is_object() {
                ('{', '}')
            } else {
                ('[', ']')
            };
            let base = line_indent(text, container.start);
            let new = if st.multiline {
                let child = format!("{base}{}", st.unit);
                format!(
                    "{open}{nl}{child}{}{nl}{base}{close}",
                    render(&child),
                    nl = st.nl
                )
            } else {
                format!("{open}{}{close}", render(""))
            };
            splice(text, container.start, container.end, &new)
        }
    }
}

// ───────────────────────── installing ─────────────────────────

/// The matcher group WHY adds to an event: no `matcher` (so it matches every
/// occurrence) and one command handler.
fn group(command: &str, timeout: u32) -> J {
    J::Obj(vec![(
        "hooks".into(),
        J::Arr(vec![J::Obj(vec![
            ("type".into(), J::Str("command".into())),
            ("command".into(), J::Str(command.into())),
            ("timeout".into(), J::Num(timeout)),
        ])]),
    )])
}

/// A brand-new config file containing only our hooks.
pub fn fresh(events: &[(&str, u32)], command: &str) -> String {
    let st = Style {
        multiline: true,
        unit: "  ".into(),
        nl: "\n",
    };
    let hooks = J::Obj(
        events
            .iter()
            .map(|(e, t)| ((*e).to_owned(), J::Arr(vec![group(command, *t)])))
            .collect(),
    );
    let doc = J::Obj(vec![("hooks".into(), hooks)]);
    format!("{}\n", doc.render("", &st))
}

fn add_group(text: &str, event: &str, command: &str, timeout: u32) -> Result<String> {
    let doc = parse_doc(text)?;
    let st = Style::of(text, &doc);
    let sep = if st.multiline { ": " } else { ":" };
    let style = &st;
    let member = |key: &str, value: J| {
        let key = key.to_owned();
        move |base: &str| format!("{}{sep}{}", json_string(&key), value.render(base, style))
    };
    match doc.hooks() {
        None => {
            let value = J::Obj(vec![(
                event.to_owned(),
                J::Arr(vec![group(command, timeout)]),
            )]);
            Ok(append(text, &doc.root, &st, &member("hooks", value)))
        }
        Some(hooks) => match hooks.value.get(event) {
            None => Ok(append(
                text,
                &hooks.value,
                &st,
                &member(event, J::Arr(vec![group(command, timeout)])),
            )),
            Some(m) if m.value.is_array() => {
                let g = group(command, timeout);
                Ok(append(text, &m.value, &st, &|base| g.render(base, &st)))
            }
            Some(_) => Err(format!("`hooks.{event}` is not an array")),
        },
    }
}

/// Make `text` contain exactly one correct WHY entry per event in `events`
/// (and none anywhere else), leaving every other byte alone. Entries that are
/// already correct are not touched, so installing twice changes nothing.
pub fn install(
    text: &str,
    events: &[(&str, u32)],
    command: &str,
    is_own: &dyn Fn(&str) -> bool,
) -> Result<String> {
    let wanted = |h: &OwnHandler| {
        h.group_size == 1
            && !h.has_matcher
            && h.command == command
            && events
                .iter()
                .any(|(e, t)| *e == h.event && h.timeout == Some(*t))
    };
    let mut cur = text.to_owned();
    // 1. Drop own entries that are stale, duplicated, in unwanted events, or
    //    share a group with a user's handler we would otherwise rewrite.
    for _ in 0..10_000 {
        match remove_one(&cur, is_own, &wanted)? {
            Some(next) => cur = next,
            None => break,
        }
    }
    // 2. Add what is missing.
    let present: HashSet<String> = own_handlers(&cur, is_own)?
        .into_iter()
        .map(|h| h.event)
        .collect();
    for (event, timeout) in events {
        if !present.contains(*event) {
            cur = add_group(&cur, event, command, *timeout)?;
        }
    }
    Ok(cur)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CMD: &str = "'/h/why-hook' --harness codex --instance 'installed'";
    const EVENTS: &[(&str, u32)] = &[("SessionStart", 5), ("Stop", 5)];

    fn own(c: &str) -> bool {
        c.starts_with("'/h/why-hook' ")
    }

    fn add(text: &str) -> String {
        install(text, EVENTS, CMD, &own).unwrap()
    }

    /// Whatever we produce must be valid JSON that still holds the user's value.
    fn json(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn a_new_file_has_every_event() {
        let out = fresh(EVENTS, CMD);
        let v = json(&out);
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], CMD);
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["timeout"], 5);
        assert!(out.ends_with("}\n"));
        assert_eq!(own_handlers(&out, &own).unwrap().len(), 2);
    }

    #[test]
    fn install_then_remove_is_the_identity_on_pretty_files() {
        let original = "{\n  \"theme\": \"dark\",\n  \"hooks\": {\n    \"Stop\": [\n      {\n        \"hooks\": [\n          {\"type\": \"command\", \"command\": \"echo mine\"}\n        ]\n      }\n    ]\n  }\n}\n";
        let with = add(original);
        let v = json(&with);
        // The user's hook is first and untouched, ours follows in its own group.
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], "echo mine");
        assert_eq!(v["hooks"]["Stop"][1]["hooks"][0]["command"], CMD);
        assert_eq!(v["hooks"]["SessionStart"][0]["hooks"][0]["command"], CMD);
        assert_eq!(v["theme"], "dark");
        assert_eq!(remove_own(&with, &own).unwrap(), original);
    }

    #[test]
    fn install_is_idempotent() {
        for original in [
            "{}",
            "{\n}\n",
            "{\"a\":1}",
            "{\n  \"a\": 1\n}\n",
            "{\"hooks\":{}}",
        ] {
            let once = add(original);
            assert_eq!(add(&once), once, "{original:?}");
            assert_eq!(own_handlers(&once, &own).unwrap().len(), 2);
        }
    }

    #[test]
    fn layout_trailing_newline_and_unicode_are_preserved() {
        let original = "{\t\"nom\": \"caf\\u00e9 ☕\",\t\"hooks\": {} }";
        let with = add(original);
        assert!(with.contains("caf\\u00e9 ☕"));
        assert!(!with.ends_with('\n'), "no newline was added");
        assert_eq!(json(&with)["nom"], "café ☕");
        // Tab indentation is reused for our block.
        let tabbed = "{\n\t\"a\": 1\n}\n";
        assert!(add(tabbed).contains("\n\t\"hooks\": {"));
        let crlf = "{\r\n  \"a\": 1\r\n}\r\n";
        let out = add(crlf);
        assert!(!out.contains("\n\n") && out.contains("\r\n  \"hooks\""));
    }

    #[test]
    fn same_event_other_matchers_and_handlers_stay_in_order() {
        let original = r#"{"hooks":{"Stop":[{"matcher":"x","hooks":[{"type":"command","command":"a"},{"type":"command","command":"b"}]},{"hooks":[{"type":"command","command":"c"}]}],"Other":[]}}"#;
        let v = json(&add(original));
        let stop = v["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 3);
        assert_eq!(stop[0]["hooks"][1]["command"], "b");
        assert_eq!(stop[1]["hooks"][0]["command"], "c");
        assert_eq!(stop[2]["hooks"][0]["command"], CMD);
        assert_eq!(v["hooks"]["Other"], serde_json::json!([]));
        assert_eq!(remove_own(&add(original), &own).unwrap(), original);
    }

    #[test]
    fn removal_covers_every_comma_position() {
        // Our entry first, in the middle, last, alone, and sharing a group.
        let ours = r#"{"type":"command","command":"'/h/why-hook' --x"}"#;
        let other = r#"{"type":"command","command":"keep"}"#;
        for (hooks, expect) in [
            (format!("{ours},{other}"), other.to_string()),
            (
                format!("{other},{ours},{other}"),
                format!("{other},{other}"),
            ),
            (format!("{other},{ours}"), other.to_string()),
        ] {
            let text = format!(r#"{{"hooks":{{"Stop":[{{"hooks":[{hooks}]}}]}}}}"#);
            let want = format!(r#"{{"hooks":{{"Stop":[{{"hooks":[{expect}]}}]}}}}"#);
            assert_eq!(remove_own(&text, &own).unwrap(), want);
        }
        // Alone: the whole hooks key goes, the rest of the file stays.
        let alone = format!(r#"{{"a":1,"hooks":{{"Stop":[{{"hooks":[{ours}]}}]}}}}"#);
        assert_eq!(remove_own(&alone, &own).unwrap(), r#"{"a":1}"#);
        let only = format!(r#"{{"hooks":{{"Stop":[{{"hooks":[{ours}]}}]}}}}"#);
        assert_eq!(remove_own(&only, &own).unwrap(), "{}");
    }

    #[test]
    fn an_outdated_entry_is_replaced_not_duplicated() {
        let stale = r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"'/h/why-hook' --old","timeout":600}]}]}}"#;
        let out = add(stale);
        let hits = own_handlers(&out, &own).unwrap();
        assert_eq!(hits.len(), 2);
        assert!(
            hits.iter()
                .all(|h| h.command == CMD && h.timeout == Some(5))
        );
    }

    #[test]
    fn our_handler_sharing_a_group_is_split_out_and_the_neighbour_kept() {
        let shared = r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"theirs"},{"type":"command","command":"'/h/why-hook' --old"}]}]}}"#;
        let v = json(&add(shared));
        assert_eq!(v["hooks"]["Stop"][0]["hooks"].as_array().unwrap().len(), 1);
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], "theirs");
        assert_eq!(v["hooks"]["Stop"][1]["hooks"][0]["command"], CMD);
    }

    #[test]
    fn refuses_what_it_must_not_repair() {
        for (text, why) in [
            ("", "not valid"),
            ("{", "not valid"),
            ("{\"a\":1,}", "not valid"),
            ("// c\n{}", "not valid"),
            ("[]", "not a JSON object"),
            ("\"hi\"", "not a JSON object"),
            ("null", "not a JSON object"),
            (r#"{"hooks":"x"}"#, "`hooks` is not an object"),
            (r#"{"hooks":[]}"#, "`hooks` is not an object"),
            (r#"{"hooks":{},"hooks":{}}"#, "more than one `hooks`"),
            (r#"{"hooks":{"Stop":[],"Stop":[]}}"#, "more than one `Stop`"),
        ] {
            let err = install(text, EVENTS, CMD, &own).unwrap_err();
            assert!(err.contains(why), "{text:?}: {err}");
            assert!(validate(text).is_err());
        }
        // An event that is not an array cannot receive our group.
        let err = install(r#"{"hooks":{"Stop":"x"}}"#, EVENTS, CMD, &own).unwrap_err();
        assert!(err.contains("not an array"), "{err}");
    }

    #[test]
    fn only_command_handlers_with_our_rule_count() {
        let text = r#"{"hooks":{"Stop":[{"hooks":[{"type":"http","command":"'/h/why-hook' x"},{"command":"'/h/why-hook' y"},{"type":"command","command":"'/h/why-hook' z"}]}]}}"#;
        let hits = own_handlers(text, &own).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].command, "'/h/why-hook' z");
    }

    #[test]
    fn compact_files_get_compact_blocks() {
        let out = add(r#"{"a":1}"#);
        assert!(!out.contains('\n'), "{out}");
        assert!(out.starts_with(r#"{"a":1,"hooks":{"#));
    }
}
