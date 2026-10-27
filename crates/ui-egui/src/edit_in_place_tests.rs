use super::*;
use serde_json::json;
use solidcraft_engine::Session;

fn volume_of(session: &Session, prefix: &str) -> f64 {
    session
        .world_state()
        .bodies
        .iter()
        .filter(|b| b.name.starts_with(prefix))
        .map(|b| solidcraft_engine::kernel::measure(&b.body).unwrap().volume)
        .sum()
}

fn volume(app: &SolveApp) -> f64 {
    volume_of(&app.session, "")
}

/// Ticks the window's background work, at least one frame's worth, until it is done: opens,
/// saves, Update Links and the assembly refresh of an edit in place.
fn settle(app: &mut SolveApp) {
    let ctx = egui::Context::default();
    let began = std::time::Instant::now();
    loop {
        crate::file_load::tick(app, &ctx);
        crate::save_job::tick(app, false);
        tick(app);
        let busy = !app.file_load.idle() || app.saving.is_some() || app.edit_in_place.finish_after_save.is_some() || app.edit_in_place.refreshing();
        if !busy {
            return;
        }
        assert!(began.elapsed() < std::time::Duration::from_secs(120), "the background work went on");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// Saves a box design at `path`; `extra` adds a second box on top of it.
fn save_box(path: &std::path::Path, size: [f64; 3], extra: bool) {
    let mut s = Session::default();
    s.execute("solid.box", &json!({"length": size[0], "width": size[1], "height": size[2]})).unwrap();
    if extra {
        s.execute("solid.box", &json!({"length": 5, "width": 5, "height": 10, "corner": [0, 0, size[2]], "operation": "join"})).unwrap();
    }
    s.execute("file.save", &json!({"path": path, "overwrite": true})).unwrap();
}

struct Fixture {
    app: SolveApp,
    folder: std::path::PathBuf,
    plate: u64,
    plate_file: std::path::PathBuf,
    post_file: std::path::PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

/// An assembly that links a 10x20x30 plate placed 50 mm along X and a 10 mm post cube placed
/// 100 mm the other way, saved next to them.
fn assembly(name: &str) -> Fixture {
    let folder = std::env::temp_dir().join(format!("solidcraft-edit-in-place-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    let (plate_file, post_file) = (folder.join("plate.solidcraft"), folder.join("post.solidcraft"));
    save_box(&plate_file, [10.0, 20.0, 30.0], false);
    save_box(&post_file, [10.0, 10.0, 10.0], false);
    let mut app = SolveApp::new(Session::default(), Default::default());
    let plate = app.run("component.insert_design", json!({"path": plate_file, "translate": [50, 0, 0]})).unwrap()["component"].as_u64().unwrap();
    app.run("component.insert_design", json!({"path": post_file, "translate": [-100, 0, 0]})).unwrap();
    app.run("file.save", json!({"path": folder.join("case.solidcraft")})).unwrap();
    assert!((volume(&app) - 7000.0).abs() < 1e-6);
    Fixture { app, folder, plate, plate_file, post_file }
}

fn showing(app: &SolveApp, file: &std::path::Path) -> bool {
    app.session.path.as_deref().is_some_and(|p| crate::documents::same_path(p, &file.to_string_lossy()))
}

fn context(app: &SolveApp) -> PartContext {
    app.edit_in_place.active.clone().expect("a part is edited in place")
}

#[test]
fn a_linked_component_is_edited_in_place_and_finish_editing_updates_the_assembly() {
    let mut f = assembly("finish");
    double_click_component(&mut f.app, json!({"component": f.plate}));
    settle(&mut f.app);
    // The part in a tab of its own, the post around it in the part's frame
    assert_eq!(f.app.docs.count(), 2);
    assert!(showing(&f.app, &f.plate_file));
    let c = context(&f.app);
    assert!(c.opened);
    assert_eq!(c.bodies, 1, "only the post: the part's own bodies are not context");
    assert!((c.bounds.min.x - -150.0).abs() < 1e-6 && (c.bounds.max.x - -140.0).abs() < 1e-6, "{:?}", c.bounds);
    assert!((volume(&f.app) - 6000.0).abs() < 1e-6, "the part's own design, not the assembly");
    // Changed and finished: saved, back in the assembly, the part's tab closed, the link read again
    f.app.run("solid.box", json!({"length": 10, "width": 10, "height": 10, "operation": "new"})).unwrap();
    finish(&mut f.app);
    settle(&mut f.app);
    assert!(f.app.edit_in_place.active.is_none());
    assert_eq!(f.app.docs.count(), 1);
    assert!(f.app.session.path.as_deref().is_some_and(|p| p.ends_with("case.solidcraft")));
    assert!((volume(&f.app) - 8000.0).abs() < 1e-6, "{}", volume(&f.app));
}

#[test]
fn with_changed_links_left_alone_finish_editing_updates_the_assembly_itself() {
    let mut f = assembly("finish-manual");
    f.app.preferences.update_changed_links = false;
    double_click_component(&mut f.app, json!({"component": f.plate}));
    settle(&mut f.app);
    f.app.run("solid.box", json!({"length": 10, "width": 10, "height": 10, "operation": "new"})).unwrap();
    finish(&mut f.app);
    settle(&mut f.app);
    assert_eq!(f.app.docs.count(), 1);
    assert!((volume(&f.app) - 8000.0).abs() < 1e-6, "Finish Editing promises the change: {}", volume(&f.app));
}

#[test]
fn update_links_in_the_part_refreshes_the_assembly_around_it() {
    let mut f = assembly("refresh");
    double_click_component(&mut f.app, json!({"component": f.plate}));
    settle(&mut f.app);
    let before = context(&f.app);
    // The post grows on disk (another window, a checkout) while the plate is edited
    save_box(&f.post_file, [10.0, 10.0, 10.0], true);
    f.app.start("component.update_links");
    assert!(f.app.edit_in_place.refreshing(), "the assembly is read again off the window thread");
    assert!(f.app.status.as_ref().is_some_and(|(s, _, error)| s.starts_with("Updating links in") && !error), "{:?}", f.app.status);
    settle(&mut f.app);
    // Still in the part, the assembly's tab updated and the picture around the part with it
    assert!(showing(&f.app, &f.plate_file));
    let after = context(&f.app);
    assert!(!Arc::ptr_eq(&before.triangles, &after.triangles));
    assert!((after.bounds.max.z - 20.0).abs() < 1e-6, "the post's new box shows: {:?}", after.bounds);
    let assembly = crate::documents::session_at(&f.app, &after.assembly).unwrap();
    assert!((volume_of(assembly, "") - 7250.0).abs() < 1e-6, "{}", volume_of(assembly, ""));
    assert!(assembly.is_dirty(), "the assembly keeps its own copy of the linked bodies");
    assert_eq!(f.app.status.as_ref().map(|s| s.0.as_str()), Some("Update Links done"));
    // Finishing afterwards reads nothing again: the refresh read every link
    finish(&mut f.app);
    assert!(f.app.file_load.idle(), "the assembly read its links in the refresh already");
    settle(&mut f.app);
    assert!((volume(&f.app) - 7250.0).abs() < 1e-6, "{}", volume(&f.app));
}

#[test]
fn looking_at_the_assembly_while_it_refreshes_keeps_the_refresh() {
    let mut f = assembly("looked");
    f.app.preferences.update_changed_links = false;
    double_click_component(&mut f.app, json!({"component": f.plate}));
    settle(&mut f.app);
    save_box(&f.post_file, [10.0, 10.0, 10.0], true);
    refresh(&mut f.app);
    crate::documents::switch(&mut f.app, 0);
    crate::documents::switch(&mut f.app, 1);
    settle(&mut f.app);
    let assembly = crate::documents::session_at(&f.app, &context(&f.app).assembly).unwrap();
    assert!((volume_of(assembly, "") - 7250.0).abs() < 1e-6, "showing the assembly changes nothing in it: {}", volume_of(assembly, ""));
}

#[test]
fn an_edit_whose_part_does_not_open_is_forgotten() {
    let mut f = assembly("unopened");
    std::fs::remove_file(&f.plate_file).unwrap();
    double_click_component(&mut f.app, json!({"component": f.plate}));
    settle(&mut f.app);
    assert!(f.app.edit_in_place.active.is_none() && f.app.edit_in_place.pending.is_none());
    assert_eq!(f.app.docs.count(), 1);
    // The part opened by hand later is a plain tab, not an old edit
    save_box(&f.plate_file, [10.0, 20.0, 30.0], false);
    let mut opened = Session::default();
    opened.execute("doc.open", &json!({"path": f.plate_file})).unwrap();
    crate::documents::open_tab(&mut f.app, opened).unwrap();
    assert!(showing(&f.app, &f.plate_file));
    assert!(f.app.edit_in_place.active.is_none());
}

#[test]
fn finishing_a_part_saved_to_another_file_keeps_the_edit_and_says_so() {
    let mut f = assembly("elsewhere");
    double_click_component(&mut f.app, json!({"component": f.plate}));
    settle(&mut f.app);
    f.app.run("file.save", json!({"path": f.folder.join("copy.solidcraft")})).unwrap();
    finish(&mut f.app);
    settle(&mut f.app);
    assert!(f.app.edit_in_place.active.is_some(), "the assembly still links the part, not the copy");
    assert!(f.app.session.path.as_deref().is_some_and(|p| p.ends_with("copy.solidcraft")));
    assert!(f.app.status.as_ref().is_some_and(|(s, _, error)| *error && s.contains("plate")), "{:?}", f.app.status);
}

#[test]
fn a_refresh_for_an_assembly_changed_meanwhile_is_dropped() {
    let mut f = assembly("dropped");
    // Coming back to the assembly would read the changed post itself: only the edit made there counts here
    f.app.preferences.update_changed_links = false;
    double_click_component(&mut f.app, json!({"component": f.plate}));
    settle(&mut f.app);
    save_box(&f.post_file, [10.0, 10.0, 10.0], true);
    refresh(&mut f.app);
    // Before the result is taken up, the assembly changes in its own tab
    crate::documents::switch(&mut f.app, 0);
    f.app.run("solid.box", json!({"length": 1, "width": 1, "height": 1, "corner": [0, 300, 0], "operation": "new"})).unwrap();
    crate::documents::switch(&mut f.app, 1);
    settle(&mut f.app);
    let assembly = crate::documents::session_at(&f.app, &context(&f.app).assembly).unwrap();
    assert!((volume_of(assembly, "") - 7001.0).abs() < 1e-6, "the change made meanwhile stays, the stale result goes: {}", volume_of(assembly, ""));
    assert!(f.app.status.as_ref().is_some_and(|(_, _, error)| *error), "{:?}", f.app.status);
    // Run again, it goes through
    refresh(&mut f.app);
    settle(&mut f.app);
    let assembly = crate::documents::session_at(&f.app, &context(&f.app).assembly).unwrap();
    assert!((volume_of(assembly, "") - 7251.0).abs() < 1e-6, "{}", volume_of(assembly, ""));
}

#[test]
fn with_the_assembly_closed_the_edit_stays_and_says_so() {
    let mut f = assembly("closed");
    double_click_component(&mut f.app, json!({"component": f.plate}));
    settle(&mut f.app);
    crate::documents::close(&mut f.app, 0, false);
    assert_eq!(f.app.docs.count(), 1);
    assert!(showing(&f.app, &f.plate_file));
    refresh(&mut f.app);
    assert!(!f.app.edit_in_place.refreshing());
    assert!(f.app.status.as_ref().is_some_and(|(_, _, error)| *error), "{:?}", f.app.status);
    finish(&mut f.app);
    settle(&mut f.app);
    assert!(f.app.edit_in_place.active.is_some(), "nothing to go back to: the edit stays");
    assert!(showing(&f.app, &f.plate_file));
    // Closing the part's tab ends the edit with it
    crate::documents::close(&mut f.app, 0, true);
    assert!(f.app.edit_in_place.active.is_none());
}

#[test]
fn a_part_already_open_is_reused_and_stays_open() {
    let mut f = assembly("reused");
    let mut opened = Session::default();
    opened.execute("doc.open", &json!({"path": f.plate_file})).unwrap();
    crate::documents::open_tab(&mut f.app, opened).unwrap();
    crate::documents::switch(&mut f.app, 0);
    double_click_component(&mut f.app, json!({"component": f.plate}));
    settle(&mut f.app);
    assert_eq!(f.app.docs.count(), 2);
    assert!(showing(&f.app, &f.plate_file));
    assert!(!context(&f.app).opened);
    finish(&mut f.app);
    settle(&mut f.app);
    assert_eq!(f.app.docs.count(), 2, "the tab was there before the edit, so it stays");
    assert!(f.app.session.path.as_deref().is_some_and(|p| p.ends_with("case.solidcraft")));
}

#[test]
fn a_turned_placement_draws_the_assembly_turned_the_other_way() {
    let mut f = assembly("turned");
    // A quarter turn about Z through the plate's origin
    let turn = json!({"occurrence": "plate:1", "space": "world", "axis": [0, 0, 1], "angle": "90 deg", "origin": [50, 0, 0], "move_grounded": true});
    f.app.run("occurrence.move", turn).unwrap();
    f.app.run("file.save", json!({})).unwrap();
    double_click_component(&mut f.app, json!({"component": f.plate}));
    settle(&mut f.app);
    // The post at x -100..-90, y 0..10 lies along +Y of the turned plate: x 0..10, y 140..150
    let b = context(&f.app).bounds;
    assert!((b.min.y - 140.0).abs() < 1e-6 && (b.max.y - 150.0).abs() < 1e-6, "{b:?}");
    assert!((b.min.x - 0.0).abs() < 1e-6 && (b.max.x - 10.0).abs() < 1e-6, "{b:?}");
}

#[test]
fn a_second_placement_is_the_frame_and_no_copy_of_the_part_is_context() {
    let mut f = assembly("second");
    let second = f.app.run("component.insert_design", json!({"path": f.plate_file, "translate": [0, 100, 0], "reuse": true})).unwrap();
    assert_eq!(second["component"].as_u64(), Some(f.plate), "{second}");
    f.app.run("file.save", json!({})).unwrap();
    let (_, path, world) = f.app.session.doc.placements().into_iter().find(|(c, _, world)| *c == f.plate && world[3][1] > 50.0).unwrap();
    double_click_component(&mut f.app, json!({"component": f.plate, "path": path}));
    settle(&mut f.app);
    let c = context(&f.app);
    assert_eq!(c.bodies, 1, "both placements of the plate are the part itself");
    // The post's corner at (-100, 0) seen from the second placement, not the first
    let (x, y) = (-100.0 - world[3][0], -world[3][1]);
    assert!((c.bounds.min.x - x).abs() < 1e-6 && (c.bounds.min.y - y).abs() < 1e-6, "{:?} from {:?}", c.bounds, world[3]);
    assert!(world[3][1] > 50.0 && (y - -100.0).abs() < 1e-6, "the frame is the second placement's: {:?}", world[3]);
}

#[test]
fn double_clicking_a_linked_part_in_the_view_edits_the_placement_clicked() {
    let mut f = assembly("double");
    f.app.run("component.insert_design", json!({"path": f.plate_file, "translate": [0, 100, 0], "reuse": true})).unwrap();
    f.app.run("file.save", json!({})).unwrap();
    let (_, path, world) = f.app.session.doc.placements().into_iter().find(|(c, _, world)| *c == f.plate && world[3][1] > 50.0).unwrap();
    let body = f.app.session.occurrence_bodies(&path, true).unwrap().into_iter().next().unwrap();
    assert!(double_click_face(&mut f.app, &body));
    settle(&mut f.app);
    assert!(showing(&f.app, &f.plate_file));
    let c = context(&f.app);
    assert_eq!(c.placement, path, "the placement whose face was clicked, not the first one");
    assert!((c.bounds.min.y - -world[3][1]).abs() < 1e-6, "{:?}", c.bounds);
}

#[test]
fn double_clicking_a_component_modelled_here_activates_it_once() {
    let mut f = assembly("double-activate");
    let bracket = f.app.run("component.create", json!({"name": "Bracket"})).unwrap()["component"].as_u64().unwrap();
    let made = f.app.run("solid.box", json!({"length": 5, "width": 5, "height": 5, "corner": [0, -200, 0]})).unwrap();
    let body = made["created_bodies"][0].as_str().unwrap().to_string();
    f.app.run("component.activate", json!({"component": "root"})).unwrap();
    assert!(double_click_face(&mut f.app, &body));
    assert_eq!(f.app.session.active_component, bracket);
    assert_eq!(f.app.docs.count(), 1, "a component modelled here opens nothing");
    assert!(f.app.edit_in_place.active.is_none() && f.app.edit_in_place.pending.is_none());
    // Active already: the double-click is left to whatever the face itself offers
    assert!(!double_click_face(&mut f.app, &body));
}

#[test]
fn double_clicking_a_body_of_this_design_or_with_the_preference_off_does_nothing_here() {
    let mut f = assembly("double-off");
    let made = f.app.run("solid.box", json!({"length": 5, "width": 5, "height": 5, "corner": [0, -200, 0], "operation": "new"})).unwrap();
    let own = made["created_bodies"][0].as_str().unwrap().to_string();
    assert!(!double_click_face(&mut f.app, &own), "a body modelled here is not a linked design");
    let linked = f.app.session.occurrence_bodies(&f.app.session.doc.first_path(f.plate), true).unwrap().into_iter().next().unwrap();
    f.app.preferences.double_click_components = false;
    assert!(!double_click_face(&mut f.app, &linked));
    settle(&mut f.app);
    assert_eq!(f.app.docs.count(), 1);
    assert!(f.app.edit_in_place.active.is_none() && f.app.edit_in_place.pending.is_none());
}

#[test]
fn activate_only_activates_and_edit_in_place_opens_the_placement_right_clicked() {
    use crate::context_menu::{self, Target};
    let mut f = assembly("menu");
    let inserted = f.app.run("component.insert_design", json!({"path": f.plate_file, "translate": [0, 100, 0], "reuse": true})).unwrap();
    let second = inserted["occurrence"].as_u64().unwrap();
    f.app.run("file.save", json!({})).unwrap();
    let item = |app: &SolveApp, target: Target, id: &str| context_menu::items(app, &target).into_iter().find(|i| i.id == id).unwrap();
    // Activate: the linked component becomes the active one, nothing opens
    let activate = item(&f.app, Target::Component { id: f.plate }, "ui.activate");
    context_menu::run_item(&mut f.app, &activate, egui::Pos2::ZERO);
    settle(&mut f.app);
    assert_eq!(f.app.session.active_component, f.plate);
    assert_eq!(f.app.docs.count(), 1);
    assert!(f.app.edit_in_place.active.is_none() && f.app.edit_in_place.pending.is_none());
    // Edit in Place on the second placement: the part opens around that one
    let path = f.app.session.doc.placements().into_iter().map(|(_, path, _)| path).find(|p| p.last() == Some(&second)).unwrap();
    let edit = item(&f.app, Target::Occurrence { path: path.clone() }, "ui.editInPlace");
    context_menu::run_item(&mut f.app, &edit, egui::Pos2::ZERO);
    settle(&mut f.app);
    assert!(showing(&f.app, &f.plate_file));
    assert_eq!(context(&f.app).placement, path);
}

#[test]
fn finish_editing_is_a_model_tab_button_only_while_a_part_is_edited_in_place() {
    let mut f = assembly("ribbon");
    let mut h = crate::scenario::Harness::new();
    h.app = std::mem::replace(&mut f.app, SolveApp::new(Session::default(), Default::default()));
    h.app.home.open = false;
    h.frames(3);
    assert!(crate::scenario::rect_of("panel:MODEL:FINISH EDITING").is_none(), "no edit in place, no button");
    double_click_component(&mut h.app, json!({"component": f.plate}));
    settle(&mut h.app);
    h.frames(3);
    assert!(crate::scenario::rect_of("panel:MODEL:FINISH EDITING").is_some(), "the part's MODEL tab ends with Finish Editing");
    h.app.start("component.finish_edit");
    settle(&mut h.app);
    h.frames(3);
    assert!(h.app.edit_in_place.active.is_none());
    assert_eq!(h.app.docs.count(), 1);
    assert!(crate::scenario::rect_of("panel:MODEL:FINISH EDITING").is_none());
}

#[test]
fn the_back_button_finishes_the_edit_unless_a_dialog_is_open() {
    let mut f = assembly("back");
    back(&mut f.app);
    assert_eq!(f.app.docs.count(), 1, "nothing is edited in place: nothing happens");
    double_click_component(&mut f.app, json!({"component": f.plate}));
    settle(&mut f.app);
    f.app.start("solid.box");
    assert!(f.app.dialog.is_some());
    back(&mut f.app);
    settle(&mut f.app);
    assert!(f.app.edit_in_place.active.is_some() && f.app.dialog.is_some(), "the open dialog keeps the edit");
    crate::dialogs::cancel(&mut f.app);
    back(&mut f.app);
    settle(&mut f.app);
    assert!(f.app.edit_in_place.active.is_none());
    assert_eq!(f.app.docs.count(), 1);
}

#[test]
fn turned_off_or_unsaved_double_clicking_a_linked_component_does_what_it_did() {
    let mut f = assembly("off");
    f.app.preferences.double_click_components = false;
    double_click_component(&mut f.app, json!({"component": f.plate}));
    settle(&mut f.app);
    assert_eq!(f.app.session.active_component, f.plate);
    assert_eq!(f.app.docs.count(), 1);
    assert!(f.app.edit_in_place.active.is_none());
    // An assembly never saved has no folder to find its links from
    let mut fresh = SolveApp::new(Session::default(), Default::default());
    let linked = fresh.run("component.insert_design", json!({"path": f.plate_file})).unwrap()["component"].as_u64().unwrap();
    double_click_component(&mut fresh, json!({"component": linked}));
    settle(&mut fresh);
    assert!(fresh.edit_in_place.active.is_none() && fresh.edit_in_place.pending.is_none());
    assert!(fresh.status.as_ref().is_some_and(|(_, _, error)| *error), "{:?}", fresh.status);
}
