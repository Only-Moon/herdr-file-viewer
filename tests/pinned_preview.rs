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
use herdr_file_viewer::opener::{Opener, OpenerOutcome};
use herdr_file_viewer::presenter::{Focus, PaneGeometry};
use herdr_file_viewer::preview::PreviewPresentation;
use herdr_file_viewer::search::Match;
use herdr_file_viewer::view_policy::ViewMode;
use ratatui::layout::Rect;
use ratatui::text::Text;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{
    Arc, Mutex,
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

struct CountingEditor(Arc<AtomicUsize>);

impl EditorHandoff for CountingEditor {
    fn open(&mut self, _file: &Path) -> EditorOutcome {
        self.0.fetch_add(1, Ordering::SeqCst);
        EditorOutcome::NoTakeover
    }
}

struct CountingOpener(Arc<AtomicUsize>);

impl Opener for CountingOpener {
    fn open(&mut self, _path: &Path) -> OpenerOutcome {
        self.0.fetch_add(1, Ordering::SeqCst);
        OpenerOutcome::Launched
    }

    fn reveal(&mut self, _path: &Path) -> OpenerOutcome {
        self.0.fetch_add(1, Ordering::SeqCst);
        OpenerOutcome::Launched
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

/// Build a controller whose clipboard log remains observable after construction.
fn controller_with_recording_clipboard(root: &Path) -> (Controller, Arc<Mutex<Vec<String>>>) {
    let clipboard = common::RecordingClipboard::default();
    let copied = Arc::clone(&clipboard.copied);
    let components = Components {
        providers: Box::new(|_resolved| RootProviders {
            git: Arc::new(StubGit),
            content: Box::new(Lines),
        }),
        editor: Box::new(NoopEditor),
        clipboard: Box::new(clipboard),
        renderers: None,
    };
    (
        Controller::new(
            common::resolved(root.to_path_buf(), false),
            Baseline::Head,
            components,
        ),
        copied,
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

fn pin_ready_controller() -> (TempDir, Controller) {
    let dir = TempDir::new();
    std::fs::write(dir.path().join("preview.rs"), "placeholder\n").unwrap();
    let mut ctrl = controller(dir.path());
    await_content(&mut ctrl);
    ctrl.set_preview_viewports(herdr_file_viewer::presenter::PreviewViewports {
        active: (8, 4),
        pinned: None,
    });
    ctrl.handle(Intent::PinPreview);
    ctrl.set_preview_viewports(herdr_file_viewer::presenter::PreviewViewports {
        active: (8, 4),
        pinned: Some((8, 4)),
    });
    (dir, ctrl)
}

#[test]
fn pin_focus_cycles_and_removal_returns_focus_to_active() {
    let (_dir, mut ctrl) = pin_ready_controller();

    assert_eq!(ctrl.focus(), Focus::Tree);
    ctrl.handle(Intent::ToggleFocus);
    assert_eq!(ctrl.focus(), Focus::Pinned);
    ctrl.handle(Intent::ToggleFocus);
    assert_eq!(ctrl.focus(), Focus::Content);
    ctrl.handle(Intent::ToggleFocus);
    assert_eq!(ctrl.focus(), Focus::Tree);

    ctrl.handle(Intent::ToggleFocus);
    ctrl.handle(Intent::PinPreview);
    assert!(ctrl.view_state().pinned.is_none());
    assert_eq!(ctrl.focus(), Focus::Content);
}

#[test]
fn pinned_scroll_search_and_paging_do_not_touch_active_interaction() {
    let (_dir, mut ctrl) = pin_ready_controller();
    ctrl.handle(Intent::ToggleFocus);
    assert_eq!(ctrl.focus(), Focus::Pinned);

    ctrl.handle(Intent::NavDown);
    assert_eq!(ctrl.active_interaction().vertical_scroll, 0);
    assert_eq!(ctrl.view_state().pinned.as_ref().unwrap().scroll, 1);
    ctrl.handle(Intent::PageDown);
    assert_eq!(ctrl.view_state().pinned.as_ref().unwrap().scroll, 5);
    ctrl.handle(Intent::Expand);
    assert_eq!(ctrl.active_interaction().horizontal_scroll, 0);
    assert!(ctrl.view_state().pinned.as_ref().unwrap().hscroll > 0);

    ctrl.handle(Intent::OpenSearch);
    for ch in "needle".chars() {
        ctrl.handle_prompt_key(key(KeyCode::Char(ch)));
    }
    ctrl.handle_prompt_key(key(KeyCode::Enter));
    assert!(ctrl.active_interaction().search.is_none());
    assert_eq!(
        ctrl.view_state()
            .pinned
            .as_ref()
            .and_then(|p| p.search.as_ref())
            .map(|s| s.matches.len()),
        Some(20)
    );
}

#[test]
fn pinned_unavailable_actions_are_consumed_with_a_notice() {
    let dir = TempDir::new();
    std::fs::write(dir.path().join("preview.rs"), "placeholder\n").unwrap();
    let editor_calls = Arc::new(AtomicUsize::new(0));
    let opener_calls = Arc::new(AtomicUsize::new(0));
    let clipboard = common::RecordingClipboard::default();
    let copied = Arc::clone(&clipboard.copied);
    let editor_counter = Arc::clone(&editor_calls);
    let components = Components {
        providers: Box::new(|_resolved| RootProviders {
            git: Arc::new(StubGit),
            content: Box::new(Lines),
        }),
        editor: Box::new(CountingEditor(editor_counter)),
        clipboard: Box::new(clipboard),
        renderers: None,
    };
    let mut ctrl = Controller::new(
        common::resolved(dir.path().to_path_buf(), false),
        Baseline::Head,
        components,
    );
    await_content(&mut ctrl);
    ctrl.set_preview_viewports(herdr_file_viewer::presenter::PreviewViewports {
        active: (8, 4),
        pinned: None,
    });
    ctrl.handle(Intent::PinPreview);
    ctrl.set_preview_viewports(herdr_file_viewer::presenter::PreviewViewports {
        active: (8, 4),
        pinned: Some((8, 4)),
    });
    ctrl.set_opener(Box::new(CountingOpener(Arc::clone(&opener_calls))));
    ctrl.handle(Intent::ToggleFocus);
    let unavailable = [
        Intent::Activate,
        Intent::OpenFullscreen,
        Intent::OpenGoToLine,
        Intent::TreeScrollRight,
        Intent::OpenInEditor,
        Intent::OpenWithApp,
        Intent::RevealInFileManager,
        Intent::AddAnnotation,
        Intent::ShowAnnotations,
        Intent::CycleDiffRender,
        Intent::CycleView,
        Intent::ToggleWrap,
    ];
    for intent in unavailable {
        let before = ctrl.view_state();
        let fx = ctrl.handle(intent);
        assert!(fx.redraw, "{intent:?} reports rejection");
        assert_eq!(ctrl.focus(), Focus::Pinned, "{intent:?} keeps pinned focus");
        assert!(
            ctrl.action_notice().is_some(),
            "{intent:?} explains rejection"
        );
        assert_eq!(
            ctrl.view_state().pinned.as_ref().unwrap().scroll,
            before.pinned.as_ref().unwrap().scroll
        );
        assert_eq!(
            ctrl.active_interaction().vertical_scroll,
            before.content_scroll
        );
        assert_eq!(editor_calls.load(Ordering::SeqCst), 0, "{intent:?}");
        assert_eq!(opener_calls.load(Ordering::SeqCst), 0, "{intent:?}");
        assert!(copied.lock().unwrap().is_empty(), "{intent:?}");
    }
}

#[test]
fn pinned_y_copies_the_captured_root_relative_path_after_re_root() {
    let original_root = TempDir::new();
    let original_file = original_root.path().join("pinned.rs");
    std::fs::write(&original_file, "original\n").unwrap();
    let new_root = TempDir::new();
    std::fs::write(new_root.path().join("current.rs"), "current\n").unwrap();
    let (mut ctrl, copied) = controller_with_recording_clipboard(original_root.path());

    await_content(&mut ctrl);
    ctrl.handle(Intent::PinPreview);
    ctrl.re_root(new_root.path());
    await_content(&mut ctrl);
    std::fs::remove_dir_all(original_root.path()).unwrap();

    ctrl.handle(Intent::ToggleFocus);
    assert_eq!(ctrl.focus(), Focus::Pinned);
    ctrl.handle(Intent::CopyRepoPath);

    assert_eq!(
        copied.lock().unwrap().as_slice(),
        ["pinned.rs"],
        "pinned y uses its captured origin, not the current tree or filesystem"
    );
}

// Windows forbids control bytes in filesystem entry names. The controller's sanitizer is
// platform-independent, but this end-to-end proof needs a hostile path that can be pinned.
#[cfg(unix)]
#[test]
fn pinned_capital_y_copies_a_sanitized_captured_absolute_path_after_root_removal() {
    let original_root = TempDir::new();
    let hostile_name = "pin\u{1b}[2J\u{7}\n.rs";
    let original_file = original_root.path().join(hostile_name);
    std::fs::write(&original_file, "original\n").unwrap();
    let expected = original_file
        .to_string_lossy()
        .chars()
        .filter(|ch| !ch.is_control())
        .collect::<String>();
    let new_root = TempDir::new();
    std::fs::write(new_root.path().join("current.rs"), "current\n").unwrap();
    let (mut ctrl, copied) = controller_with_recording_clipboard(original_root.path());

    await_content(&mut ctrl);
    ctrl.handle(Intent::PinPreview);
    ctrl.re_root(new_root.path());
    await_content(&mut ctrl);
    std::fs::remove_dir_all(original_root.path()).unwrap();

    ctrl.handle(Intent::ToggleFocus);
    assert_eq!(ctrl.focus(), Focus::Pinned);
    ctrl.handle(Intent::CopyAbsPath);

    let copied = copied.lock().unwrap();
    assert_eq!(
        copied.as_slice(),
        [expected],
        "pinned Y uses the captured absolute origin without reading its removed root"
    );
    assert!(
        copied[0].chars().all(|ch| !ch.is_control()),
        "clipboard text never carries hostile terminal or paste control characters"
    );
}

#[test]
fn active_zoom_keeps_pin_while_pinned_fullscreen_is_rejected() {
    let (_dir, mut ctrl) = pin_ready_controller();
    ctrl.handle(Intent::ToggleFocus);
    assert_eq!(ctrl.focus(), Focus::Pinned);
    ctrl.handle(Intent::OpenFullscreen);
    assert!(!ctrl.zoomed());
    assert!(ctrl.view_state().pinned.is_some());
    assert!(ctrl.action_notice().is_some());

    ctrl.handle(Intent::ToggleFocus);
    assert_eq!(ctrl.focus(), Focus::Content);
    ctrl.handle(Intent::OpenFullscreen);
    assert!(ctrl.zoomed());
    assert!(ctrl.view_state().pinned.is_some());
}

#[test]
fn tree_hidden_focus_cycles_between_pinned_and_active() {
    let (_dir, mut ctrl) = pin_ready_controller();
    ctrl.handle(Intent::ToggleFocus);
    assert_eq!(ctrl.focus(), Focus::Pinned);
    ctrl.handle(Intent::ToggleZoom);
    assert!(ctrl.zoomed());
    assert_eq!(ctrl.focus(), Focus::Pinned);
    ctrl.handle(Intent::ToggleFocus);
    assert_eq!(ctrl.focus(), Focus::Content);
    ctrl.handle(Intent::ToggleFocus);
    assert_eq!(ctrl.focus(), Focus::Pinned);
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
fn pin_preview_intent_invokes_the_existing_snapshot_lifecycle() {
    // T-13 only wires the registry/dispatcher to T-6's already-tested lifecycle; it must not
    // create a second pin implementation.
    let dir = TempDir::new();
    std::fs::write(dir.path().join("preview.rs"), "placeholder\n").unwrap();
    let mut ctrl = controller(dir.path());
    await_content(&mut ctrl);

    assert!(ctrl.handle(Intent::PinPreview).redraw);
    assert!(
        ctrl.view_state().pinned.is_some(),
        "dispatch must create T-6's snapshot"
    );
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
