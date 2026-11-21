//! The `EventStore::{edit_calls, shell_turns}` contract, run against BOTH
//! backends with hand-built events (no git, no adapters). One set of cases, so
//! the in-memory fake and the SQLite index cannot drift.

use serde_json::json;
use why_lib::contracts::{EvidenceLabel, ToolOutcome, WorktreeId};
use why_lib::events::{
    EditTouch, EventKind, EventStore, MemoryEventStore, NormalizedEvent, SessionKey,
    SqliteEventStore, ToolCall, ToolClass,
};
use why_wire::{Envelope, Harness, PROTOCOL_VERSION};

/// Both backends, labelled (the SQLite one keeps its temp dir alive).
fn stores() -> Vec<(&'static str, Box<dyn EventStore>, Option<tempfile::TempDir>)> {
    let dir = tempfile::tempdir().unwrap();
    let sqlite = SqliteEventStore::open(&dir.path().join("events.db")).unwrap();
    vec![
        ("memory", Box::new(MemoryEventStore::new()), None),
        ("sqlite", Box::new(sqlite), Some(dir)),
    ]
}

/// One tool event, described by what matters; the rest has defaults.
struct Ev<'a> {
    session: &'a str,
    harness: Harness,
    turn: &'a str,
    id: Option<&'a str>,
    agent: Option<&'a str>,
    class: ToolClass,
    /// `None`: a start (`PreToolUse`).
    end: Option<ToolOutcome>,
    at: u64,
    wt: Option<&'a str>,
    files: &'a [&'a str],
    evidence: EvidenceLabel,
}

impl Default for Ev<'_> {
    fn default() -> Self {
        Ev {
            session: "s1",
            harness: Harness::ClaudeCode,
            turn: "p1",
            id: None,
            agent: None,
            class: ToolClass::Edit,
            end: None,
            at: 10,
            wt: Some("wt-1"),
            files: &[],
            evidence: EvidenceLabel::Observed,
        }
    }
}

impl Ev<'_> {
    fn build(&self) -> NormalizedEvent {
        let (hook, kind) = match self.end {
            None => ("PreToolUse", EventKind::ToolStarted),
            Some(outcome) => ("PostToolUse", EventKind::ToolFinished { outcome }),
        };
        // Everything that distinguishes two events goes into the payload, so
        // distinct events get distinct dedupe keys and an identical `Ev`
        // rebuilt later is a true redelivery.
        let env = Envelope {
            v: PROTOCOL_VERSION,
            harness: self.harness,
            harness_version: None,
            event: hook.into(),
            received_at_ms: self.at,
            cwd: "/x".into(),
            pid: 1,
            payload: json!({"session_id": self.session, "prompt_id": self.turn,
                "tool_use_id": self.id, "agent_id": self.agent, "wt": self.wt,
                "class": format!("{:?}", self.class), "files": self.files}),
        };
        let mut ev = NormalizedEvent::from_envelope(&env, 0, self.session, kind);
        ev.turn_id = Some(self.turn.to_owned());
        ev.agent_id = self.agent.map(str::to_owned);
        ev.worktree_id = self.wt.map(|w| WorktreeId(w.to_owned()));
        ev.evidence = self.evidence;
        ev.files = self.files.iter().map(|f| (*f).to_owned()).collect();
        ev.tool = Some(ToolCall {
            use_id: self.id.map(str::to_owned),
            name: "T".into(),
            class: self.class,
        });
        ev
    }
}

fn prompt(session: &str, turn: &str, at: u64) -> NormalizedEvent {
    let env = Envelope {
        v: PROTOCOL_VERSION,
        harness: Harness::ClaudeCode,
        harness_version: None,
        event: "UserPromptSubmit".into(),
        received_at_ms: at,
        cwd: "/x".into(),
        pid: 1,
        payload: json!({"session_id": session, "prompt_id": turn}),
    };
    let mut ev = NormalizedEvent::from_envelope(&env, 0, session, EventKind::Prompt);
    ev.turn_id = Some(turn.to_owned());
    ev.worktree_id = Some(WorktreeId("wt-1".into()));
    ev
}

fn session_end(session: &str, turn: &str, at: u64) -> NormalizedEvent {
    let mut ev = prompt(session, turn, at);
    ev.kind = EventKind::SessionEnd;
    ev.hook_event = "SessionEnd".into();
    // A different payload than the prompt's, so it has its own key.
    ev.key.0 = format!("end-{session}-{turn}-{at}");
    ev
}

fn wt(id: &str) -> WorktreeId {
    WorktreeId(id.into())
}

fn q(paths: &[&str]) -> Vec<String> {
    paths.iter().map(|p| (*p).to_owned()).collect()
}

fn ids(touches: &[EditTouch]) -> Vec<Option<&str>> {
    touches.iter().map(|t| t.tool_use_id.as_deref()).collect()
}

#[test]
fn a_call_is_one_touch_with_the_finish_outcome() {
    for (name, store, _dir) in stores() {
        let ok = |id| Ev {
            id: Some(id),
            files: &["a.rs"],
            ..Ev::default()
        };
        store
            .append(&[
                ok("t1").build(),
                Ev {
                    end: Some(ToolOutcome::Ok),
                    at: 11,
                    ..ok("t1")
                }
                .build(),
                Ev {
                    end: Some(ToolOutcome::Failed),
                    at: 21,
                    ..ok("t2")
                }
                .build(),
                Ev { at: 30, ..ok("t3") }.build(), // a start with no result
            ])
            .unwrap();
        let touches = store.edit_calls(&wt("wt-1"), &q(&["a.rs"])).unwrap();
        // Newest first: t3 (30), t2 (21), t1 (10).
        assert_eq!(
            ids(&touches),
            [Some("t3"), Some("t2"), Some("t1")],
            "{name}"
        );
        let outcomes: Vec<_> = touches.iter().map(|t| t.outcome).collect();
        assert_eq!(
            outcomes,
            [ToolOutcome::Unknown, ToolOutcome::Failed, ToolOutcome::Ok],
            "{name}"
        );
        assert_eq!(touches[2].at_ms, 10, "{name}: a call is dated by its start");
    }
}

/// The index may only hold one half of a call. Both halves must still be merged.
#[test]
fn a_call_is_completed_from_a_half_that_names_no_file() {
    for (name, store, _dir) in stores() {
        store
            .append(&[
                // Start names the file, finish names nothing.
                Ev {
                    id: Some("a"),
                    files: &["a.rs"],
                    ..Ev::default()
                }
                .build(),
                Ev {
                    id: Some("a"),
                    end: Some(ToolOutcome::Ok),
                    at: 11,
                    ..Ev::default()
                }
                .build(),
                // Start names nothing, finish names the file.
                Ev {
                    id: Some("b"),
                    at: 20,
                    ..Ev::default()
                }
                .build(),
                Ev {
                    id: Some("b"),
                    end: Some(ToolOutcome::Ok),
                    at: 21,
                    files: &["b.rs"],
                    ..Ev::default()
                }
                .build(),
            ])
            .unwrap();
        let a = store.edit_calls(&wt("wt-1"), &q(&["a.rs"])).unwrap();
        assert_eq!((a.len(), a[0].outcome), (1, ToolOutcome::Ok), "{name}");
        let b = store.edit_calls(&wt("wt-1"), &q(&["b.rs"])).unwrap();
        assert_eq!((b.len(), b[0].outcome), (1, ToolOutcome::Ok), "{name}");
    }
}

#[test]
fn successful_edit_targets_come_from_the_finish() {
    for (name, store, _dir) in stores() {
        let start = |id, path| Ev {
            id: Some(id),
            files: path,
            ..Ev::default()
        };
        store
            .append(&[
                start("inside", &["requested.rs"]).build(),
                Ev {
                    id: Some("inside"),
                    end: Some(ToolOutcome::Ok),
                    at: 11,
                    files: &["executed.rs"],
                    ..Ev::default()
                }
                .build(),
                start("outside", &["other-requested.rs"]).build(),
                Ev {
                    id: Some("outside"),
                    end: Some(ToolOutcome::Ok),
                    at: 12,
                    files: &["/outside/executed.rs"],
                    ..Ev::default()
                }
                .build(),
            ])
            .unwrap();
        assert!(
            store
                .edit_calls(&wt("wt-1"), &q(&["requested.rs", "other-requested.rs"]))
                .unwrap()
                .is_empty(),
            "{name}"
        );
        let touches = store.edit_calls(&wt("wt-1"), &q(&["executed.rs"])).unwrap();
        assert_eq!(ids(&touches), [Some("inside")], "{name}");
        assert_eq!(touches[0].outcome, ToolOutcome::Ok, "{name}");
    }
}

#[test]
fn the_same_relative_path_in_another_worktree_is_another_file() {
    for (name, store, _dir) in stores() {
        let mk = |session, w| Ev {
            session,
            wt: Some(w),
            id: Some("t"),
            end: Some(ToolOutcome::Ok),
            files: &["src/a.rs"],
            ..Ev::default()
        };
        store
            .append(&[mk("s-one", "wt-1").build(), mk("s-two", "wt-2").build()])
            .unwrap();
        let one = store.edit_calls(&wt("wt-1"), &q(&["src/a.rs"])).unwrap();
        let two = store.edit_calls(&wt("wt-2"), &q(&["src/a.rs"])).unwrap();
        assert_eq!(one.len(), 1, "{name}");
        assert_eq!(one[0].session.session_id, "s-one", "{name}");
        assert_eq!(two.len(), 1, "{name}");
        assert_eq!(two[0].session.session_id, "s-two", "{name}");
        assert!(
            store
                .edit_calls(&wt("wt-3"), &q(&["src/a.rs"]))
                .unwrap()
                .is_empty(),
            "{name}"
        );
    }
}

/// One session that moved between worktrees: each event counts where it resolved.
#[test]
fn a_session_that_changes_worktree_splits_its_edits_by_worktree() {
    for (name, store, _dir) in stores() {
        let mk = |id, w| Ev {
            id: Some(id),
            wt: Some(w),
            end: Some(ToolOutcome::Ok),
            files: &["a.rs"],
            ..Ev::default()
        };
        store
            .append(&[mk("t1", "wt-1").build(), mk("t2", "wt-2").build()])
            .unwrap();
        assert_eq!(
            ids(&store.edit_calls(&wt("wt-1"), &q(&["a.rs"])).unwrap()),
            [Some("t1")],
            "{name}"
        );
        assert_eq!(
            ids(&store.edit_calls(&wt("wt-2"), &q(&["a.rs"])).unwrap()),
            [Some("t2")],
            "{name}"
        );
    }
}

#[test]
fn stored_paths_are_canonical_and_never_matched_by_prefix() {
    for (name, store, _dir) in stores() {
        store
            .append(&[Ev {
                id: Some("t"),
                end: Some(ToolOutcome::Ok),
                // Raw spellings, as an adapter could leave them when a file was
                // already relative: dots, doubled slashes, spaces, unicode; plus
                // three that are not inside the worktree.
                files: &[
                    "./a/../b.rs",
                    "d//e/./f.rs",
                    "dir with space/né 世界.rs",
                    "/abs/outside.rs",
                    "../up.rs",
                    "",
                ],
                ..Ev::default()
            }
            .build()])
            .unwrap();
        for hit in ["b.rs", "d/e/f.rs", "dir with space/né 世界.rs"] {
            assert_eq!(
                store.edit_calls(&wt("wt-1"), &q(&[hit])).unwrap().len(),
                1,
                "{name}: {hit}"
            );
        }
        // Not canonical, absolute, a prefix, or a parent: all match nothing.
        for miss in [
            "./a/../b.rs",
            "/abs/outside.rs",
            "outside.rs",
            "up.rs",
            "../up.rs",
            "a",
            "d",
            "B.rs",
            "",
        ] {
            assert!(
                store
                    .edit_calls(&wt("wt-1"), &q(&[miss]))
                    .unwrap()
                    .is_empty(),
                "{name}: {miss:?}"
            );
        }
    }
}

#[test]
fn only_edit_class_events_inside_a_worktree_are_indexed() {
    for (name, store, _dir) in stores() {
        let file = ["a.rs"];
        store
            .append(&[
                Ev {
                    id: Some("no-wt"),
                    wt: None,
                    files: &file,
                    ..Ev::default()
                }
                .build(),
                Ev {
                    id: Some("shell"),
                    class: ToolClass::Shell,
                    files: &file,
                    ..Ev::default()
                }
                .build(),
                Ev {
                    id: Some("test"),
                    class: ToolClass::TestRun,
                    files: &file,
                    ..Ev::default()
                }
                .build(),
                Ev {
                    id: Some("other"),
                    class: ToolClass::Other,
                    files: &file,
                    ..Ev::default()
                }
                .build(),
                Ev {
                    id: Some("edit"),
                    files: &file,
                    ..Ev::default()
                }
                .build(),
            ])
            .unwrap();
        let touches = store.edit_calls(&wt("wt-1"), &q(&["a.rs"])).unwrap();
        assert_eq!(ids(&touches), [Some("edit")], "{name}");
    }
}

#[test]
fn a_redelivery_does_not_double_count() {
    for (name, store, _dir) in stores() {
        let batch = [
            Ev {
                id: Some("t"),
                files: &["a.rs"],
                ..Ev::default()
            }
            .build(),
            Ev {
                id: Some("t"),
                end: Some(ToolOutcome::Ok),
                at: 11,
                files: &["a.rs"],
                ..Ev::default()
            }
            .build(),
        ];
        store.append(&batch).unwrap();
        let again = store.append(&batch).unwrap();
        assert_eq!(again.inserted, 0, "{name}");
        let touches = store.edit_calls(&wt("wt-1"), &q(&["a.rs"])).unwrap();
        assert_eq!(touches.len(), 1, "{name}");
        assert_eq!(touches[0].outcome, ToolOutcome::Ok, "{name}");
    }
}

/// `tool_use_id` is only unique within a session, and a subagent shares its
/// parent's session: calls pair per (session, agent, id).
#[test]
fn calls_with_the_same_id_in_different_sessions_or_agents_stay_apart() {
    for (name, store, _dir) in stores() {
        let mk = |session, agent, end| Ev {
            session,
            agent,
            id: Some("same"),
            end,
            files: &["a.rs"],
            ..Ev::default()
        };
        store
            .append(&[
                mk("s1", None, None).build(),
                mk("s1", None, Some(ToolOutcome::Ok)).build(),
                mk("s2", None, Some(ToolOutcome::Failed)).build(),
                mk("s1", Some("agent-7"), None).build(),
            ])
            .unwrap();
        let touches = store.edit_calls(&wt("wt-1"), &q(&["a.rs"])).unwrap();
        let mut seen: Vec<_> = touches
            .iter()
            .map(|t| {
                (
                    t.session.session_id.as_str(),
                    t.agent_id.as_deref(),
                    t.outcome,
                )
            })
            .collect();
        seen.sort_by_key(|(s, a, _)| (*s, *a));
        assert_eq!(
            seen,
            [
                ("s1", None, ToolOutcome::Ok),
                ("s1", Some("agent-7"), ToolOutcome::Unknown),
                ("s2", None, ToolOutcome::Failed),
            ],
            "{name}"
        );
    }
}

#[test]
fn only_the_queried_paths_of_a_multi_file_call_are_reported() {
    for (name, store, _dir) in stores() {
        store
            .append(&[Ev {
                id: Some("patch"),
                end: Some(ToolOutcome::Ok),
                files: &["a.rs", "b.rs", "private/c.rs"],
                ..Ev::default()
            }
            .build()])
            .unwrap();
        let touches = store
            .edit_calls(&wt("wt-1"), &q(&["b.rs", "a.rs", "zzz.rs"]))
            .unwrap();
        assert_eq!(touches.len(), 1, "{name}");
        assert_eq!(touches[0].paths, ["a.rs", "b.rs"], "{name}");
    }
}

#[test]
fn a_query_of_thousands_of_paths_is_answered() {
    for (name, store, _dir) in stores() {
        store
            .append(&[Ev {
                id: Some("t"),
                end: Some(ToolOutcome::Ok),
                files: &["file-777.rs"],
                ..Ev::default()
            }
            .build()])
            .unwrap();
        let many: Vec<String> = (0..2_500).map(|i| format!("file-{i}.rs")).collect();
        assert_eq!(
            store.edit_calls(&wt("wt-1"), &many).unwrap().len(),
            1,
            "{name}"
        );
        assert!(
            store.edit_calls(&wt("wt-1"), &[]).unwrap().is_empty(),
            "{name}"
        );
    }
}

/// `turnIndex` is the turn's index in `get_session_timeline`, also when turns
/// arrive out of order, a turn has no id, and a lifecycle event names a turn.
#[test]
fn turn_indices_agree_with_the_timeline() {
    for (name, store, _dir) in stores() {
        let edit = |id, turn, at| Ev {
            id: Some(id),
            turn,
            at,
            end: Some(ToolOutcome::Ok),
            files: &["a.rs"],
            ..Ev::default()
        };
        store
            .append(&[
                prompt("s1", "p1", 100),
                edit("e1", "p1", 110).build(),
                // Arrives late but happened between p1 and p3.
                prompt("s1", "p2", 200),
                edit("e2", "p2", 210).build(),
                prompt("s1", "p3", 300),
                edit("e3", "p3", 310).build(),
                edit("e0", "", 50).build(), // before everything, no turn id
                session_end("s1", "p3", 999), // lifecycle: never a turn
            ])
            .unwrap();
        let timeline = store
            .timeline(&SessionKey::new(Harness::ClaudeCode, "s1"))
            .unwrap()
            .unwrap();
        let touches = store.edit_calls(&wt("wt-1"), &q(&["a.rs"])).unwrap();
        assert_eq!(touches.len(), 4, "{name}");
        for t in &touches {
            assert_eq!(
                timeline.turns[t.turn_index as usize].id,
                t.turn_id,
                "{name}: {t:?} vs {:?}",
                timeline.turns.iter().map(|x| &x.id).collect::<Vec<_>>()
            );
        }
        // The turn without an id was first in time, so it is turn 0.
        let e0 = touches
            .iter()
            .find(|t| t.tool_use_id.as_deref() == Some("e0"))
            .unwrap();
        assert_eq!((e0.turn_id.as_str(), e0.turn_index), ("", 0), "{name}");
    }
}

#[test]
fn paired_finishes_do_not_add_turns_or_reorder_them_after_clock_rollback() {
    for (name, store, _dir) in stores() {
        let edit = |id, turn, at| Ev {
            id: Some(id),
            turn,
            at,
            files: &["a.rs"],
            ..Ev::default()
        };
        let mut without_turn = Ev {
            end: Some(ToolOutcome::Ok),
            ..edit("second", "", 50)
        }
        .build();
        without_turn.turn_id = None;
        store
            .append(&[
                edit("first", "p1", 100).build(),
                edit("second", "p2", 200).build(),
                without_turn,
                Ev {
                    end: Some(ToolOutcome::Ok),
                    ..edit("second", "p2", 60)
                }
                .build(),
                edit("third", "p3", 300).build(),
            ])
            .unwrap();
        let timeline = store
            .timeline(&SessionKey::new(Harness::ClaudeCode, "s1"))
            .unwrap()
            .unwrap();
        assert_eq!(timeline.turns.len(), 3, "{name}");
        let touches = store.edit_calls(&wt("wt-1"), &q(&["a.rs"])).unwrap();
        assert_eq!(touches.len(), 3, "{name}");
        for touch in &touches {
            assert_eq!(
                timeline.turns.get(touch.turn_index as usize).map(|t| &t.id),
                Some(&touch.turn_id),
                "{name}: {touch:?}"
            );
        }
    }
}

#[test]
fn shell_turns_count_shell_and_test_runs_per_turn() {
    for (name, store, _dir) in stores() {
        let sh = |id, turn, class, end, at| Ev {
            id: Some(id),
            turn,
            class,
            end,
            at,
            ..Ev::default()
        };
        store
            .append(&[
                prompt("s1", "p1", 1),
                sh("a", "p1", ToolClass::Shell, None, 10).build(),
                sh("a", "p1", ToolClass::Shell, Some(ToolOutcome::Ok), 11).build(),
                sh("attempt", "p1", ToolClass::Shell, None, 9).build(),
                sh("b", "p1", ToolClass::TestRun, Some(ToolOutcome::Failed), 12).build(),
                prompt("s1", "p2", 20),
                sh("c", "p2", ToolClass::Shell, Some(ToolOutcome::Ok), 30).build(), // lost start
                // Not shell: an edit, and a shell command of another worktree.
                sh("e", "p2", ToolClass::Edit, None, 31).build(),
                Ev {
                    id: Some("x"),
                    wt: Some("wt-2"),
                    class: ToolClass::Shell,
                    ..Ev::default()
                }
                .build(),
            ])
            .unwrap();
        let turns = store.shell_turns(&wt("wt-1")).unwrap();
        let got: Vec<_> = turns
            .iter()
            .map(|t| {
                (
                    t.turn_id.as_str(),
                    t.turn_index,
                    t.command_count,
                    t.first_at_ms,
                )
            })
            .collect();
        assert_eq!(got, [("p2", 1, 1, 30), ("p1", 0, 2, 10)], "{name}");
        assert!(store.shell_turns(&wt("wt-2")).unwrap().is_empty(), "{name}");
        assert!(
            store.shell_turns(&wt("nowhere")).unwrap().is_empty(),
            "{name}"
        );
    }
}

/// Retention forgets whole sessions: the index must go with them, and a later
/// append of the same keys must work again.
#[test]
fn pruning_a_session_removes_its_index_rows() {
    for (name, store, _dir) in stores() {
        let old = Ev {
            id: Some("t"),
            end: Some(ToolOutcome::Ok),
            at: 5,
            files: &["a.rs"],
            ..Ev::default()
        }
        .build();
        let new = Ev {
            session: "s2",
            id: Some("t"),
            end: Some(ToolOutcome::Ok),
            at: 500,
            files: &["a.rs"],
            ..Ev::default()
        }
        .build();
        store.append(&[old.clone(), new]).unwrap();
        assert_eq!(
            store.edit_calls(&wt("wt-1"), &q(&["a.rs"])).unwrap().len(),
            2,
            "{name}"
        );

        assert_eq!(store.prune_older_than(100).unwrap(), 1, "{name}");
        let left = store.edit_calls(&wt("wt-1"), &q(&["a.rs"])).unwrap();
        assert_eq!(left.len(), 1, "{name}");
        assert_eq!(left[0].session.session_id, "s2", "{name}");

        // The pruned event can be delivered again (a spool replay) and indexes anew.
        assert_eq!(store.append(&[old]).unwrap().inserted, 1, "{name}");
        assert_eq!(
            store.edit_calls(&wt("wt-1"), &q(&["a.rs"])).unwrap().len(),
            2,
            "{name}"
        );
    }
}

/// "Observed only": a tool event that is not labelled `Observed` is never
/// counted, as an edit or as a shell command.
#[test]
fn only_observed_events_are_counted() {
    for (name, store, _dir) in stores() {
        let declared = EvidenceLabel::Declared;
        store
            .append(&[
                Ev {
                    id: Some("said"),
                    evidence: declared,
                    files: &["a.rs"],
                    ..Ev::default()
                }
                .build(),
                Ev {
                    id: Some("did"),
                    files: &["a.rs"],
                    ..Ev::default()
                }
                .build(),
                Ev {
                    id: Some("sh-said"),
                    class: ToolClass::Shell,
                    evidence: declared,
                    ..Ev::default()
                }
                .build(),
            ])
            .unwrap();
        let touches = store.edit_calls(&wt("wt-1"), &q(&["a.rs"])).unwrap();
        assert_eq!(ids(&touches), [Some("did")], "{name}");
        assert!(store.shell_turns(&wt("wt-1")).unwrap().is_empty(), "{name}");
    }
}

/// Two turns whose first events share a millisecond: the one whose first event
/// ARRIVED first is first, like the timeline's stable sort. (Taking the minimum
/// time and the minimum arrival independently would put the other one first.)
#[test]
fn equal_timestamps_order_turns_by_arrival_like_the_timeline() {
    for (name, store, _dir) in stores() {
        let edit = |id, turn, at| Ev {
            id: Some(id),
            turn,
            at,
            end: Some(ToolOutcome::Ok),
            files: &["a.rs"],
            ..Ev::default()
        };
        // Arrival order: X@200, Y@100, X@100. Turn Y's first event (100, #2)
        // beats turn X's first (100, #3), although X owns the earliest arrival (#1).
        store
            .append(&[
                edit("x-late", "X", 200).build(),
                edit("y", "Y", 100).build(),
                edit("x-early", "X", 100).build(),
            ])
            .unwrap();
        let timeline = store
            .timeline(&SessionKey::new(Harness::ClaudeCode, "s1"))
            .unwrap()
            .unwrap();
        assert_eq!(
            timeline
                .turns
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>(),
            ["Y", "X"],
            "{name}: premise"
        );
        for t in store.edit_calls(&wt("wt-1"), &q(&["a.rs"])).unwrap() {
            assert_eq!(
                timeline.turns[t.turn_index as usize].id, t.turn_id,
                "{name}: {t:?}"
            );
        }
    }
}
