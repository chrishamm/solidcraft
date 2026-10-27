//! Edit in place: double-clicking a linked component opens the design it comes from, with the
//! rest of the assembly drawn faded around it where the component sits, so its own timeline,
//! browser and commands are at hand. Finish Editing saves the part and goes back to the assembly,
//! which reads the link again.
//!
//! The part is an ordinary tab: saving, autosave, recovery, closing and quitting treat it as any
//! other design. The assembly around it is a picture of the assembly's tab, taken when the edit
//! starts and again after Update Links.

use std::sync::Arc;
use std::sync::mpsc::Receiver;

use serde_json::{Value, json};
use solidcraft_engine::Session;
use solidcraft_engine::doc::{apply_point, apply_vector, mat_inverse, mat_mul};
use solidcraft_engine::geom::Aabb3;

use crate::SolveApp;
use crate::gpu::GpuScene;
use crate::theme::Tokens;

/// The assembly around a part being edited in place.
#[derive(Clone)]
pub struct PartContext {
    /// The assembly's file and name.
    pub assembly: String,
    pub assembly_name: String,
    /// The part's file.
    pub part: String,
    /// The linked component edited, and the placement the assembly is drawn around.
    pub component: u64,
    pub placement: Vec<u64>,
    /// How many of the assembly's bodies are drawn around the part.
    pub bodies: usize,
    /// Their triangles in the part's frame, three points and normals each, worked out once.
    pub triangles: Arc<Vec<([f32; 3], [f32; 3])>>,
    /// Their extent in the part's frame, so fitting the view frames the assembly too.
    pub bounds: Aabb3,
    /// The part's tab was opened for this edit, so Finish Editing closes it again.
    pub opened: bool,
    /// The number of tabs when the edit started (more once the part has opened in a new one).
    tabs: usize,
}

/// Update Links run on the assembly while its part is edited in place.
struct Refresh {
    /// The assembly's file, and its document when the run started: a result for an assembly
    /// changed since is dropped (showing it changes nothing).
    assembly: String,
    base: Arc<solidcraft_engine::doc::Document>,
    done: Receiver<Result<Session, String>>,
}

/// The edit in place of the tab shown, and one waiting for its part to open.
#[derive(Default)]
pub struct EditInPlace {
    pub active: Option<PartContext>,
    pending: Option<PartContext>,
    /// Finish Editing is waiting for the save of the part at this file.
    finish_after_save: Option<String>,
    refresh: Option<Refresh>,
}

impl EditInPlace {
    /// Update Links is running on an assembly whose part is edited in place.
    pub fn refreshing(&self) -> bool {
        self.refresh.is_some()
    }
}

/// A component double-clicked in the browser: a linked one is edited in place (Preferences:
/// `double_click_components`), any other is made the active component (`component.activate`
/// with `params`). Activate itself always does the latter.
pub(crate) fn double_click_component(app: &mut SolveApp, params: Value) {
    let component = params["component"].as_u64().unwrap_or(0);
    if app.preferences.double_click_components && app.session.doc.links.contains_key(&component) {
        let path = params["path"].as_array().map(|p| p.iter().filter_map(Value::as_u64).collect::<Vec<_>>());
        enter(app, component, path);
    } else {
        let _ = app.run("component.activate", params);
    }
}

/// A face of the body `body` double-clicked in the view: the component the body belongs to, at
/// that body's placement, is edited in place when it is linked and activated otherwise. A body of
/// the design itself or of the active component is left alone, as are features. Whether it acted.
pub(crate) fn double_click_face(app: &mut SolveApp, body: &str) -> bool {
    if !app.preferences.double_click_components {
        return false;
    }
    let Some(component) = app.session.world_state().body(body).map(|b| b.component).filter(|c| *c != 0) else { return false };
    let placement = app
        .session
        .doc
        .placements()
        .into_iter()
        .filter(|(c, _, _)| *c == component)
        .map(|(_, path, _)| path)
        .find(|path| app.session.occurrence_bodies(path, true).is_ok_and(|bodies| bodies.iter().any(|b| b == body)));
    if app.session.doc.links.contains_key(&component) {
        enter(app, component, placement);
        return true;
    }
    let active = app.session.active_component == component
        && (placement.is_none() || app.session.active_occurrence_path.is_none() || app.session.active_occurrence_path == placement);
    if active {
        return false;
    }
    let occurrence = placement.as_ref().and_then(|p| p.last().copied());
    app.run("component.activate", json!({"component": component, "occurrence": occurrence, "path": placement})).is_ok()
}

/// Something runs that a change of the design shown would spoil.
fn busy(app: &SolveApp) -> bool {
    app.file_picker_busy() || app.file_load.busy() || app.saving.is_some() || app.edit_in_place.refreshing()
}

/// The bodies of `session` around its linked component `component`, in that component's frame
/// at `placement`: how many, their triangles and their extent. `None` when the placement is gone.
fn snapshot(session: &Session, component: u64, placement: &[u64]) -> Option<(usize, Vec<([f32; 3], [f32; 3])>, Aabb3)> {
    let frame = session.occurrence_placement(placement).ok().and_then(|p| mat_inverse(&p.world))?;
    let world = session.world_state();
    let (mut bodies, mut triangles, mut bounds) = (0, Vec::new(), Aabb3::default());
    for b in world.bodies.iter().filter(|b| b.component != component && session.body_visible(&b.name)) {
        let (mesh, placed) = b.mesh_in_place();
        let m = mat_mul(&frame, &placed);
        bodies += 1;
        for t in &mesh.triangles {
            // A triangle with an index out of range is left out whole, so the rest stay in step.
            let corners: Option<Vec<_>> = t.iter().map(|k| Some((*mesh.positions.get(*k as usize)?, *mesh.normals.get(*k as usize)?))).collect();
            for (p, n) in corners.into_iter().flatten() {
                let p = apply_point(&m, p);
                bounds.add(p);
                triangles.push((p.to_f32(), apply_vector(&m, n).to_f32()));
            }
        }
    }
    Some((bodies, triangles, bounds))
}

/// Open the design the linked component `component` comes from, with the assembly around it as
/// its placement `path` (default: its first) puts it.
pub(crate) fn enter(app: &mut SolveApp, component: u64, path: Option<Vec<u64>>) {
    if busy(app) {
        return;
    }
    let Some(assembly) = app.session.path.clone() else {
        app.set_status(crate::i18n::tr("ui.edit_in_place_save_first").to_string(), true);
        return;
    };
    let links = match app.session.execute("component.links", &json!({})) {
        Ok(links) => links,
        Err(e) => {
            app.set_status(e.to_string(), true);
            return;
        }
    };
    let Some(link) = links["links"].as_array().into_iter().flatten().find(|l| l["component"].as_u64() == Some(component)).cloned() else {
        app.set_status(app.tr_label("ui.relink_not_linked").to_string(), true);
        return;
    };
    let Some(part) = link["file"].as_str().map(str::to_string) else { return };
    let placement = path.unwrap_or_else(|| app.session.doc.first_path(component));
    let Some((bodies, triangles, bounds)) = snapshot(&app.session, component, &placement) else { return };
    app.edit_in_place.pending = Some(PartContext {
        assembly,
        assembly_name: app.session.doc.name.clone(),
        part,
        component,
        placement,
        bodies,
        triangles: Arc::new(triangles),
        bounds,
        opened: false,
        tabs: app.docs.count(),
    });
    crate::network_prompt::open_linked(app, &link);
}

/// The design shown has become the part an edit waits for: the edit starts there. An edit whose
/// part did not open (the open failed or was cancelled) is dropped by [`tick`].
pub(crate) fn attach(app: &mut SolveApp) {
    let shown = app.session.path.as_deref();
    let Some(mut context) = app.edit_in_place.pending.take_if(|c| shown.is_some_and(|p| crate::documents::same_path(p, &c.part))) else {
        return;
    };
    context.opened = app.docs.count() > context.tabs;
    app.edit_in_place.active = Some(context);
    app.fit_view();
}

/// Update Links pressed while a part is edited in place: the assembly's tab reads its links again
/// (off the window thread), and the picture around the part follows.
pub(crate) fn refresh(app: &mut SolveApp) {
    if app.edit_in_place.refreshing() {
        return;
    }
    let Some(assembly) = app.edit_in_place.active.as_ref().map(|c| c.assembly.clone()) else { return };
    let Some(session) = crate::documents::session_at(app, &assembly) else {
        let name = app.edit_in_place.active.as_ref().map(|c| c.assembly_name.clone()).unwrap_or_default();
        app.set_status(crate::i18n::format("ui.edit_in_place_assembly_closed", &[("name", &name)]), true);
        return;
    };
    let mut scratch = session.scratch();
    // Link paths are kept relative to the design's own file.
    scratch.path.clone_from(&session.path);
    let base = session.doc.clone();
    let (tx, done) = std::sync::mpsc::channel();
    let run = move || {
        let r = scratch.execute("component.update_links", &json!({})).map(|_| scratch).map_err(|e| e.to_string());
        let _ = tx.send(r);
    };
    #[cfg(not(target_arch = "wasm32"))]
    if std::thread::Builder::new().name("edit-in-place-links".into()).spawn(run).is_err() {
        app.set_status(crate::i18n::tr("ui.edit_in_place_refresh_stopped").to_string(), true);
        return;
    }
    #[cfg(target_arch = "wasm32")]
    run();
    app.edit_in_place.refresh = Some(Refresh { assembly, base, done });
    let name = app.edit_in_place.active.as_ref().map(|c| c.assembly_name.clone()).unwrap_or_default();
    app.set_status(crate::i18n::format("ui.edit_in_place_refreshing", &[("name", &name)]), false);
}

/// Take up a finished refresh: the assembly's tab adopts it when it is still the assembly the
/// run started from, and the picture around the part is taken again.
fn take_refresh(app: &mut SolveApp) {
    let Some(r) = &app.edit_in_place.refresh else { return };
    let result = match r.done.try_recv() {
        Ok(result) => result,
        Err(std::sync::mpsc::TryRecvError::Empty) => return,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(crate::i18n::tr("ui.edit_in_place_refresh_stopped").to_string()),
    };
    let Some(r) = app.edit_in_place.refresh.take() else { return };
    let computed = match result {
        Ok(computed) => computed,
        Err(e) => {
            app.set_status(e, true);
            return;
        }
    };
    let Some(session) = crate::documents::session_at_mut(app, &r.assembly) else { return };
    if !Arc::ptr_eq(&session.doc, &r.base) {
        app.set_status(crate::i18n::tr("ui.edit_in_place_refresh_dropped").to_string(), true);
        return;
    }
    session.accept_computed(computed);
    // Every link was read: coming back to the assembly need not read them again.
    crate::documents::read_again_at(app, &r.assembly);
    let Some(context) = app.edit_in_place.active.as_ref().filter(|c| crate::documents::same_path(&c.assembly, &r.assembly)) else { return };
    let (component, placement) = (context.component, context.placement.clone());
    let snap = crate::documents::session_at(app, &r.assembly).and_then(|s| snapshot(s, component, &placement));
    let Some((bodies, triangles, bounds)) = snap else { return };
    if let Some(context) = app.edit_in_place.active.as_mut() {
        (context.bodies, context.triangles, context.bounds) = (bodies, Arc::new(triangles), bounds);
    }
    let language = crate::i18n::Locale::for_language(&app.preferences.language);
    app.set_status(language.format("ui.command_done", &[("command", language.label(crate::command_label("component.update_links")))]), false);
}

/// Finish Editing: save the part (and wait for it), go back to the assembly, close the part's
/// tab when the edit opened it and read the assembly's links again.
pub(crate) fn finish(app: &mut SolveApp) {
    let Some(part) = app.edit_in_place.active.as_ref().map(|c| c.part.clone()) else { return };
    if busy(app) {
        return;
    }
    // Saved to another file (Save As, a copy of a read-only design): the assembly still links this one.
    if !app.session.path.as_deref().is_some_and(|p| crate::documents::same_path(p, &part)) {
        let name = std::path::Path::new(&part).file_stem().map_or_else(|| part.clone(), |n| n.to_string_lossy().into_owned());
        app.set_status(crate::i18n::format("ui.edit_in_place_saved_elsewhere", &[("name", &name)]), true);
        return;
    }
    if app.session.is_dirty() {
        app.edit_in_place.finish_after_save = Some(part);
        crate::toolbar::save(app);
        return;
    }
    let Some(context) = app.edit_in_place.active.take() else { return };
    let part_tab = app.docs.active;
    match crate::documents::reuse_path(app, &context.assembly) {
        Ok(true) => {}
        _ => {
            app.set_status(crate::i18n::format("ui.edit_in_place_assembly_closed", &[("name", &context.assembly_name)]), true);
            app.edit_in_place.active = Some(context);
            return;
        }
    }
    if context.opened {
        crate::documents::close(app, part_tab, false);
    }
    // Coming back to its tab already updates what changed (`update_changed_links`).
    if !app.preferences.update_changed_links {
        crate::network_prompt::update_links(app);
    }
}

/// Each frame: a refresh that finished is taken up; an edit waiting for a part that nothing will
/// open any more is dropped; and Finish Editing goes on once the part's save is done, in the
/// part's own tab (a save that failed, was cancelled or went elsewhere leaves the edit as it was).
pub(crate) fn tick(app: &mut SolveApp) {
    take_refresh(app);
    if app.edit_in_place.pending.is_some() && !app.file_load.busy() && app.network_prompt.is_none() && app.lock_prompts.is_empty() {
        app.edit_in_place.pending = None;
    }
    let waiting = app.saving.is_some() || app.file_picker.is_some() || app.disk_prompt;
    if !waiting && let Some(part) = app.edit_in_place.finish_after_save.take() {
        let shown = app.session.path.as_deref().is_some_and(|p| crate::documents::same_path(p, &part));
        if shown && !app.session.is_dirty() {
            finish(app);
        }
    }
}

/// The mouse's back button (or a keyboard's Back key) pressed: Finish Editing, unless a dialog, a
/// tool or a sketch is open in the part.
pub(crate) fn back(app: &mut SolveApp) {
    if app.edit_in_place.active.is_some() && app.dialog.is_none() && app.tool.is_none() && app.session.active_sketch.is_none() {
        finish(app);
    }
}

/// What the scene key needs of the edit: the assembly picture shown (0: none).
pub(crate) fn scene_key(app: &SolveApp) -> usize {
    app.edit_in_place.active.as_ref().map_or(0, |c| Arc::as_ptr(&c.triangles) as usize)
}

/// The extent of the assembly around the part, when one is shown.
pub(crate) fn bounds(app: &SolveApp) -> Option<Aabb3> {
    app.edit_in_place.active.as_ref().map(|c| c.bounds).filter(|b| !b.is_empty())
}

/// Draw the assembly around the part, faded and out of reach of picks.
pub(crate) fn draw_context(app: &SolveApp, sc: &mut GpuScene) {
    let Some(context) = &app.edit_in_place.active else { return };
    let col = sc.color(crate::viewport::c4(Tokens::get().body.gamma_multiply(0.35)));
    for (p, n) in context.triangles.iter() {
        sc.ghost_tri(*p, *n, col);
    }
}

/// Above the browser's rows while a part is edited in place: which assembly it is in (Finish
/// Editing is on the MODEL tab).
pub(crate) fn browser_strip(app: &SolveApp, ui: &mut egui::Ui) {
    let Some(context) = &app.edit_in_place.active else { return };
    let t = Tokens::get();
    let key = if app.edit_in_place.refreshing() { "ui.edit_in_place_refreshing" } else { "ui.edit_in_place_banner" };
    let text = crate::i18n::format(key, &[("name", &context.assembly_name)]);
    ui.horizontal(|ui| {
        ui.add_space(t.space_6);
        crate::ui_kit::text(ui, &text, crate::ui_kit::Text::Hint("edit_in_place"));
    });
    ui.add_space(t.space_3);
}

#[cfg(test)]
#[path = "edit_in_place_tests.rs"]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests;
