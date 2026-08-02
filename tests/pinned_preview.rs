//! Pinned-preview interaction-state seams that are useful before pin lifecycle wiring lands.

mod common;

use common::TempDir;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use herdr_file_viewer::controller::{
    Components, ContentProvider, Controller, EditorHandoff, EditorOutcome, GitService,
    RenderResult, RootProviders,
};
use herdr_file_viewer::git::{Baseline, Status};
use herdr_file_viewer::infile::SearchState;
use herdr_file_viewer::intent::Intent;
use herdr_file_viewer::presenter::{Focus, PaneGeometry};
use herdr_file_viewer::preview::PreviewPresentation;
use herdr_file_viewer::search::Match;
use herdr_file_viewer::view_policy::ViewMode;
use ratatui::layout::Rect;
use ratatui::text::Text;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

#[derive(Default)]
struct StubGit;

impl GitService for StubGit {
    fn status(&self) -> BTreeMap<std::path::PathBuf, Status> {
        BTreeMap::new()
    }

    fn changed_set(&self, _baseline: Baseline) -> BTreeMap<std::path::PathBuf, Status> {
        BTreeMap::new()
    }

    fn diff(&self, _path: &Path, _baseline: Baseline, _full_context: bool) -> String {
        String::new()
    }

    fn diff_directory(&self, _path: &Path, _baseline: Baseline) -> String {
        String::new()
    }
}

#[derive(Clone)]
struct Lines;

impl ContentProvider for Lines {
    fn render(&self, _path: &Path, _mode: ViewMode, _raw_diff: Option<&str>) -> RenderResult {
        RenderResult {
            content: Text::raw(
                (1..=20)
                    .map(|n| format!("line {n} needle"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            notices: Vec::new(),
            source: None,
        }
    }
}

struct CountingLines {
    calls: Arc<AtomicUsize>,
}

impl ContentProvider for CountingLines {
    fn render(&self, _path: &Path, _mode: ViewMode, _raw_diff: Option<&str>) -> RenderResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Lines.render(_path, _mode, _raw_diff)
    }
}

struct DiskContent;

impl ContentProvider for DiskContent {
    fn render(&self, path: &Path, _mode: ViewMode, _raw_diff: Option<&str>) -> RenderResult {
        RenderResult {
            content: Text::raw(std::fs::read_to_string(path).unwrap()),
            notices: Vec::new(),
            source: None,
        }
    }
}

struct NoopEditor;

impl EditorHandoff for NoopEditor {
    fn open(&mut self, _file: &Path) -> EditorOutcome {
        EditorOutcome::NoTakeover
    }
}

fn controller(root: &Path) -> Controller {
    let components = Components {
        providers: Box::new(|_resolved| RootProviders {
            git: Arc::new(StubGit),
            content: Box::new(Lines),
        }),
        editor: Box::new(NoopEditor),
        clipboard: Box::new(common::RecordingClipboard::default()),
        renderers: None,
    };
    Controller::new(
        common::resolved(root.to_path_buf(), false),
        Baseline::Head,
        components,
    )
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn await_content(ctrl: &mut Controller) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while ctrl.content().lines.len() != 20 {
        ctrl.poll();
        assert!(Instant::now() < deadline, "preview content never rendered");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn content_text(ctrl: &Controller) -> String {
    ctrl.content()
        .lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn await_content_containing(ctrl: &mut Controller, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !content_text(ctrl).contains(expected) {
        ctrl.poll();
        assert!(
            Instant::now() < deadline,
            "preview content never rendered {expected:?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn assert_provider_call_count_stays(calls: &AtomicUsize, expected: usize) {
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        assert_eq!(
            calls.load(Ordering::SeqCst),
            expected,
            "content provider call count changed during the bounded quiet period"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(calls.load(Ordering::SeqCst), expected);
}

#[test]
fn active_interaction_can_be_copied_and_mutated_independently() {
    let dir = TempDir::new();
    std::fs::write(dir.path().join("preview.rs"), "placeholder\n").unwrap();
    let mut ctrl = controller(dir.path());

    let interaction = ctrl.active_interaction_mut();
    interaction.vertical_scroll = 7;
    interaction.horizontal_scroll = 3;
    interaction.search = Some(SearchState {
        query: "needle".into(),
        matches: vec![
            Match {
                line: 2,
                start: 7,
                end: 13,
            },
            Match {
                line: 4,
                start: 7,
                end: 13,
            },
        ],
        current: 1,
    });

    let mut copied = ctrl.active_interaction().clone();
    copied.vertical_scroll = 11;
    copied.horizontal_scroll = 9;
    copied.search.as_mut().unwrap().query = "changed".into();

    assert_eq!(ctrl.active_interaction().vertical_scroll, 7);
    assert_eq!(ctrl.active_interaction().horizontal_scroll, 3);
    assert_eq!(
        ctrl.active_interaction().search.as_ref().unwrap().query,
        "needle"
    );
    assert_eq!(
        ctrl.active_interaction().search.as_ref().unwrap().current,
        1
    );
    assert_ne!(copied, *ctrl.active_interaction());
}

#[test]
fn unpinned_navigation_projects_the_existing_active_interaction() {
    let dir = TempDir::new();
    std::fs::write(dir.path().join("preview.rs"), "placeholder\n").unwrap();
    let mut ctrl = controller(dir.path());
    await_content(&mut ctrl);
    ctrl.set_content_viewport(40, 5);
    ctrl.set_pane_geometry(PaneGeometry {
        content_inner: Some(Rect::new(0, 0, 40, 5)),
        ..Default::default()
    });

    ctrl.handle(Intent::ToggleFocus);
    assert_eq!(ctrl.focus(), Focus::Content);
    ctrl.handle(Intent::NavDown);
    ctrl.handle(Intent::OpenSearch);
    for ch in "needle".chars() {
        ctrl.handle_prompt_key(key(KeyCode::Char(ch)));
    }
    ctrl.handle_prompt_key(key(KeyCode::Enter));
    ctrl.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 1,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    ctrl.handle_mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 6,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    ctrl.handle_mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: 6,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });

    assert_eq!(ctrl.active_interaction().vertical_scroll, 0);
    assert_eq!(ctrl.view_state().content_scroll, 0);
    assert_eq!(ctrl.active_interaction().horizontal_scroll, 0);
    assert_eq!(
        ctrl.search().map(|search| search.query.as_str()),
        Some("needle")
    );
    assert_eq!(
        ctrl.view_state()
            .search
            .as_ref()
            .map(|search| search.matches.len()),
        Some(20)
    );
    assert!(ctrl.active_interaction().selection.is_some());
    assert!(ctrl.view_state().content_selection.is_some());
}

#[test]
fn layout_only_wrap_toggle_keeps_the_active_document_presentation_in_sync() {
    let dir = TempDir::new();
    std::fs::write(dir.path().join("preview.rs"), "placeholder\n").unwrap();
    let mut ctrl = controller(dir.path());
    await_content(&mut ctrl);

    ctrl.handle(Intent::ToggleWrap);

    let projected = ctrl.view_state();
    let settled = *ctrl
        .active_document()
        .expect("the syntax preview remains a settled active document")
        .presentation();
    assert_eq!(
        settled,
        PreviewPresentation::new(
            settled.view_mode(),
            projected.wrap,
            projected.content_pad_left,
        )
    );
}

#[test]
fn no_pin_navigation_does_not_add_content_provider_work() {
    let dir = TempDir::new();
    std::fs::write(dir.path().join("preview.rs"), "placeholder\n").unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let provider_calls = Arc::clone(&calls);
    let components = Components {
        providers: Box::new(move |_resolved| RootProviders {
            git: Arc::new(StubGit),
            content: Box::new(CountingLines {
                calls: Arc::clone(&provider_calls),
            }),
        }),
        editor: Box::new(NoopEditor),
        clipboard: Box::new(common::RecordingClipboard::default()),
        renderers: None,
    };
    let mut ctrl = Controller::new(
        common::resolved(dir.path().to_path_buf(), false),
        Baseline::Head,
        components,
    );
    await_content(&mut ctrl);
    assert_eq!(calls.load(Ordering::SeqCst), 1, "one initial file render");

    ctrl.set_content_viewport(40, 5);
    ctrl.set_pane_geometry(PaneGeometry {
        content_inner: Some(Rect::new(0, 0, 40, 5)),
        ..Default::default()
    });
    ctrl.handle(Intent::ToggleFocus);
    ctrl.handle(Intent::NavDown);
    ctrl.handle(Intent::OpenSearch);
    for ch in "needle".chars() {
        ctrl.handle_prompt_key(key(KeyCode::Char(ch)));
    }
    ctrl.handle_prompt_key(key(KeyCode::Enter));
    ctrl.handle(Intent::NextMatch);
    ctrl.handle(Intent::PrevMatch);

    assert_provider_call_count_stays(&calls, 1);
}

#[test]
fn loading_directory_and_empty_tree_are_not_active_documents() {
    let loading_root = TempDir::new();
    std::fs::write(loading_root.path().join("preview.rs"), "placeholder\n").unwrap();
    let loading = controller(loading_root.path());
    assert!(
        loading.active_document().is_none(),
        "loading is not pinnable"
    );

    let directory_root = TempDir::new();
    std::fs::create_dir(directory_root.path().join("child")).unwrap();
    let directory = controller(directory_root.path());
    assert!(
        directory.active_document().is_none(),
        "directory guidance is not pinnable"
    );

    let empty_root = TempDir::new();
    let empty = controller(empty_root.path());
    assert!(
        empty.active_document().is_none(),
        "empty-tree guidance is not pinnable"
    );
}

#[test]
fn pin_lifecycle_clones_the_settled_preview_and_toggles_the_same_identity() {
    let dir = TempDir::new();
    std::fs::write(dir.path().join("preview.rs"), "placeholder\n").unwrap();
    let mut ctrl = controller(dir.path());
    await_content(&mut ctrl);
    ctrl.set_content_viewport(40, 5);
    let interaction = ctrl.active_interaction_mut();
    interaction.vertical_scroll = 7;
    interaction.horizontal_scroll = 3;
    interaction.search = Some(SearchState {
        query: "needle".into(),
        matches: vec![Match {
            line: 4,
            start: 7,
            end: 13,
        }],
        current: 0,
    });

    assert!(ctrl.pin_active_preview().redraw);
    let pin = ctrl.view_state().pinned.expect("settled file is pinned");
    assert_eq!(pin.content, *ctrl.content());
    assert_eq!(pin.scroll, 7);
    assert_eq!(pin.hscroll, 3);
    assert_eq!(pin.search.expect("copied search").matches.len(), 1);
    assert_eq!(ctrl.view_state().preview_split_pct, 50);

    assert!(ctrl.pin_active_preview().redraw);
    assert!(
        ctrl.view_state().pinned.is_none(),
        "same identity removes the pin"
    );

    assert!(ctrl.pin_active_preview().redraw);
    assert!(ctrl.view_state().pinned.is_some());
    assert_eq!(ctrl.view_state().preview_split_pct, 50);
}

#[test]
fn pinning_a_different_file_replaces_one_frozen_snapshot_without_rendering() {
    let dir = TempDir::new();
    std::fs::write(dir.path().join("a.rs"), "a\n").unwrap();
    std::fs::write(dir.path().join("b.rs"), "b\n").unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let provider_calls = Arc::clone(&calls);
    let components = Components {
        providers: Box::new(move |_resolved| RootProviders {
            git: Arc::new(StubGit),
            content: Box::new(CountingLines {
                calls: Arc::clone(&provider_calls),
            }),
        }),
        editor: Box::new(NoopEditor),
        clipboard: Box::new(common::RecordingClipboard::default()),
        renderers: None,
    };
    let mut ctrl = Controller::new(
        common::resolved(dir.path().to_path_buf(), false),
        Baseline::Head,
        components,
    );
    await_content(&mut ctrl);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    ctrl.pin_active_preview();
    let first = ctrl
        .view_state()
        .pinned
        .expect("first file was captured")
        .origin
        .expect("pins retain origins");

    ctrl.handle(Intent::NavDown);
    await_content(&mut ctrl);
    let frozen = ctrl.view_state().pinned.expect("navigation keeps the pin");
    assert_eq!(frozen.origin.as_ref(), Some(&first));

    let calls_before_pin = calls.load(Ordering::SeqCst);
    ctrl.pin_active_preview();
    assert_provider_call_count_stays(&calls, calls_before_pin);
    let replacement = ctrl.view_state().pinned.expect("different file replaces");
    assert_ne!(replacement.origin.as_ref(), Some(&first));
    assert_eq!(replacement.content.lines.len(), 20);
}

#[test]
fn rejected_pin_attempts_leave_no_file_and_directory_targets_explained() {
    let empty_root = TempDir::new();
    let mut empty = controller(empty_root.path());
    assert!(empty.pin_active_preview().redraw);
    assert!(empty.view_state().pinned.is_none());
    assert_eq!(
        empty.action_notice(),
        Some("Cannot pin: no file is selected")
    );

    let directory_root = TempDir::new();
    std::fs::create_dir(directory_root.path().join("child")).unwrap();
    let mut directory = controller(directory_root.path());
    assert!(directory.pin_active_preview().redraw);
    assert!(directory.view_state().pinned.is_none());
    assert_eq!(directory.action_notice(), Some("Cannot pin a directory"));
}

#[test]
fn active_refresh_and_width_reflow_do_not_change_a_pinned_snapshot() {
    let dir = TempDir::new();
    let path = dir.path().join("preview.md");
    std::fs::write(&path, "before reflow\n").unwrap();
    let components = Components {
        providers: Box::new(|_resolved| RootProviders {
            git: Arc::new(StubGit),
            content: Box::new(DiskContent),
        }),
        editor: Box::new(NoopEditor),
        clipboard: Box::new(common::RecordingClipboard::default()),
        renderers: None,
    };
    let mut ctrl = Controller::new(
        common::resolved(dir.path().to_path_buf(), false),
        Baseline::Head,
        components,
    );
    await_content_containing(&mut ctrl, "before reflow");
    ctrl.pin_active_preview();
    let before = ctrl.view_state().pinned.expect("pin before active changes");

    std::fs::write(&path, "after reflow\n").unwrap();
    ctrl.set_content_viewport(40, 5);
    await_content_containing(&mut ctrl, "after reflow");
    std::fs::write(&path, "after refresh\n").unwrap();
    ctrl.handle(Intent::Refresh);
    await_content_containing(&mut ctrl, "after refresh");
    let after = ctrl
        .view_state()
        .pinned
        .expect("pin survives active refresh");
    assert_eq!(after.content, before.content);
    assert_eq!(after.notices, before.notices);
    assert_eq!(after.origin, before.origin);
    assert_eq!(after.scroll, before.scroll);
    assert_eq!(after.hscroll, before.hscroll);
}
